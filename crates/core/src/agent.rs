//! Agent loop — то, что связывает провайдера, тулы, политику доступа и запись
//! сессии в один работающий агент.
//!
//! Снаружи модуль выглядит как канал: [`Agent::spawn`] отдаёт [`AgentHandle`]
//! (в него кладут [`Op`]) и `UnboundedReceiver<Event>` (из него читают
//! [`Event`]). Никакого UI ядро не знает: CLI — лишь первый потребитель этого
//! канала, TUI и ACP придут к нему же.
//!
//! # Ход
//!
//! [`Op::UserInput`] запускает ход: `TurnStarted` → цикл сэмплинга. Каждая
//! итерация — запрос к провайдеру, трансляция дельт в события и, если модель
//! попросила тулы, их последовательное выполнение:
//!
//! ```text
//! Op::UserInput ─┐
//!                ▼
//!         Event::TurnStarted
//!                ▼
//!   ┌──► provider.stream() ──► TextDelta / ThinkingDelta ──► MessageComplete + Usage
//!   │            │
//!   │            ├─ нет tool_use ──► TurnFinished{Completed}
//!   │            │
//!   │            └─ есть tool_use ──► гейт политики ──► ApprovalRequired ⇄ Op::Approve
//!   │                                       │
//!   │                                       ▼
//!   │                          ToolCallStarted ──► ToolCallFinished
//!   └───────── tool results одним сообщением в историю ◄────┘
//! ```
//!
//! Предохранитель — `max_turns` итераций сэмплинга на ход
//! (`TurnFinished{MaxTurns}`).
//!
//! # Инварианты
//!
//! * **История append-only.** Сообщения только дописываются, никогда не
//!   переписываются: префикс промпта обязан быть стабильным, иначе KV-кэш
//!   локального сервера инвалидируется на каждом запросе. Фильтр сборки
//!   запроса — `agent_visible` (фундамент компакции из task 7).
//! * **У каждого `tool_use` есть парный `tool_result`.** Даже если ход прерван
//!   или тул запрещён политикой: строгие OpenAI-совместимые сервера отвергают
//!   запрос с «висящим» тул-коллом.
//! * **Отказ и ошибка — это данные для модели.** Запрет политики, отказ
//!   пользователя, невалидные аргументы — всё возвращается как tool result с
//!   `is_error = true` и текстом «что не так и что делать»; модель чинится сама
//!   (retry-with-error).
//! * **Отмена — один примитив.** [`CancellationToken`]: у хода свой дочерний
//!   токен, у каждого тул-колла — дочерний от него. [`AgentHandle::cancel_token`]
//!   отдаёт корневой: его отмена гасит и текущий ход, и все следующие.
//!
//! # Что ядро НЕ делает в MVP
//!
//! `StreamChunk::ToolCallDelta` наружу не транслируется (аргументы всё равно
//! показываются целиком в `ToolCallStarted`), тул-коллы исполняются строго
//! последовательно, а [`Op::Compact`] пока игнорируется — компакция придёт в
//! task 7.

use std::collections::VecDeque;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use futures::StreamExt;
use serde_json::Value;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::event::{ApprovalDecision, Event, Op, StopReason};
use crate::message::{ContentBlock, Message, Usage};
use crate::permission::{self, PermissionCheck, PermissionPolicy};
use crate::prompt;
use crate::provider::{Provider, ProviderError, ProviderRequest, StreamChunk, ToolDef};
use crate::session::{SessionItem, SessionRecorder};
use crate::tools::{ToolCtx, ToolRegistry};

/// Сколько раз за один ход агент готов сходить к модели, если не задано иное.
pub const DEFAULT_MAX_TURNS: usize = 50;

/// Настройки агента.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentConfig {
    /// Идентификатор модели в терминах сервера.
    pub model: String,
    /// Предохранитель: максимум итераций сэмплинга на один ход.
    pub max_turns: usize,
    /// Верхняя граница генерации, если задана.
    pub max_tokens: Option<u32>,
    /// Температура сэмплинга, если задана.
    pub temperature: Option<f32>,
    /// Политика доступа к тулам.
    pub policy: PermissionPolicy,
    /// Рабочая директория: от неё резолвятся пути тулов и в ней ищется
    /// `AGENTS.md`.
    pub cwd: PathBuf,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
            max_turns: DEFAULT_MAX_TURNS,
            max_tokens: None,
            temperature: None,
            policy: PermissionPolicy::default(),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }
}

impl AgentConfig {
    /// Конфиг для модели; остальное — дефолты.
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            ..Self::default()
        }
    }

    /// Задать рабочую директорию.
    #[must_use]
    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = cwd.into();
        self
    }

    /// Задать политику доступа.
    #[must_use]
    pub fn with_policy(mut self, policy: PermissionPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Задать предохранитель `max_turns`.
    #[must_use]
    pub fn with_max_turns(mut self, max_turns: usize) -> Self {
        self.max_turns = max_turns;
        self
    }

    /// Задать лимит генерации.
    #[must_use]
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// Задать температуру.
    #[must_use]
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }
}

/// Агент: провайдер + тулы + конфиг, ещё не запущенный.
///
/// Собирается синхронно (в том числе читает `AGENTS.md`), запускается
/// [`Agent::spawn`].
pub struct Agent {
    provider: Arc<dyn Provider>,
    registry: ToolRegistry,
    config: AgentConfig,
    recorder: Option<SessionRecorder>,
    system: String,
    tool_defs: Vec<ToolDef>,
    history: Vec<Message>,
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent")
            .field("provider", &self.provider.name())
            .field("config", &self.config)
            .field("tools", &self.registry.names())
            .field("recorded", &self.recorder.is_some())
            .field("history_len", &self.history.len())
            .finish()
    }
}

impl Agent {
    /// Собрать агента. Системный промпт строится здесь один раз
    /// ([`prompt::system_prompt`], включая `AGENTS.md` рядом с `cwd`), список
    /// тулов фиксируется в порядке регистрации — оба обязаны быть стабильными
    /// между запросами.
    pub fn new(provider: Arc<dyn Provider>, registry: ToolRegistry, config: AgentConfig) -> Self {
        let system = prompt::system_prompt(&config.cwd);
        let tool_defs = registry.defs();
        Self {
            provider,
            registry,
            config,
            recorder: None,
            system,
            tool_defs,
            history: Vec::new(),
        }
    }

    /// Писать ход в JSONL-сессию. Писатель закрывается сам, когда агент
    /// останавливается (канал [`Op`] закрыт).
    #[must_use]
    pub fn with_recorder(mut self, recorder: SessionRecorder) -> Self {
        self.recorder = Some(recorder);
        self
    }

    /// Заменить системный промпт целиком (тесты, эвалы, `--system` в CLI).
    #[must_use]
    pub fn with_system_prompt(mut self, system: impl Into<String>) -> Self {
        self.system = system.into();
        self
    }

    /// Системный промпт, с которым агент пойдёт к модели.
    pub fn system_prompt(&self) -> &str {
        &self.system
    }

    /// Настройки агента.
    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    /// Запустить agent loop в отдельной tokio-задаче.
    ///
    /// Возвращает ручку для [`Op`] и приёмник [`Event`]. Задача живёт, пока жив
    /// хотя бы один [`AgentHandle`]: когда последний уходит, цикл дочитывает
    /// очередь, закрывает сессию и завершается.
    pub fn spawn(self) -> (AgentHandle, UnboundedReceiver<Event>) {
        let (op_tx, op_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();

        let agent_loop = AgentLoop {
            provider: self.provider,
            registry: self.registry,
            config: self.config,
            recorder: self.recorder,
            system: self.system,
            tool_defs: self.tool_defs,
            history: self.history,
            events: event_tx,
            ops: OpChannel {
                rx: op_rx,
                pending: VecDeque::new(),
                closed: false,
            },
            cancel: cancel.clone(),
        };

        let task = tokio::spawn(agent_loop.run());
        (
            AgentHandle {
                ops: op_tx,
                cancel,
                task,
            },
            event_rx,
        )
    }
}

/// Ручка запущенного агента: сюда кладут [`Op`].
#[derive(Debug)]
pub struct AgentHandle {
    ops: UnboundedSender<Op>,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

impl AgentHandle {
    /// Отправить команду агенту.
    ///
    /// Не блокирует и не паникует: если цикл уже остановлен, потеря команды
    /// логируется. Команды, пришедшие посреди чужого хода (новый ввод,
    /// аппрув не того тула), буферизуются и разбираются после его завершения —
    /// на этом вырастет steering из фазы 3.
    pub fn submit(&self, op: Op) {
        if let Err(error) = self.ops.send(op) {
            tracing::error!(%error, "agent loop остановлен, команда потеряна");
        }
    }

    /// Прервать текущий ход — то же, что [`Op::Interrupt`].
    pub fn interrupt(&self) {
        self.submit(Op::Interrupt);
    }

    /// Корневой токен отмены.
    ///
    /// Токены ходов — его дети, поэтому отмена корневого гасит текущий ход и
    /// все последующие. Для «прервать только этот ход» есть [`Op::Interrupt`].
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Остановить агента и дождаться, пока он закроет сессию.
    pub async fn shutdown(self) {
        let Self { ops, task, .. } = self;
        drop(ops);
        if let Err(error) = task.await {
            tracing::error!(%error, "задача agent loop завершилась аварийно");
        }
    }
}

/// Канал команд плюс буфер того, что пришло не вовремя.
struct OpChannel {
    rx: UnboundedReceiver<Op>,
    /// Команды, полученные во время хода и к нему не относящиеся.
    pending: VecDeque<Op>,
    /// Отправитель закрыт — ждать больше нечего.
    closed: bool,
}

impl OpChannel {
    /// Следующая команда: сперва отложенные, потом из канала.
    async fn next(&mut self) -> Option<Op> {
        if let Some(op) = self.pending.pop_front() {
            return Some(op);
        }
        if self.closed {
            return None;
        }
        match self.rx.recv().await {
            Some(op) => Some(op),
            None => {
                self.closed = true;
                None
            }
        }
    }

    /// Достать отложенный аппрув для конкретного тул-колла, если он пришёл
    /// раньше, чем мы начали его ждать.
    fn take_pending_approval(&mut self, id: &str) -> Option<ApprovalDecision> {
        let position = self
            .pending
            .iter()
            .position(|op| matches!(op, Op::Approve { id: candidate, .. } if candidate == id))?;
        match self.pending.remove(position) {
            Some(Op::Approve { decision, .. }) => Some(decision),
            _ => None,
        }
    }
}

/// Исход гонки «полезная работа против входящих команд».
enum Raced<T> {
    /// Работа доведена до конца.
    Done(T),
    /// Пришёл [`Op::Interrupt`] (или сработал токен отмены).
    Interrupted,
}

/// Дождаться `fut`, параллельно разбирая входящие команды.
///
/// [`Op::Interrupt`] отменяет токен хода и **бросает** `fut` — так рвётся
/// зависший стрим провайдера, который иначе не остановить. Прочие команды
/// откладываются в буфер.
async fn race_ops<F: Future>(
    fut: F,
    ops: &mut OpChannel,
    turn_cancel: &CancellationToken,
) -> Raced<F::Output> {
    tokio::pin!(fut);
    loop {
        if ops.closed {
            return tokio::select! {
                output = &mut fut => Raced::Done(output),
                () = turn_cancel.cancelled() => Raced::Interrupted,
            };
        }
        tokio::select! {
            output = &mut fut => return Raced::Done(output),
            () = turn_cancel.cancelled() => return Raced::Interrupted,
            op = ops.rx.recv() => match op {
                Some(Op::Interrupt) => {
                    turn_cancel.cancel();
                    return Raced::Interrupted;
                }
                Some(other) => ops.pending.push_back(other),
                None => ops.closed = true,
            },
        }
    }
}

/// То же, но `fut` не бросается: на прерывании токен отменяется, а работа
/// дожидается.
///
/// Так исполняются тулы: `bash` по отмене успевает убить процесс-группу и
/// вернуть внятный результат, а брошенная посреди работы задача оставила бы
/// сиротой всё дерево процессов. Возвращается ещё и флаг «ход прерван».
async fn race_ops_awaiting<F: Future>(
    fut: F,
    ops: &mut OpChannel,
    turn_cancel: &CancellationToken,
) -> (F::Output, bool) {
    tokio::pin!(fut);
    let mut interrupted = turn_cancel.is_cancelled();
    loop {
        if interrupted {
            return ((&mut fut).await, true);
        }
        if ops.closed {
            tokio::select! {
                output = &mut fut => return (output, false),
                () = turn_cancel.cancelled() => interrupted = true,
            }
            continue;
        }
        tokio::select! {
            output = &mut fut => return (output, false),
            () = turn_cancel.cancelled() => interrupted = true,
            op = ops.rx.recv() => match op {
                Some(Op::Interrupt) => {
                    turn_cancel.cancel();
                    interrupted = true;
                }
                Some(other) => ops.pending.push_back(other),
                None => ops.closed = true,
            },
        }
    }
}

/// Решение по запросу доступа.
enum Approval {
    Allow,
    Deny,
    Interrupted,
}

/// Дождаться [`Op::Approve`] с нужным `id`.
///
/// Аппрувы для других тул-коллов и прочие команды откладываются; закрытие
/// канала означает, что отвечать некому — ход прерывается.
async fn wait_for_approval(
    id: &str,
    ops: &mut OpChannel,
    turn_cancel: &CancellationToken,
) -> Approval {
    if let Some(decision) = ops.take_pending_approval(id) {
        return decision.into();
    }
    loop {
        if turn_cancel.is_cancelled() {
            return Approval::Interrupted;
        }
        if ops.closed {
            tracing::warn!(%id, "канал команд закрыт, аппрув не придёт");
            return Approval::Interrupted;
        }
        tokio::select! {
            () = turn_cancel.cancelled() => return Approval::Interrupted,
            op = ops.rx.recv() => match op {
                Some(Op::Interrupt) => {
                    turn_cancel.cancel();
                    return Approval::Interrupted;
                }
                Some(Op::Approve { id: candidate, decision }) if candidate == id => {
                    return decision.into();
                }
                Some(other) => {
                    tracing::debug!(?other, %id, "команда отложена: ждём аппрув");
                    ops.pending.push_back(other);
                }
                None => ops.closed = true,
            },
        }
    }
}

impl From<ApprovalDecision> for Approval {
    fn from(decision: ApprovalDecision) -> Self {
        if decision.is_allowed() {
            Self::Allow
        } else {
            Self::Deny
        }
    }
}

/// Запрошенный моделью тул-колл, вынутый из ответа.
#[derive(Debug, Clone)]
struct PendingCall {
    id: String,
    name: String,
    input: Value,
}

/// Состояние запущенного агента: всё, что живёт внутри tokio-задачи.
struct AgentLoop {
    provider: Arc<dyn Provider>,
    registry: ToolRegistry,
    config: AgentConfig,
    recorder: Option<SessionRecorder>,
    system: String,
    tool_defs: Vec<ToolDef>,
    history: Vec<Message>,
    events: UnboundedSender<Event>,
    ops: OpChannel,
    cancel: CancellationToken,
}

impl AgentLoop {
    /// Разбор команд, пока канал жив.
    async fn run(mut self) {
        while let Some(op) = self.ops.next().await {
            match op {
                Op::UserInput { text } => self.run_turn(text).await,
                Op::Interrupt => tracing::debug!("Interrupt вне хода — нечего прерывать"),
                Op::Approve { id, .. } => {
                    tracing::debug!(%id, "Approve без ожидающего тул-колла — игнорируем");
                }
                // TODO(task 7): ручная компакция истории через провайдера.
                Op::Compact => tracing::debug!("Op::Compact появится в task 7"),
            }
        }

        if let Some(recorder) = self.recorder.take() {
            recorder.close().await;
        }
    }

    /// Один ход: ввод пользователя → цикл сэмплинга → `TurnFinished`.
    async fn run_turn(&mut self, text: String) {
        let turn_id = Uuid::new_v4().to_string();
        let turn_cancel = self.cancel.child_token();

        self.emit(Event::TurnStarted {
            turn_id: turn_id.clone(),
        });
        self.record(SessionItem::turn_started(turn_id.as_str()));
        self.push_message(Message::user(text));

        let reason = self.sample_loop(&turn_cancel).await;

        self.emit(Event::TurnFinished { reason });
        self.record(SessionItem::turn_finished(turn_id.as_str(), reason));
    }

    /// Цикл сэмплинга: запрос → дельты → тулы → снова запрос.
    async fn sample_loop(&mut self, turn_cancel: &CancellationToken) -> StopReason {
        for _ in 0..self.config.max_turns {
            if turn_cancel.is_cancelled() {
                return StopReason::Cancelled;
            }

            let provider = Arc::clone(&self.provider);
            let request = self.build_request();
            let raced = race_ops(provider.stream(request), &mut self.ops, turn_cancel).await;
            let mut stream = match raced {
                Raced::Interrupted => return StopReason::Cancelled,
                Raced::Done(Ok(stream)) => stream,
                Raced::Done(Err(error)) => return self.fail(error),
            };

            let mut complete: Option<(Message, Usage)> = None;
            loop {
                let raced = race_ops(stream.next(), &mut self.ops, turn_cancel).await;
                match raced {
                    Raced::Interrupted => return StopReason::Cancelled,
                    Raced::Done(None) => break,
                    Raced::Done(Some(Err(error))) => return self.fail(error),
                    Raced::Done(Some(Ok(chunk))) => match chunk {
                        StreamChunk::TextDelta(text) => self.emit(Event::TextDelta { text }),
                        StreamChunk::ThinkingDelta(text) => {
                            self.emit(Event::ThinkingDelta { text })
                        }
                        // В MVP наружу не транслируется: аргументы всё равно
                        // уедут целиком в ToolCallStarted.
                        StreamChunk::ToolCallDelta { .. } => {}
                        StreamChunk::Complete { message, usage } => {
                            complete = Some((message, usage))
                        }
                    },
                }
            }

            let Some((message, usage)) = complete else {
                self.emit(Event::Error {
                    message: "провайдер закрыл поток, не отдав готовое сообщение".to_string(),
                });
                return StopReason::Error;
            };

            let calls: Vec<PendingCall> = message
                .tool_uses()
                .map(|(id, name, input)| PendingCall {
                    id: id.to_string(),
                    name: name.to_string(),
                    input: input.clone(),
                })
                .collect();

            self.push_message(message.clone());
            self.emit(Event::MessageComplete { message });
            self.emit(Event::Usage { usage });

            if calls.is_empty() {
                return StopReason::Completed;
            }
            if !self.run_tool_calls(calls, turn_cancel).await {
                return StopReason::Cancelled;
            }
        }

        StopReason::MaxTurns
    }

    /// Исполнить тул-коллы одного ответа модели — по порядку, по одному.
    ///
    /// Результаты уходят в историю **одним** сообщением из `ToolResult`-блоков,
    /// по блоку на каждый `tool_use` — включая запрещённые, отклонённые и не
    /// дошедшие до исполнения. Возвращает `false`, если ход прерван.
    async fn run_tool_calls(
        &mut self,
        calls: Vec<PendingCall>,
        turn_cancel: &CancellationToken,
    ) -> bool {
        let mut results: Vec<ContentBlock> = Vec::with_capacity(calls.len());
        let mut interrupted = false;

        for call in &calls {
            match self.config.policy.check(&call.name) {
                PermissionCheck::Allowed => {}
                PermissionCheck::Denied { reason } => {
                    self.finish_tool_call(&mut results, call, reason, true);
                    continue;
                }
                PermissionCheck::NeedsApproval => {
                    self.emit(Event::ApprovalRequired {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        input: call.input.clone(),
                    });
                    let approval = wait_for_approval(&call.id, &mut self.ops, turn_cancel).await;
                    match approval {
                        Approval::Allow => {}
                        Approval::Deny => {
                            let reason = permission::user_denial(&call.name);
                            self.finish_tool_call(&mut results, call, reason, true);
                            continue;
                        }
                        Approval::Interrupted => {
                            interrupted = true;
                            break;
                        }
                    }
                }
            }

            self.emit(Event::ToolCallStarted {
                id: call.id.clone(),
                name: call.name.clone(),
                input: call.input.clone(),
            });

            let ctx = ToolCtx::with_cancel(self.config.cwd.clone(), turn_cancel.child_token());
            let execution = self.registry.execute(&call.name, call.input.clone(), &ctx);
            let (output, cancelled) =
                race_ops_awaiting(execution, &mut self.ops, turn_cancel).await;

            self.finish_tool_call(&mut results, call, output.content, output.is_error);
            if cancelled {
                interrupted = true;
                break;
            }
        }

        // Тул-коллы, до которых ход не дошёл, всё равно получают результат:
        // без пары `tool_use` ломает следующий запрос.
        for call in calls.iter().skip(results.len()) {
            results.push(ContentBlock::tool_error(
                call.id.as_str(),
                permission::interrupted_result(),
            ));
        }
        if !results.is_empty() {
            self.push_message(Message::tool(results));
        }

        !interrupted
    }

    /// Событие завершения тул-колла плюс блок результата для истории.
    fn finish_tool_call(
        &self,
        results: &mut Vec<ContentBlock>,
        call: &PendingCall,
        content: String,
        is_error: bool,
    ) {
        self.emit(Event::ToolCallFinished {
            id: call.id.clone(),
            content: content.clone(),
            is_error,
        });
        results.push(ContentBlock::ToolResult {
            tool_use_id: call.id.clone(),
            content,
            is_error,
        });
    }

    /// Запрос к провайдеру из текущего состояния.
    fn build_request(&self) -> ProviderRequest {
        ProviderRequest {
            model: self.config.model.clone(),
            system: self.system.clone(),
            messages: self
                .history
                .iter()
                .filter(|message| message.is_agent_visible())
                .cloned()
                .collect(),
            tools: self.tool_defs.clone(),
            max_tokens: self.config.max_tokens,
            temperature: self.config.temperature,
        }
    }

    /// Ошибка провайдера: наружу — [`Event::Error`], ход — `TurnFinished{Error}`.
    fn fail(&self, error: ProviderError) -> StopReason {
        if matches!(error, ProviderError::ContextLengthExceeded) {
            // TODO(task 7): точка компакции — сжать историю и повторить запрос
            // один раз, вместо того чтобы валить ход.
            tracing::warn!("контекст модели переполнен; компакция появится в task 7");
        }
        self.emit(Event::Error {
            message: error.to_string(),
        });
        StopReason::Error
    }

    /// Дописать сообщение в историю и в сессию. История append-only: сообщения
    /// только добавляются.
    fn push_message(&mut self, message: Message) {
        self.record(SessionItem::message(message.clone()));
        self.history.push(message);
    }

    /// Записать элемент сессии, если запись включена.
    fn record(&self, item: SessionItem) {
        if let Some(recorder) = &self.recorder {
            recorder.record(item);
        }
    }

    /// Отдать событие наружу. Закрытый приёмник — не повод ронять ход.
    fn emit(&self, event: Event) {
        if self.events.send(event).is_err() {
            tracing::debug!("приёмник событий закрыт, событие потеряно");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Role;
    use crate::provider::MessageStream;
    use crate::session::{SessionMeta, SessionRecorder, load_session};
    use crate::tools::EditStrategy;
    use async_trait::async_trait;
    use serde_json::json;
    use std::path::Path;
    use std::sync::Mutex;
    use std::time::Duration;

    /// Тесты не должны висеть, если цикл сломан.
    const EVENT_TIMEOUT: Duration = Duration::from_secs(10);

    // ─────────────────────────── MockProvider ───────────────────────────

    /// Сценарий одного обращения к модели.
    #[derive(Debug, Clone)]
    enum Script {
        /// Отдать чанки и закрыть поток.
        Chunks(Vec<StreamChunk>),
        /// Отдать чанки и зависнуть — «модель думает» бесконечно.
        Hang(Vec<StreamChunk>),
        /// Провалиться до начала стрима.
        Fail(String),
    }

    /// Провайдер со скриптованными ответами. Последний сценарий повторяется,
    /// пока его просят: так тестируется `max_turns`.
    struct MockProvider {
        scripts: Mutex<VecDeque<Script>>,
        requests: Mutex<Vec<ProviderRequest>>,
    }

    impl MockProvider {
        fn new(scripts: Vec<Script>) -> Arc<Self> {
            Arc::new(Self {
                scripts: Mutex::new(scripts.into()),
                requests: Mutex::new(Vec::new()),
            })
        }

        fn requests(&self) -> Vec<ProviderRequest> {
            self.requests.lock().unwrap().clone()
        }

        fn calls(&self) -> usize {
            self.requests.lock().unwrap().len()
        }
    }

    #[async_trait]
    impl Provider for MockProvider {
        fn name(&self) -> &str {
            "mock"
        }

        async fn stream(&self, req: ProviderRequest) -> Result<MessageStream, ProviderError> {
            self.requests.lock().unwrap().push(req);
            let script = {
                let mut scripts = self.scripts.lock().unwrap();
                if scripts.len() > 1 {
                    scripts.pop_front().expect("очередь непуста")
                } else {
                    scripts.front().cloned().expect("сценарий не задан")
                }
            };
            match script {
                Script::Chunks(chunks) => {
                    Ok(Box::pin(futures::stream::iter(chunks.into_iter().map(Ok))))
                }
                Script::Hang(chunks) => Ok(Box::pin(
                    futures::stream::iter(chunks.into_iter().map(Ok))
                        .chain(futures::stream::pending()),
                )),
                Script::Fail(message) => Err(ProviderError::Network(message)),
            }
        }
    }

    /// Ответ одним текстом, без тулов.
    fn text_script(text: &str) -> Script {
        Script::Chunks(vec![
            StreamChunk::TextDelta(text.to_string()),
            StreamChunk::Complete {
                message: Message::assistant(text),
                usage: Usage::new(10, 5),
            },
        ])
    }

    /// Ответ с тул-коллами (и, возможно, текстом перед ними).
    fn tools_script(text: &str, calls: &[(&str, &str, Value)]) -> Script {
        let mut blocks = Vec::new();
        let mut chunks = Vec::new();
        if !text.is_empty() {
            blocks.push(ContentBlock::text(text));
            chunks.push(StreamChunk::TextDelta(text.to_string()));
        }
        for (id, name, input) in calls {
            blocks.push(ContentBlock::tool_use(*id, *name, input.clone()));
        }
        chunks.push(StreamChunk::Complete {
            message: Message::new(Role::Assistant, blocks),
            usage: Usage::new(20, 8),
        });
        Script::Chunks(chunks)
    }

    // ──────────────────────────── обвязка ────────────────────────────

    /// Запущенный агент со скриптованным провайдером и записью сессии.
    struct Harness {
        handle: AgentHandle,
        events: UnboundedReceiver<Event>,
        provider: Arc<MockProvider>,
        dir: tempfile::TempDir,
        session: PathBuf,
    }

    impl Harness {
        async fn new(scripts: Vec<Script>, policy: PermissionPolicy, max_turns: usize) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let provider = MockProvider::new(scripts);
            let recorder = SessionRecorder::create(
                &dir.path().join("sessions"),
                SessionMeta::new(dir.path().to_string_lossy(), "mock-model", "mock"),
            )
            .await
            .unwrap();
            let session = recorder.path().to_path_buf();

            let config = AgentConfig::new("mock-model")
                .with_cwd(dir.path())
                .with_policy(policy)
                .with_max_turns(max_turns);
            let (handle, events) = Agent::new(
                provider.clone(),
                ToolRegistry::with_defaults(EditStrategy::SearchReplace),
                config,
            )
            // Промпт фиксируем: тесты не должны зависеть от AGENTS.md выше по дереву.
            .with_system_prompt("тестовый системный промпт")
            .with_recorder(recorder)
            .spawn();

            Self {
                handle,
                events,
                provider,
                dir,
                session,
            }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.dir.path().join(name)
        }

        fn submit(&self, op: Op) {
            self.handle.submit(op);
        }

        async fn next_event(&mut self) -> Event {
            tokio::time::timeout(EVENT_TIMEOUT, self.events.recv())
                .await
                .expect("таймаут ожидания события")
                .expect("канал событий закрыт")
        }

        /// Собрать события до конца хода включительно.
        async fn drain_turn(&mut self) -> Vec<Event> {
            let mut collected = Vec::new();
            loop {
                let event = self.next_event().await;
                let finished = matches!(event, Event::TurnFinished { .. });
                collected.push(event);
                if finished {
                    return collected;
                }
            }
        }

        /// Прогнать ход целиком: отправить ввод и дождаться `TurnFinished`.
        async fn turn(&mut self, text: &str) -> Vec<Event> {
            self.submit(Op::user_input(text));
            self.drain_turn().await
        }

        /// Остановить агента и прочитать записанную сессию.
        async fn finish(self) -> Vec<SessionItem> {
            let Self {
                handle,
                session,
                dir,
                ..
            } = self;
            handle.shutdown().await;
            let loaded = load_session(&session).await.unwrap();
            assert_eq!(loaded.skipped, 0, "сессия повреждена");
            drop(dir);
            loaded.lines.into_iter().map(|line| line.item).collect()
        }
    }

    fn tags(events: &[Event]) -> Vec<String> {
        events
            .iter()
            .map(|event| {
                serde_json::to_value(event).unwrap()["type"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    }

    fn stop_reason(events: &[Event]) -> StopReason {
        events
            .iter()
            .find_map(|event| match event {
                Event::TurnFinished { reason } => Some(*reason),
                _ => None,
            })
            .expect("ход не завершился")
    }

    /// Все `ToolCallFinished` как `(id, content, is_error)`.
    fn tool_results(events: &[Event]) -> Vec<(String, String, bool)> {
        events
            .iter()
            .filter_map(|event| match event {
                Event::ToolCallFinished {
                    id,
                    content,
                    is_error,
                } => Some((id.clone(), content.clone(), *is_error)),
                _ => None,
            })
            .collect()
    }

    /// Сообщения истории в порядке записи.
    fn messages(items: &[SessionItem]) -> Vec<Message> {
        items
            .iter()
            .filter_map(|item| match item {
                SessionItem::Message { message } => Some(message.clone()),
                _ => None,
            })
            .collect()
    }

    fn approval_id(events: &[Event]) -> String {
        events
            .iter()
            .find_map(|event| match event {
                Event::ApprovalRequired { id, .. } => Some(id.clone()),
                _ => None,
            })
            .expect("аппрув не запрашивался")
    }

    // ───────────────────────────── тесты ─────────────────────────────

    /// Полный ход: модель зовёт два тула подряд, получает результаты одним
    /// сообщением и отвечает текстом.
    #[tokio::test]
    async fn full_turn_with_two_tool_calls_then_final_answer() {
        let mut harness = Harness::new(
            vec![
                tools_script(
                    "смотрю",
                    &[
                        ("call_1", "read", json!({"path": "a.txt"})),
                        (
                            "call_2",
                            "write",
                            json!({"path": "b.txt", "content": "копия hello"}),
                        ),
                    ],
                ),
                text_script("готово"),
            ],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
        )
        .await;
        std::fs::write(harness.path("a.txt"), "hello\n").unwrap();

        let events = harness.turn("скопируй a.txt в b.txt").await;

        assert_eq!(
            tags(&events),
            [
                "turn_started",
                "text_delta",
                "message_complete",
                "usage",
                "tool_call_started",
                "tool_call_finished",
                "tool_call_started",
                "tool_call_finished",
                "text_delta",
                "message_complete",
                "usage",
                "turn_finished",
            ]
        );
        assert_eq!(stop_reason(&events), StopReason::Completed);

        let results = tool_results(&events);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "call_1");
        assert!(results[0].1.contains("hello"), "{}", results[0].1);
        assert!(!results[0].2);
        assert_eq!(results[1].0, "call_2");
        assert!(!results[1].2, "{}", results[1].1);
        assert_eq!(
            std::fs::read_to_string(harness.path("b.txt")).unwrap(),
            "копия hello"
        );

        // Второй запрос к модели видит ровно append-only историю.
        let requests = harness.provider.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].model, "mock-model");
        assert_eq!(requests[0].system, "тестовый системный промпт");
        assert_eq!(
            requests[0].messages,
            vec![Message::user("скопируй a.txt в b.txt")]
        );
        assert_eq!(
            requests[0]
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["bash", "edit", "read", "write"]
        );
        assert_eq!(requests[1].messages.len(), 3);
        assert_eq!(requests[1].messages[0].role, Role::User);
        assert_eq!(requests[1].messages[1].role, Role::Assistant);
        assert_eq!(requests[1].messages[2].role, Role::Tool);

        // Сессия: маркеры хода и каждое сообщение по порядку.
        let items = harness.finish().await;
        assert!(matches!(items[0], SessionItem::Meta { .. }));
        assert_eq!(items[1], SessionItem::turn_started(turn_id(&items)));
        assert_eq!(
            items.last().unwrap(),
            &SessionItem::turn_finished(turn_id(&items), StopReason::Completed)
        );

        let history = messages(&items);
        assert_eq!(history.len(), 4);
        assert_eq!(history[0], Message::user("скопируй a.txt в b.txt"));
        assert_eq!(history[1].role, Role::Assistant);
        assert_eq!(history[1].tool_uses().count(), 2);
        assert_eq!(history[2].role, Role::Tool);
        assert_eq!(
            history[2].content.len(),
            2,
            "оба результата одним сообщением"
        );
        assert_eq!(history[3], Message::assistant("готово"));
    }

    fn turn_id(items: &[SessionItem]) -> &str {
        items
            .iter()
            .find_map(|item| match item {
                SessionItem::TurnStarted { turn_id } => Some(turn_id.as_str()),
                _ => None,
            })
            .expect("нет маркера начала хода")
    }

    /// Прерывание посреди стрима завершает ход, но не ломает агента.
    #[tokio::test]
    async fn interrupt_mid_stream_cancels_the_turn_and_the_loop_survives() {
        let mut harness = Harness::new(
            vec![
                Script::Hang(vec![StreamChunk::TextDelta("думаю".into())]),
                text_script("продолжаю"),
            ],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
        )
        .await;

        harness.submit(Op::user_input("длинная задача"));
        assert!(matches!(
            harness.next_event().await,
            Event::TurnStarted { .. }
        ));
        assert_eq!(
            harness.next_event().await,
            Event::TextDelta {
                text: "думаю".into()
            }
        );

        harness.submit(Op::Interrupt);
        let rest = harness.drain_turn().await;
        assert_eq!(stop_reason(&rest), StopReason::Cancelled);

        // Следующий ход идёт как ни в чём не бывало.
        let events = harness.turn("а теперь просто ответь").await;
        assert_eq!(stop_reason(&events), StopReason::Completed);

        // Прерванный ход не оставил в истории ответа модели.
        let items = harness.finish().await;
        let history = messages(&items);
        assert_eq!(history.len(), 3);
        assert_eq!(history[0], Message::user("длинная задача"));
        assert_eq!(history[1], Message::user("а теперь просто ответь"));
        assert_eq!(history[2], Message::assistant("продолжаю"));
    }

    /// Внешняя отмена через корневой токен — тот же путь, что `Op::Interrupt`.
    #[tokio::test]
    async fn cancel_token_stops_the_running_turn() {
        let mut harness = Harness::new(
            vec![Script::Hang(vec![StreamChunk::TextDelta("думаю".into())])],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
        )
        .await;

        harness.submit(Op::user_input("подвисшая задача"));
        assert!(matches!(
            harness.next_event().await,
            Event::TurnStarted { .. }
        ));
        assert!(matches!(
            harness.next_event().await,
            Event::TextDelta { .. }
        ));

        harness.handle.cancel_token().cancel();
        let rest = harness.drain_turn().await;
        assert_eq!(stop_reason(&rest), StopReason::Cancelled);
        harness.finish().await;
    }

    /// Ошибка провайдера завершает ход, но агент остаётся жив.
    #[tokio::test]
    async fn provider_error_finishes_the_turn_and_the_loop_survives() {
        let mut harness = Harness::new(
            vec![
                Script::Fail("connection refused".into()),
                text_script("теперь работает"),
            ],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
        )
        .await;

        let events = harness.turn("сходи к модели").await;
        assert_eq!(tags(&events), ["turn_started", "error", "turn_finished"]);
        assert_eq!(stop_reason(&events), StopReason::Error);
        assert!(matches!(
            &events[1],
            Event::Error { message } if message.contains("connection refused")
        ));

        let events = harness.turn("ещё раз").await;
        assert_eq!(stop_reason(&events), StopReason::Completed);
        harness.finish().await;
    }

    /// `AskWrites`: `write` спрашивает, отказ уходит модели как ошибка тула.
    #[tokio::test]
    async fn ask_writes_denied_by_user_returns_an_error_result() {
        let mut harness = Harness::new(
            vec![
                tools_script(
                    "",
                    &[(
                        "call_1",
                        "write",
                        json!({"path": "secret.txt", "content": "нет"}),
                    )],
                ),
                text_script("понял, не пишу"),
            ],
            PermissionPolicy::AskWrites,
            DEFAULT_MAX_TURNS,
        )
        .await;

        harness.submit(Op::user_input("создай secret.txt"));
        let mut events = Vec::new();
        loop {
            let event = harness.next_event().await;
            let is_approval = matches!(event, Event::ApprovalRequired { .. });
            events.push(event);
            if is_approval {
                break;
            }
        }
        assert_eq!(
            tags(&events),
            [
                "turn_started",
                "message_complete",
                "usage",
                "approval_required"
            ]
        );

        let id = approval_id(&events);
        harness.submit(Op::deny(&id));
        let rest = harness.drain_turn().await;
        assert_eq!(stop_reason(&rest), StopReason::Completed);

        let results = tool_results(&rest);
        assert_eq!(results.len(), 1);
        assert!(results[0].2, "отказ — ошибка тула");
        assert!(
            results[0].1.starts_with("Denied by user:"),
            "{}",
            results[0].1
        );
        assert!(
            !harness.path("secret.txt").exists(),
            "файл не должен появиться"
        );
        // Тул не запускался.
        assert!(!tags(&rest).contains(&"tool_call_started".to_string()));

        // Модель получила отказ обычным tool result.
        let requests = harness.provider.requests();
        let last = requests.last().unwrap().messages.last().unwrap();
        assert_eq!(last.role, Role::Tool);
        assert_eq!(
            last.content,
            vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".into(),
                content: permission::user_denial("write"),
                is_error: true,
            }]
        );
        harness.finish().await;
    }

    /// `AskWrites`: разрешённый `write` исполняется.
    #[tokio::test]
    async fn ask_writes_allowed_by_user_executes_the_tool() {
        let mut harness = Harness::new(
            vec![
                tools_script(
                    "",
                    &[(
                        "call_1",
                        "write",
                        json!({"path": "out.txt", "content": "данные"}),
                    )],
                ),
                text_script("записал"),
            ],
            PermissionPolicy::AskWrites,
            DEFAULT_MAX_TURNS,
        )
        .await;

        harness.submit(Op::user_input("создай out.txt"));
        let mut events = Vec::new();
        loop {
            let event = harness.next_event().await;
            let is_approval = matches!(event, Event::ApprovalRequired { .. });
            events.push(event);
            if is_approval {
                break;
            }
        }

        harness.submit(Op::allow(approval_id(&events)));
        let rest = harness.drain_turn().await;
        assert_eq!(stop_reason(&rest), StopReason::Completed);
        assert_eq!(
            tags(&rest),
            [
                "tool_call_started",
                "tool_call_finished",
                "text_delta",
                "message_complete",
                "usage",
                "turn_finished",
            ]
        );
        assert!(!tool_results(&rest)[0].2);
        assert_eq!(
            std::fs::read_to_string(harness.path("out.txt")).unwrap(),
            "данные"
        );
        harness.finish().await;
    }

    /// Прерывание во время ожидания аппрува: ход отменён, но у `tool_use`
    /// всё равно есть парный результат.
    #[tokio::test]
    async fn interrupt_while_waiting_for_approval_still_pairs_the_tool_call() {
        let mut harness = Harness::new(
            vec![tools_script(
                "",
                &[(
                    "call_1",
                    "write",
                    json!({"path": "out.txt", "content": "данные"}),
                )],
            )],
            PermissionPolicy::AskWrites,
            DEFAULT_MAX_TURNS,
        )
        .await;

        harness.submit(Op::user_input("создай out.txt"));
        loop {
            if matches!(harness.next_event().await, Event::ApprovalRequired { .. }) {
                break;
            }
        }
        harness.submit(Op::Interrupt);
        let rest = harness.drain_turn().await;
        assert_eq!(stop_reason(&rest), StopReason::Cancelled);
        assert!(!harness.path("out.txt").exists());

        let items = harness.finish().await;
        let history = messages(&items);
        let last = history.last().unwrap();
        assert_eq!(last.role, Role::Tool);
        assert_eq!(
            last.content,
            vec![ContentBlock::tool_error(
                "call_1",
                permission::interrupted_result()
            )]
        );
    }

    /// `ReadOnly` отклоняет `write` молча — без вопроса пользователю.
    #[tokio::test]
    async fn read_only_denies_write_without_asking() {
        let mut harness = Harness::new(
            vec![
                tools_script(
                    "",
                    &[(
                        "call_1",
                        "write",
                        json!({"path": "out.txt", "content": "данные"}),
                    )],
                ),
                text_script("не могу писать"),
            ],
            PermissionPolicy::ReadOnly,
            DEFAULT_MAX_TURNS,
        )
        .await;

        let events = harness.turn("создай out.txt").await;
        assert_eq!(
            tags(&events),
            [
                "turn_started",
                "message_complete",
                "usage",
                "tool_call_finished",
                "text_delta",
                "message_complete",
                "usage",
                "turn_finished",
            ],
            "ни approval_required, ни tool_call_started"
        );
        assert_eq!(stop_reason(&events), StopReason::Completed);

        let results = tool_results(&events);
        assert!(results[0].2);
        assert!(results[0].1.contains("read-only"), "{}", results[0].1);
        assert!(!harness.path("out.txt").exists());
        harness.finish().await;
    }

    /// `read` в `ReadOnly` не блокируется.
    #[tokio::test]
    async fn read_only_still_allows_reading() {
        let mut harness = Harness::new(
            vec![
                tools_script("", &[("call_1", "read", json!({"path": "a.txt"}))]),
                text_script("прочитал"),
            ],
            PermissionPolicy::ReadOnly,
            DEFAULT_MAX_TURNS,
        )
        .await;
        std::fs::write(harness.path("a.txt"), "содержимое\n").unwrap();

        let events = harness.turn("прочитай a.txt").await;
        assert_eq!(stop_reason(&events), StopReason::Completed);
        let results = tool_results(&events);
        assert!(!results[0].2, "{}", results[0].1);
        assert!(results[0].1.contains("содержимое"));
        harness.finish().await;
    }

    /// Неразобранные аргументы (маркер провайдера) отклоняются валидацией, а
    /// модель чинится вторым запросом.
    #[tokio::test]
    async fn unparsed_arguments_come_back_as_an_error_and_the_model_retries() {
        let mut harness = Harness::new(
            vec![
                // Ровно то, что кладёт openai_compat, когда аргументы не
                // разобрались ни строгим парсом, ни json_repair.
                tools_script(
                    "",
                    &[(
                        "call_1",
                        "write",
                        json!({
                            "__mark2_unparsed": true,
                            "__mark2_raw_arguments": "{\"path\": \"out.txt\", \"conte",
                        }),
                    )],
                ),
                tools_script(
                    "",
                    &[(
                        "call_2",
                        "write",
                        json!({"path": "out.txt", "content": "исправился"}),
                    )],
                ),
                text_script("готово"),
            ],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
        )
        .await;

        let events = harness.turn("создай out.txt").await;
        assert_eq!(stop_reason(&events), StopReason::Completed);

        let results = tool_results(&events);
        assert_eq!(results.len(), 2);
        assert!(results[0].2, "невалидные аргументы — ошибка");
        assert!(
            results[0]
                .1
                .starts_with("Invalid arguments for tool `write`:"),
            "{}",
            results[0].1
        );
        assert!(results[0].1.contains("path"), "{}", results[0].1);
        assert!(!results[1].2, "{}", results[1].1);
        assert_eq!(
            std::fs::read_to_string(harness.path("out.txt")).unwrap(),
            "исправился"
        );
        assert_eq!(harness.provider.calls(), 3);
        harness.finish().await;
    }

    /// Невалидные аргументы известного тула — тоже данные для модели.
    #[tokio::test]
    async fn invalid_arguments_are_reported_with_the_schema() {
        let mut harness = Harness::new(
            vec![
                tools_script("", &[("call_1", "read", json!({"path": 42}))]),
                text_script("понял"),
            ],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
        )
        .await;

        let events = harness.turn("прочитай что-нибудь").await;
        let results = tool_results(&events);
        assert!(results[0].2);
        assert!(
            results[0]
                .1
                .contains("Expected arguments matching this JSON Schema:"),
            "{}",
            results[0].1
        );
        harness.finish().await;
    }

    /// Модель, зациклившаяся на тулах, останавливается предохранителем.
    #[tokio::test]
    async fn max_turns_stops_a_tool_loop() {
        let mut harness = Harness::new(
            vec![tools_script(
                "",
                &[("call_1", "read", json!({"path": "a.txt"}))],
            )],
            PermissionPolicy::Yolo,
            2,
        )
        .await;
        std::fs::write(harness.path("a.txt"), "hello\n").unwrap();

        let events = harness.turn("читай в цикле").await;
        assert_eq!(stop_reason(&events), StopReason::MaxTurns);
        assert_eq!(harness.provider.calls(), 2, "ровно max_turns обращений");
        assert_eq!(
            tags(&events)
                .iter()
                .filter(|tag| *tag == "message_complete")
                .count(),
            2
        );
        harness.finish().await;
    }

    /// Неизвестный тул не роняет ход: модель получает список доступных.
    #[tokio::test]
    async fn unknown_tool_is_reported_to_the_model() {
        let mut harness = Harness::new(
            vec![
                tools_script("", &[("call_1", "browse", json!({"url": "example.com"}))]),
                text_script("понял"),
            ],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
        )
        .await;

        let events = harness.turn("сходи в интернет").await;
        assert_eq!(stop_reason(&events), StopReason::Completed);
        let results = tool_results(&events);
        assert!(results[0].2);
        assert!(
            results[0].1.contains("Unknown tool `browse`"),
            "{}",
            results[0].1
        );
        harness.finish().await;
    }

    /// Сообщения, скрытые от модели, не уходят в запрос, но остаются в истории.
    #[tokio::test]
    async fn agent_invisible_messages_are_filtered_out_of_the_request() {
        let provider = MockProvider::new(vec![text_script("ок")]);
        let dir = tempfile::tempdir().unwrap();
        let (handle, mut events) = Agent::new(
            provider.clone(),
            ToolRegistry::new(),
            AgentConfig::new("mock-model").with_cwd(dir.path()),
        )
        .with_system_prompt("s")
        .spawn();

        handle.submit(Op::user_input("привет"));
        loop {
            let event = tokio::time::timeout(EVENT_TIMEOUT, events.recv())
                .await
                .unwrap()
                .unwrap();
            if matches!(event, Event::TurnFinished { .. }) {
                break;
            }
        }
        handle.shutdown().await;

        let request = provider.requests().pop().unwrap();
        assert_eq!(request.messages, vec![Message::user("привет")]);
        assert!(
            request.tools.is_empty(),
            "реестр пуст — тулов в запросе нет"
        );
    }

    /// Системный промпт по умолчанию — из [`prompt`], вместе с `AGENTS.md`.
    #[tokio::test]
    async fn default_system_prompt_includes_project_instructions() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(prompt::AGENTS_MD), "правила проекта").unwrap();

        let agent = Agent::new(
            MockProvider::new(vec![text_script("ок")]),
            ToolRegistry::with_defaults(EditStrategy::SearchReplace),
            AgentConfig::new("mock-model").with_cwd(dir.path()),
        );
        assert!(agent.system_prompt().starts_with("You are mark2"));
        assert!(agent.system_prompt().contains("правила проекта"));
        assert_eq!(agent.config().max_turns, DEFAULT_MAX_TURNS);
        assert_eq!(agent.config().policy, PermissionPolicy::AskWrites);
        assert!(format!("{agent:?}").contains("mock"));
    }

    #[test]
    fn config_defaults_and_builders() {
        let config = AgentConfig::default();
        assert_eq!(config.max_turns, DEFAULT_MAX_TURNS);
        assert_eq!(config.policy, PermissionPolicy::AskWrites);
        assert!(config.max_tokens.is_none());
        assert!(config.temperature.is_none());

        let config = AgentConfig::new("qwen3-coder")
            .with_cwd(Path::new("/work"))
            .with_policy(PermissionPolicy::Yolo)
            .with_max_turns(7)
            .with_max_tokens(2048)
            .with_temperature(0.2);
        assert_eq!(config.model, "qwen3-coder");
        assert_eq!(config.cwd, PathBuf::from("/work"));
        assert_eq!(config.policy, PermissionPolicy::Yolo);
        assert_eq!(config.max_turns, 7);
        assert_eq!(config.max_tokens, Some(2048));
        assert_eq!(config.temperature, Some(0.2));
    }

    /// Настройки сэмплинга доезжают до провайдера.
    #[tokio::test]
    async fn sampling_settings_reach_the_provider() {
        let provider = MockProvider::new(vec![text_script("ок")]);
        let dir = tempfile::tempdir().unwrap();
        let (handle, mut events) = Agent::new(
            provider.clone(),
            ToolRegistry::new(),
            AgentConfig::new("mock-model")
                .with_cwd(dir.path())
                .with_max_tokens(512)
                .with_temperature(0.1),
        )
        .with_system_prompt("s")
        .spawn();

        handle.submit(Op::user_input("привет"));
        while let Some(event) = tokio::time::timeout(EVENT_TIMEOUT, events.recv())
            .await
            .unwrap()
        {
            if matches!(event, Event::TurnFinished { .. }) {
                break;
            }
        }
        handle.shutdown().await;

        let request = provider.requests().pop().unwrap();
        assert_eq!(request.max_tokens, Some(512));
        assert_eq!(request.temperature, Some(0.1));
    }
}
