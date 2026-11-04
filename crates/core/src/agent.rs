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
//! Предохранители три:
//!
//! * `max_turns` итераций сэмплинга на ход (`TurnFinished{MaxTurns}`);
//! * **детектор зацикливания** (`loop_detection`): N-й подряд идентичный
//!   тул-колл (то же имя, тот же сериализованный `input`) не исполняется —
//!   модель получает `is_error`-результат «не повторяй, ответь текстом»; на
//!   2×N-м ход завершается `TurnFinished{Error}` с [`Event::Error`]. Слабые
//!   локалки умеют найти ответ и затем звать один и тот же `echo` до
//!   `max_turns`, сжигая весь контекст на каждой итерации;
//! * **сенсор write-churn** (`write_churn`): N-я подряд успешная запись
//!   (`write`/`edit`, в любой смеси) в один и тот же путь без единого
//!   исполненного `bash` между записями исполняется как обычно, но к её
//!   результату приложена подсказка «запусти и прочитай реальную ошибку,
//!   прежде чем переписывать»; с 2×N-й подсказка жёсткая («смени стратегию»).
//!   Ход не останавливается никогда: переписать файл — законная работа. Так
//!   «мечутся» модели на проектных задачах: файл переписывается целиком раз
//!   за разом с чуть другим содержимым и ни разу не запускается — вызовы не
//!   идентичны, детектор зацикливания их не видит.
//!
//! Счётчики у детектора и сенсора свои и друг друга не трогают. Срабатывание
//! любого из них видно наружу как [`Event::Nudge`] (CLI печатает его в stderr
//! маркером `[agent] <guard>`) сразу после `ToolCallFinished` того вызова, к
//! которому относится подсказка.
//!
//! # Инварианты
//!
//! * **История append-only.** Сообщения только дописываются, никогда не
//!   переписываются: префикс промпта обязан быть стабильным, иначе KV-кэш
//!   локального сервера инвалидируется на каждом запросе. Фильтр сборки
//!   запроса — `agent_visible`; компакция ничего не удаляет, а лишь снимает
//!   этот флаг (см. [`crate::compact`]).
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
//! # Компакция
//!
//! Четыре входа в [`crate::compact::compact`]:
//!
//! * **перед ходом**, до того как ввод пользователя попадёт в историю: если
//!   оценка занятости перевалила `threshold × context_limit`
//!   ([`crate::compact::CompactionConfig`]). Порядок важен — свежая просьба
//!   пользователя обязана пережить сжатие видимой;
//! * **между итерациями** хода — тот же порог перед каждой итерацией, кроме
//!   первой, когда результаты прошлого раунда тулов уже в истории. Без этой
//!   точки порог не видели бы однократный `-p` и сервера, которые на
//!   переполнение не отказывают, а молча режут промпт (ollama);
//! * **реактивно**, на [`ProviderError::ContextLengthExceeded`] — как до
//!   стрима, так и error-фреймом внутри него: сжать и повторить запрос
//!   **ровно один раз**. Компакция в итерации одна: переполнение после неё
//!   (реактивной или проактивной) и переполнение на самой суммаризации
//!   завершают ход `Error`;
//! * **вручную**, по [`Op::Compact`] вне хода.
//!
//! Шторм-защита порога: после любой компакции он молчит, пока провайдер не
//! пришлёт свежий [`Usage`]. Оценка без него — грубая байтовая, и даже если
//! сразу после сжатия она выше порога, цикл идёт сэмплить, а не пересжимает
//! саммари; страхует реактивная ветка.
//!
//! Наружу это видно как [`Event::Compacted`], в сессию — как
//! [`SessionItem::Compacted`], скрытые копии сообщений и саммари.
//!
//! # Что ядро НЕ делает в MVP
//!
//! `StreamChunk::ToolCallDelta` наружу не транслируется (аргументы всё равно
//! показываются целиком в `ToolCallStarted`), а тул-коллы исполняются строго
//! последовательно.

use std::collections::VecDeque;
use std::future::Future;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use futures::StreamExt;
use serde_json::Value;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::compact::{self, CompactionConfig, CompactionError};
use crate::event::{ApprovalDecision, Event, Guard, Op, StopReason};
use crate::memory::{self, Memory};
use crate::message::{ContentBlock, Message, Usage};
use crate::permission::{self, PermissionCheck, PermissionPolicy};
use crate::prompt;
use crate::provider::{Provider, ProviderError, ProviderRequest, StreamChunk, ToolDef};
use crate::session::{SessionItem, SessionRecorder};
use crate::tools::{ToolCtx, ToolRegistry};

/// Сколько раз за один ход агент готов сходить к модели, если не задано иное.
pub const DEFAULT_MAX_TURNS: usize = 50;

/// Порог детектора зацикливания по умолчанию: третий подряд идентичный
/// тул-колл не исполняется.
pub const DEFAULT_LOOP_DETECTION: usize = 3;

/// Порог сенсора write-churn по умолчанию: третья подряд запись в один файл
/// без запуска между записями получает мягкую подсказку, шестая — жёсткую.
pub const DEFAULT_WRITE_CHURN: usize = 3;

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
    /// Рабочая директория: от неё резолвятся пути тулов и от неё вверх ищутся
    /// `AGENTS.md` и `.mark2.memory.md`.
    pub cwd: PathBuf,
    /// Когда и как сжимать историю.
    pub compaction: CompactionConfig,
    /// Детектор зацикливания: с какого подряд идентичного тул-колла перестать
    /// его исполнять. На удвоенном пороге ход завершается ошибкой. `0` —
    /// детектор выключен; `1` равносилен `2` (иначе блокировался бы любой
    /// первый вызов).
    pub loop_detection: usize,
    /// Сенсор write-churn: с какой подряд записи (`write`/`edit`) в один и тот
    /// же путь без единого `bash` между ними прикладывать к результату записи
    /// подсказку «запусти, прежде чем переписывать»; с удвоенного порога она
    /// жёсткая. Запись не блокируется никогда. `0` — сенсор выключен (в TOML
    /// это `write_churn = false`, `true` — порог по умолчанию); `1` равносилен
    /// `2` (иначе подсказку получала бы любая первая запись).
    pub write_churn: usize,
    /// Файл памяти вместо ближайшего `.mark2.memory.md` от `cwd`
    /// ([`crate::memory`]); относительный — от `cwd`. Файла нет — памяти нет.
    pub memory_file: Option<PathBuf>,
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
            compaction: CompactionConfig::default(),
            loop_detection: DEFAULT_LOOP_DETECTION,
            write_churn: DEFAULT_WRITE_CHURN,
            memory_file: None,
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

    /// Задать настройки компакции.
    #[must_use]
    pub fn with_compaction(mut self, compaction: CompactionConfig) -> Self {
        self.compaction = compaction;
        self
    }

    /// Задать порог детектора зацикливания (`0` — выключить).
    #[must_use]
    pub fn with_loop_detection(mut self, threshold: usize) -> Self {
        self.loop_detection = threshold;
        self
    }

    /// Задать порог сенсора write-churn (`0` — выключить).
    #[must_use]
    pub fn with_write_churn(mut self, threshold: usize) -> Self {
        self.write_churn = threshold;
        self
    }

    /// Задать файл памяти явно, вместо поиска `.mark2.memory.md` от `cwd`.
    #[must_use]
    pub fn with_memory_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.memory_file = Some(path.into());
        self
    }
}

/// Агент: провайдер + тулы + конфиг, ещё не запущенный.
///
/// Собирается синхронно (в том числе читает `AGENTS.md` и память),
/// запускается [`Agent::spawn`].
pub struct Agent {
    provider: Arc<dyn Provider>,
    registry: ToolRegistry,
    config: AgentConfig,
    recorder: Option<SessionRecorder>,
    system: String,
    /// Память, инъектированная в `system`; `None` — её там нет.
    memory: Option<Memory>,
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
            .field("memory", &self.memory.as_ref().map(|memory| &memory.path))
            .field("history_len", &self.history.len())
            .finish()
    }
}

impl Agent {
    /// Собрать агента. Системный промпт строится здесь один раз
    /// ([`prompt::system_prompt_with_memory`]: `AGENTS.md` рядом с `cwd` и
    /// память агента, [`crate::memory`]), список тулов фиксируется в порядке
    /// регистрации — оба обязаны быть стабильными между запросами. Поэтому и
    /// память читается один раз: её правки по ходу сессии попадут в промпт
    /// следующей.
    pub fn new(provider: Arc<dyn Provider>, registry: ToolRegistry, config: AgentConfig) -> Self {
        let memory = memory::load_memory(&config.cwd, config.memory_file.as_deref());
        let system = prompt::system_prompt_with_memory(&config.cwd, memory.as_ref());
        let tool_defs = registry.defs();
        Self {
            provider,
            registry,
            config,
            recorder: None,
            system,
            memory,
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
    /// Память в нём уже не инъектирована — [`Agent::memory`] становится `None`.
    #[must_use]
    pub fn with_system_prompt(mut self, system: impl Into<String>) -> Self {
        self.system = system.into();
        self.memory = None;
        self
    }

    /// Системный промпт, с которым агент пойдёт к модели.
    pub fn system_prompt(&self) -> &str {
        &self.system
    }

    /// Память, инъектированная в системный промпт (для шапки сессии и
    /// отладки); `None` — памяти нет или промпт заменён целиком.
    pub fn memory(&self) -> Option<&Memory> {
        self.memory.as_ref()
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
    ///
    /// Если память обрезана по капу, первым событием стрима — ещё до хода —
    /// идёт [`Event::Warning`]: промпт собран, и человек должен узнать, что
    /// модель видит память не целиком.
    pub fn spawn(self) -> (AgentHandle, UnboundedReceiver<Event>) {
        let (op_tx, op_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();

        if let Some(message) = self
            .memory
            .as_ref()
            .and_then(|memory| memory.truncation_warning(&self.config.cwd))
        {
            // Приёмник ещё у нас в руках — отправка не может не пройти.
            let _ = event_tx.send(Event::Warning { message });
        }

        let agent_loop = AgentLoop {
            provider: self.provider,
            registry: self.registry,
            config: self.config,
            recorder: self.recorder,
            system: self.system,
            tool_defs: self.tool_defs,
            history: self.history,
            last_usage: None,
            compacted_since_usage: false,
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

/// Что делать с очередным тул-коллом по мнению детектора зацикливания.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopVerdict {
    /// Исполнять как обычно.
    Run,
    /// Не исполнять: это `repeats`-й идентичный вызов подряд.
    Block { repeats: usize },
    /// Не исполнять и завершить ход: повторы дошли до жёсткого предела.
    Abort { repeats: usize },
}

/// Детектор подряд идущих идентичных тул-коллов.
///
/// Живёт один ход. Ключ вызова — имя плюс `input`, сериализованный в JSON
/// (порядок ключей сохраняет `preserve_order`). Любой другой вызов между
/// повторами — другой тул или другие аргументы — сбрасывает счётчик.
#[derive(Debug)]
struct LoopDetector {
    /// Порог блокировки; `0` — детектор выключен.
    threshold: usize,
    last: Option<String>,
    repeats: usize,
}

impl LoopDetector {
    fn new(threshold: usize) -> Self {
        Self {
            // Порог 1 блокировал бы любой первый вызов — минимум два.
            threshold: if threshold == 0 { 0 } else { threshold.max(2) },
            last: None,
            repeats: 0,
        }
    }

    /// Учесть вызов и вынести вердикт.
    fn observe(&mut self, name: &str, input: &Value) -> LoopVerdict {
        if self.threshold == 0 {
            return LoopVerdict::Run;
        }
        let key = format!(
            "{name}\u{0}{}",
            serde_json::to_string(input).unwrap_or_default()
        );
        if self.last.as_deref() == Some(key.as_str()) {
            self.repeats += 1;
        } else {
            self.last = Some(key);
            self.repeats = 1;
        }

        let repeats = self.repeats;
        if repeats >= 2 * self.threshold {
            LoopVerdict::Abort { repeats }
        } else if repeats >= self.threshold {
            LoopVerdict::Block { repeats }
        } else {
            LoopVerdict::Run
        }
    }
}

/// Tool result вместо исполнения зациклившегося вызова. Читает модель —
/// поэтому по-английски и с прямой инструкцией, что делать вместо повтора.
fn repeated_call_result(previous: usize) -> String {
    format!(
        "You have already made this exact tool call {previous} times in a row with the same \
         result. Do not repeat it. If you have the answer, reply with plain text instead of \
         calling tools."
    )
}

/// Результат для тул-коллов того же ответа, до которых ход не дошёл, потому
/// что детектор зацикливания его остановил.
const LOOP_ABORTED_RESULT: &str =
    "Not run: the turn was stopped because the same tool call kept repeating.";

/// Текст [`Event::Nudge`] детектора зацикливания — для человека.
fn loop_nudge_message(name: &str, repeats: usize) -> String {
    format!("{repeats}-й подряд одинаковый вызов `{name}` не исполнен")
}

/// Что приложить к результату исполненного вызова по мнению сенсора
/// write-churn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChurnVerdict {
    /// Ничего.
    Quiet,
    /// Мягкая подсказка: `writes`-я подряд запись в путь без запуска.
    Soft { writes: usize },
    /// Жёсткая подсказка: записей подряд уже не меньше удвоенного порога.
    Hard { writes: usize },
}

/// Сенсор «переписывает, не запуская»: подряд идущие записи в один и тот же
/// файл без единого `bash` между ними.
///
/// Живёт один ход. В отличие от [`LoopDetector`] ничего не блокирует —
/// переписать файл законно, да и вызовы не идентичны: содержимое каждый раз
/// чуть другое (его сенсор не сравнивает — важен сам факт «пишет, не
/// запуская»). Видит только **исполненные** вызовы, цикл сообщает о них после
/// исполнения:
///
/// * успешный `write`/`edit` — запись: тот же путь — счётчик растёт, другой —
///   отсчёт заново, уже для нового пути;
/// * `bash` — сброс: что бы ни запустили и чем бы оно ни кончилось, модель
///   увидела настоящий вывод;
/// * остальное нейтрально: `read` — не проверка (перечитать свой файл — не
///   то же, что запустить), упавшая запись — не перезапись.
///
/// Вызовы, заблокированные детектором зацикливания или политикой доступа, не
/// исполнялись и до сенсора не доходят. Ключ пути — [`write_target`]: `a.py`,
/// `./a.py` и абсолютный путь к тому же файлу — одна и та же запись.
#[derive(Debug)]
struct WriteChurnDetector {
    /// Порог мягкой подсказки; `0` — сенсор выключен.
    threshold: usize,
    path: Option<PathBuf>,
    writes: usize,
}

impl WriteChurnDetector {
    fn new(threshold: usize) -> Self {
        Self {
            // Порог 1 подсказывал бы на любой первой записи — минимум два.
            threshold: if threshold == 0 { 0 } else { threshold.max(2) },
            path: None,
            writes: 0,
        }
    }

    /// Учесть исполненный тул-колл и вынести вердикт.
    fn observe(&mut self, name: &str, input: &Value, is_error: bool, cwd: &Path) -> ChurnVerdict {
        if self.threshold == 0 {
            return ChurnVerdict::Quiet;
        }
        // Имена — из реестра по умолчанию (`tools/`), как и в `permission`.
        match name {
            "bash" => {
                self.path = None;
                self.writes = 0;
                ChurnVerdict::Quiet
            }
            "write" | "edit" if !is_error => {
                let Some(path) = write_target(input, cwd) else {
                    return ChurnVerdict::Quiet;
                };
                if self.path.as_ref() == Some(&path) {
                    self.writes += 1;
                } else {
                    self.path = Some(path);
                    self.writes = 1;
                }

                let writes = self.writes;
                if writes >= 2 * self.threshold {
                    ChurnVerdict::Hard { writes }
                } else if writes >= self.threshold {
                    ChurnVerdict::Soft { writes }
                } else {
                    ChurnVerdict::Quiet
                }
            }
            _ => ChurnVerdict::Quiet,
        }
    }
}

/// Файл, в который пишет `write`/`edit`: поле `path`, относительный — от
/// `cwd`, `.` и `..` свёрнуты лексически. Файловую систему сенсор не трогает
/// (симлинки не разворачиваются): CLI и так канонизирует `cwd`.
fn write_target(input: &Value, cwd: &Path) -> Option<PathBuf> {
    let raw = input.get("path")?.as_str()?;
    let mut target = PathBuf::new();
    for component in cwd.join(raw).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                target.pop();
            }
            other => target.push(other),
        }
    }
    Some(target)
}

/// Подсказка сенсора write-churn в двух адресах.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ChurnNudge {
    /// Модели: дописывается к результату записи.
    note: String,
    /// Человеку: текст [`Event::Nudge`].
    message: String,
}

/// Подсказка по вердикту сенсора; `None` — подсказывать нечего.
///
/// Модель читает текст по-английски и с путём в её же написании; человеку —
/// путь относительно `cwd`, если файл внутри него (в бенче `cwd` — длинный
/// временный путь, который ничего не говорит).
fn churn_nudge(verdict: ChurnVerdict, input: &Value, cwd: &Path) -> Option<ChurnNudge> {
    let (writes, hard) = match verdict {
        ChurnVerdict::Quiet => return None,
        ChurnVerdict::Soft { writes } => (writes, false),
        ChurnVerdict::Hard { writes } => (writes, true),
    };
    let path = input
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or("this file");
    let target = write_target(input, cwd).unwrap_or_else(|| PathBuf::from(path));
    let shown = target.strip_prefix(cwd).unwrap_or(&target).display();

    Some(if hard {
        ChurnNudge {
            note: format!(
                "Warning: {writes} writes to {path} in a row without a single run in between. \
                 Rewriting without running is not converging. Change strategy: run it now with \
                 bash and fix only the error it actually reports; make the file smaller and \
                 check fewer things at a time."
            ),
            message: format!(
                "{writes}-я запись подряд в {shown} без запуска между ними — жёсткая подсказка \
                 сменить стратегию"
            ),
        }
    } else {
        ChurnNudge {
            note: format!(
                "Note: this is write #{writes} to {path} in a row, and nothing has been run in \
                 between. Before rewriting it again, run it (or its test) with bash and read the \
                 actual error, or simplify the approach."
            ),
            message: format!(
                "{writes}-я запись подряд в {shown} без запуска между ними — подсказка запустить"
            ),
        }
    })
}

/// Исход исполнения тул-коллов одного ответа модели.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ToolRound {
    /// Все результаты в истории — идём к модели снова.
    Continue,
    /// Ход прерван пользователем.
    Interrupted,
    /// Детектор зацикливания упёрся в жёсткий предел.
    LoopAborted { name: String, repeats: usize },
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
    /// Счётчики последнего ответа провайдера — основной сигнал занятости
    /// контекста. Сбрасывается компакцией: старый `usage` описывает промпт,
    /// которого больше нет.
    last_usage: Option<Usage>,
    /// Шторм-защита порога: история уже сжата, а свежего `Usage` с тех пор
    /// провайдер не присылал. Пока флаг поднят, проактивная компакция молчит:
    /// оценка без `Usage` — грубая байтовая, и сжимать по ней саммари снова и
    /// снова — шторм суммаризаций вместо работы.
    compacted_since_usage: bool,
    events: UnboundedSender<Event>,
    ops: OpChannel,
    cancel: CancellationToken,
}

/// Исход проверки порога компакции.
enum Threshold {
    /// История не тронута: порог не пройден, сжатие ждёт свежего `Usage` или
    /// не удалось (ошибка уже отдана наружу).
    Kept,
    /// История сжата.
    Compacted,
    /// Компакция прервана ([`Op::Interrupt`] или отмена снаружи).
    Interrupted,
}

/// Исход компакции внутри цикла.
enum Compaction {
    /// История сжата; событие и записи сессии уже отданы.
    Done,
    /// Компакция прервана ([`Op::Interrupt`] или отмена снаружи).
    Interrupted,
    /// Сжать не удалось.
    Failed(CompactionError),
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
                Op::Compact => self.run_manual_compaction().await,
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

        // Проактивная компакция — до того, как ввод попадёт в историю: иначе
        // свежая просьба пользователя тут же уехала бы в скрытые.
        if let Threshold::Interrupted = self.compact_on_threshold(&turn_cancel).await {
            let reason = StopReason::Cancelled;
            self.emit(Event::TurnFinished { reason });
            self.record(SessionItem::turn_finished(turn_id.as_str(), reason));
            return;
        }

        self.push_message(Message::user(text));

        let reason = self.sample_loop(&turn_cancel).await;

        self.emit(Event::TurnFinished { reason });
        self.record(SessionItem::turn_finished(turn_id.as_str(), reason));
    }

    /// Ручная компакция вне хода ([`Op::Compact`]).
    async fn run_manual_compaction(&mut self) {
        if !self.history.iter().any(Message::is_agent_visible) {
            tracing::debug!("Op::Compact: в истории нет видимых модели сообщений");
            return;
        }

        let cancel = self.cancel.child_token();
        match self.compact_history(&cancel).await {
            Compaction::Done => {}
            Compaction::Interrupted => tracing::debug!("ручная компакция прервана"),
            Compaction::Failed(error) => {
                tracing::warn!(%error, "ручная компакция не удалась");
                self.emit(Event::Error {
                    message: error.to_string(),
                });
            }
        }
    }

    /// Пора ли сжимать по порогу: оценка занятости перевалила порог из конфига,
    /// и после прошлой компакции провайдер уже прислал свежий `Usage`.
    fn needs_compaction(&self) -> bool {
        !self.compacted_since_usage
            && self.history.iter().any(Message::is_agent_visible)
            && self
                .config
                .compaction
                .should_compact(compact::estimate_tokens(&self.history, self.last_usage))
    }

    /// Проактивная компакция по порогу — перед ходом и между итерациями.
    ///
    /// Неудача хода не валит: оценка могла ошибаться, а настоящее переполнение
    /// поймает реактивная ветка. Наружу она уходит [`Event::Error`].
    async fn compact_on_threshold(&mut self, turn_cancel: &CancellationToken) -> Threshold {
        if !self.needs_compaction() {
            return Threshold::Kept;
        }
        match self.compact_history(turn_cancel).await {
            Compaction::Done => Threshold::Compacted,
            Compaction::Interrupted => Threshold::Interrupted,
            Compaction::Failed(error) => {
                tracing::warn!(%error, "компакция по порогу не удалась");
                self.emit(Event::Error {
                    message: error.to_string(),
                });
                Threshold::Kept
            }
        }
    }

    /// Сжать историю: суммаризация через провайдера, скрытие старых сообщений,
    /// событие наружу и записи в сессию.
    ///
    /// Сессия остаётся append-only: маркер [`SessionItem::Compacted`], затем
    /// скрытые копии сообщений и саммари **дописываются** в JSONL — уже
    /// записанные строки не переписываются.
    async fn compact_history(&mut self, turn_cancel: &CancellationToken) -> Compaction {
        let provider = Arc::clone(&self.provider);
        let model = self.config.model.clone();
        let last_usage = self.last_usage;
        let summarize = compact::compact(
            provider.as_ref(),
            &model,
            compact::SUMMARY_SYSTEM_PROMPT,
            &mut self.history,
            last_usage,
        );

        let outcome = match race_ops(summarize, &mut self.ops, turn_cancel).await {
            Raced::Interrupted => return Compaction::Interrupted,
            Raced::Done(Ok(outcome)) => outcome,
            Raced::Done(Err(error)) => return Compaction::Failed(error),
        };

        self.last_usage = None;
        self.compacted_since_usage = true;
        self.emit(Event::Compacted {
            tokens_before: outcome.tokens_before,
            tokens_after: outcome.tokens_after,
        });
        self.record(SessionItem::Compacted {
            tokens_before: outcome.tokens_before,
            tokens_after: outcome.tokens_after,
        });
        for message in outcome.hidden {
            self.record(SessionItem::message(message));
        }
        self.record(SessionItem::message(outcome.summary));

        Compaction::Done
    }

    /// Цикл сэмплинга: запрос → дельты → тулы → снова запрос.
    async fn sample_loop(&mut self, turn_cancel: &CancellationToken) -> StopReason {
        let mut loops = LoopDetector::new(self.config.loop_detection);
        let mut churn = WriteChurnDetector::new(self.config.write_churn);
        for iteration in 0..self.config.max_turns {
            if turn_cancel.is_cancelled() {
                return StopReason::Cancelled;
            }

            // Компакция в итерации — одна: проактивная здесь или реактивная
            // ниже. Переполнение после неё валит ход, иначе цикл «сжали, снова
            // не влезло» не кончился бы никогда.
            let mut compacted = false;

            // Проактивная компакция между итерациями, до сборки запроса. Без неё
            // ход видел порог только на входе: в `-p` ход ровно один и история на
            // старте пуста, а ollama на переполнение не отказывает, а молча режет
            // начало промпта. Сжимать здесь безопасно: результаты прошлого раунда
            // уже в истории одним сообщением — у каждого `tool_use` есть пара, и
            // обе уходят в скрытые. Первую итерацию уже проверил `run_turn` — до
            // ввода пользователя, который сжимать рано: модель его ещё не видела.
            if iteration > 0 {
                match self.compact_on_threshold(turn_cancel).await {
                    Threshold::Kept => {}
                    Threshold::Compacted => compacted = true,
                    Threshold::Interrupted => return StopReason::Cancelled,
                }
            }

            // Один ответ модели. Переполнение контекста — что до стрима, что
            // внутри него (llama-server отдаёт его error-фреймом уже в SSE) —
            // лечится компакцией и ровно одним повтором.
            let (message, usage) = 'request: loop {
                let provider = Arc::clone(&self.provider);
                let request = self.build_request();
                let raced = race_ops(provider.stream(request), &mut self.ops, turn_cancel).await;
                let mut stream = match raced {
                    Raced::Interrupted => return StopReason::Cancelled,
                    Raced::Done(Ok(stream)) => stream,
                    Raced::Done(Err(ProviderError::ContextLengthExceeded)) if !compacted => {
                        compacted = true;
                        match self.compact_history(turn_cancel).await {
                            Compaction::Done => continue 'request,
                            Compaction::Interrupted => return StopReason::Cancelled,
                            Compaction::Failed(error) => return self.fail(error),
                        }
                    }
                    Raced::Done(Err(error)) => return self.fail(error),
                };

                let mut complete: Option<(Message, Usage)> = None;
                loop {
                    let raced = race_ops(stream.next(), &mut self.ops, turn_cancel).await;
                    match raced {
                        Raced::Interrupted => return StopReason::Cancelled,
                        Raced::Done(None) => break,
                        Raced::Done(Some(Err(ProviderError::ContextLengthExceeded)))
                            if !compacted =>
                        {
                            compacted = true;
                            match self.compact_history(turn_cancel).await {
                                // Повтор после уже отданных дельт может
                                // повторить кусок текста в UI — это дешевле
                                // мёртвого хода.
                                Compaction::Done => continue 'request,
                                Compaction::Interrupted => return StopReason::Cancelled,
                                Compaction::Failed(error) => return self.fail(error),
                            }
                        }
                        Raced::Done(Some(Err(error))) => return self.fail(error),
                        Raced::Done(Some(Ok(chunk))) => match chunk {
                            StreamChunk::TextDelta(text) => self.emit(Event::TextDelta { text }),
                            StreamChunk::ThinkingDelta(text) => {
                                self.emit(Event::ThinkingDelta { text })
                            }
                            // В MVP наружу не транслируется: аргументы всё
                            // равно уедут целиком в ToolCallStarted.
                            StreamChunk::ToolCallDelta { .. } => {}
                            StreamChunk::Complete { message, usage } => {
                                complete = Some((message, usage))
                            }
                        },
                    }
                }

                let Some(complete) = complete else {
                    self.emit(Event::Error {
                        message: "провайдер закрыл поток, не отдав готовое сообщение".to_string(),
                    });
                    return StopReason::Error;
                };
                break 'request complete;
            };

            // Типы аргументов — к схемам тулов (`"60"` → `60`), до истории,
            // событий, сессии и детектора зацикливания: везде лежат ровно те
            // аргументы, которые получит тул. Сообщение в историю ещё не
            // попало — append-only не нарушается.
            // Каждое приведение — `ArgumentsCoerced` наружу: по счётчику видно,
            // востребована ли фича (её выключатель — `[agent] arg_coercion`).
            let mut message = message;
            for block in &mut message.content {
                if let ContentBlock::ToolUse { id, name, input } = block {
                    let fields = self.registry.coerce_input(name, input);
                    if !fields.is_empty() {
                        self.emit(Event::ArgumentsCoerced {
                            id: id.clone(),
                            name: name.clone(),
                            fields,
                        });
                    }
                }
            }

            let calls: Vec<PendingCall> = message
                .tool_uses()
                .map(|(id, name, input)| PendingCall {
                    id: id.to_string(),
                    name: name.to_string(),
                    input: input.clone(),
                })
                .collect();

            // Свежий замер занятости — порог снова вправе сработать.
            self.last_usage = Some(usage);
            self.compacted_since_usage = false;
            self.push_message(message.clone());
            self.emit(Event::MessageComplete { message });
            self.emit(Event::Usage { usage });

            if calls.is_empty() {
                return StopReason::Completed;
            }
            match self
                .run_tool_calls(calls, &mut loops, &mut churn, turn_cancel)
                .await
            {
                ToolRound::Continue => {}
                ToolRound::Interrupted => return StopReason::Cancelled,
                ToolRound::LoopAborted { name, repeats } => {
                    return self.fail(format!(
                        "модель зациклилась: {repeats} одинаковых вызовов `{name}` подряд, \
                         ход остановлен"
                    ));
                }
            }
        }

        StopReason::MaxTurns
    }

    /// Исполнить тул-коллы одного ответа модели — по порядку, по одному.
    ///
    /// Результаты уходят в историю **одним** сообщением из `ToolResult`-блоков,
    /// по блоку на каждый `tool_use` — включая запрещённые, отклонённые,
    /// заблокированные детектором зацикливания и не дошедшие до исполнения.
    ///
    /// Детектор зацикливания смотрит на вызов **до** исполнения (и может его
    /// не пустить), сенсор write-churn — **после** (и только дописывает
    /// подсказку к результату). Счётчики у них независимы.
    async fn run_tool_calls(
        &mut self,
        calls: Vec<PendingCall>,
        loops: &mut LoopDetector,
        churn: &mut WriteChurnDetector,
        turn_cancel: &CancellationToken,
    ) -> ToolRound {
        let mut results: Vec<ContentBlock> = Vec::with_capacity(calls.len());
        let mut round = ToolRound::Continue;

        for call in &calls {
            // Детектор — до политики: спрашивать пользователя про вызов,
            // который всё равно не будет исполнен, незачем.
            match loops.observe(&call.name, &call.input) {
                LoopVerdict::Run => {}
                LoopVerdict::Block { repeats } => {
                    let result = repeated_call_result(repeats - 1);
                    self.finish_tool_call(&mut results, call, result, true);
                    self.nudge(
                        call,
                        Guard::LoopDetection,
                        loop_nudge_message(&call.name, repeats),
                    );
                    continue;
                }
                LoopVerdict::Abort { repeats } => {
                    let result = repeated_call_result(repeats - 1);
                    self.finish_tool_call(&mut results, call, result, true);
                    self.nudge(
                        call,
                        Guard::LoopDetection,
                        loop_nudge_message(&call.name, repeats),
                    );
                    round = ToolRound::LoopAborted {
                        name: call.name.clone(),
                        repeats,
                    };
                    break;
                }
            }

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
                            round = ToolRound::Interrupted;
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

            // Запись исполнена в любом случае; подсказка лишь дописывается к
            // её результату, и тот остаётся успешным — иначе модель решила бы,
            // что запись не удалась, и переписала бы файл ещё раз.
            let verdict = churn.observe(&call.name, &call.input, output.is_error, &self.config.cwd);
            let nudge = churn_nudge(verdict, &call.input, &self.config.cwd);
            let mut content = output.content;
            if let Some(nudge) = &nudge {
                content.push_str("\n\n");
                content.push_str(&nudge.note);
            }

            self.finish_tool_call(&mut results, call, content, output.is_error);
            if let Some(nudge) = nudge {
                self.nudge(call, Guard::WriteChurn, nudge.message);
            }
            if cancelled {
                round = ToolRound::Interrupted;
                break;
            }
        }

        // Тул-коллы, до которых ход не дошёл, всё равно получают результат:
        // без пары `tool_use` ломает следующий запрос.
        let unreached = match round {
            ToolRound::LoopAborted { .. } => LOOP_ABORTED_RESULT.to_string(),
            _ => permission::interrupted_result(),
        };
        for call in calls.iter().skip(results.len()) {
            results.push(ContentBlock::tool_error(
                call.id.as_str(),
                unreached.clone(),
            ));
        }
        if !results.is_empty() {
            self.push_message(Message::tool(results));
        }

        round
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

    /// Сработал предохранитель: [`Event::Nudge`] наружу — сразу после
    /// `ToolCallFinished` вызова, к которому относится подсказка.
    fn nudge(&self, call: &PendingCall, guard: Guard, message: String) {
        tracing::debug!(id = %call.id, guard = guard.as_str(), %message, "предохранитель");
        self.emit(Event::Nudge {
            id: call.id.clone(),
            guard,
            message,
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

    /// Ход не вытянул: наружу — [`Event::Error`], ход — `TurnFinished{Error}`.
    fn fail(&self, error: impl std::fmt::Display) -> StopReason {
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
        /// Отказать переполнением контекста — вход в реактивную компакцию.
        ContextOverflow,
        /// Отдать чанки и оборвать поток переполнением: так ведёт себя
        /// llama-server, который шлёт ошибку error-фреймом уже внутри SSE.
        StreamOverflow(Vec<StreamChunk>),
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
                Script::ContextOverflow => Err(ProviderError::ContextLengthExceeded),
                Script::StreamOverflow(chunks) => Ok(Box::pin(
                    futures::stream::iter(chunks.into_iter().map(Ok)).chain(futures::stream::once(
                        async { Err(ProviderError::ContextLengthExceeded) },
                    )),
                )),
            }
        }
    }

    /// Ответ одним текстом, без тулов.
    fn text_script(text: &str) -> Script {
        text_script_with_usage(text, Usage::new(10, 5))
    }

    /// То же, но с заданным расходом токенов: им управляется оценка занятости
    /// контекста, то есть порог компакции.
    fn text_script_with_usage(text: &str, usage: Usage) -> Script {
        Script::Chunks(vec![
            StreamChunk::TextDelta(text.to_string()),
            StreamChunk::Complete {
                message: Message::assistant(text),
                usage,
            },
        ])
    }

    /// Ответ с тул-коллами (и, возможно, текстом перед ними).
    fn tools_script(text: &str, calls: &[(&str, &str, Value)]) -> Script {
        tools_script_with_usage(text, calls, Usage::new(20, 8))
    }

    /// То же, но с заданным расходом токенов — порог компакции между итерациями.
    fn tools_script_with_usage(text: &str, calls: &[(&str, &str, Value)], usage: Usage) -> Script {
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
            usage,
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
            Self::with_compaction(scripts, policy, max_turns, CompactionConfig::default()).await
        }

        async fn with_compaction(
            scripts: Vec<Script>,
            policy: PermissionPolicy,
            max_turns: usize,
            compaction: CompactionConfig,
        ) -> Self {
            let config = AgentConfig::new("mock-model")
                .with_policy(policy)
                .with_max_turns(max_turns)
                .with_compaction(compaction);
            Self::with_config(scripts, config).await
        }

        /// Агент с произвольным конфигом; `cwd` всегда — временная директория.
        async fn with_config(scripts: Vec<Script>, config: AgentConfig) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let provider = MockProvider::new(scripts);
            let recorder = SessionRecorder::create(
                &dir.path().join("sessions"),
                SessionMeta::new(dir.path().to_string_lossy(), "mock-model", "mock"),
            )
            .await
            .unwrap();
            let session = recorder.path().to_path_buf();

            let config = config.with_cwd(dir.path());
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

    /// `(tokens_before, tokens_after)` единственной компакции в потоке.
    fn compaction(events: &[Event]) -> (u64, u64) {
        events
            .iter()
            .find_map(|event| match event {
                Event::Compacted {
                    tokens_before,
                    tokens_after,
                } => Some((*tokens_before, *tokens_after)),
                _ => None,
            })
            .expect("компакция не выполнялась")
    }

    /// Текст саммари в сообщении, если это оно.
    fn is_summary(message: &Message) -> bool {
        message.text().starts_with(compact::SUMMARY_PREFIX)
    }

    /// Сколько компакций в потоке событий.
    fn compactions(events: &[Event]) -> usize {
        events
            .iter()
            .filter(|event| matches!(event, Event::Compacted { .. }))
            .count()
    }

    /// У каждого `tool_use` среди сообщений есть парный `tool_result`.
    fn assert_tool_calls_paired(messages: &[Message]) {
        let results: Vec<&str> = messages
            .iter()
            .flat_map(|message| &message.content)
            .filter_map(|block| match block {
                ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
                _ => None,
            })
            .collect();
        for (id, name, _) in messages.iter().flat_map(Message::tool_uses) {
            assert!(results.contains(&id), "висящий tool_use `{name}` ({id})");
        }
    }

    /// Контекст 5000 токенов: порог — 4000.
    fn small_context() -> CompactionConfig {
        CompactionConfig::new().with_context_limit(5000)
    }

    /// Расход выше порога [`small_context`]: 4096 токенов.
    fn usage_over_threshold() -> Usage {
        Usage::new(4000, 96)
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

    /// Число строкой (`"timeout_seconds": "5.0"` — привычка flint/qwen3.6)
    /// приводится к схеме до исполнения: тул отрабатывает с первого раза, а
    /// событие, история следующего запроса и сессия несут уже целое — ровно
    /// то, что получил тул.
    #[tokio::test]
    async fn numeric_string_arguments_are_coerced_everywhere_before_the_tool_runs() {
        let mut harness = Harness::new(
            vec![
                tools_script(
                    "",
                    &[(
                        "call_1",
                        "bash",
                        json!({"command": "echo hi", "timeout_seconds": "5.0"}),
                    )],
                ),
                text_script("готово"),
            ],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
        )
        .await;
        let coerced = json!({"command": "echo hi", "timeout_seconds": 5});

        let events = harness.turn("скажи hi").await;
        assert_eq!(stop_reason(&events), StopReason::Completed);
        // Сигнал наружу — один на вызов, до MessageComplete с этим вызовом.
        let coerced_events: Vec<&Event> = events
            .iter()
            .filter(|event| matches!(event, Event::ArgumentsCoerced { .. }))
            .collect();
        assert_eq!(
            coerced_events,
            [&Event::ArgumentsCoerced {
                id: "call_1".into(),
                name: "bash".into(),
                fields: vec!["timeout_seconds".into()],
            }]
        );
        let order = tags(&events);
        let position = |tag: &str| order.iter().position(|t| t == tag).unwrap();
        assert!(position("arguments_coerced") < position("message_complete"));

        let started: Vec<Value> = events
            .iter()
            .filter_map(|event| match event {
                Event::ToolCallStarted { input, .. } => Some(input.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(started, std::slice::from_ref(&coerced));
        assert_eq!(
            tool_results(&events),
            [("call_1".to_string(), "hi".to_string(), false)]
        );

        let tool_inputs = |messages: &[Message]| -> Vec<Value> {
            messages
                .iter()
                .flat_map(|message| message.tool_uses().map(|(_, _, input)| input.clone()))
                .collect()
        };
        let requests = harness.provider.requests();
        assert_eq!(
            tool_inputs(&requests[1].messages),
            std::slice::from_ref(&coerced)
        );

        let items = harness.finish().await;
        assert_eq!(tool_inputs(&messages(&items)), [coerced]);
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

    // ─────────────────────── детектор зацикливания ───────────────────────

    fn started_count(events: &[Event]) -> usize {
        tags(events)
            .iter()
            .filter(|tag| *tag == "tool_call_started")
            .count()
    }

    fn read_a(id: &str) -> Script {
        tools_script("", &[(id, "read", json!({"path": "a.txt"}))])
    }

    /// Третий подряд идентичный вызов не исполняется: модель получает ошибку
    /// с инструкцией ответить текстом — и отвечает.
    #[tokio::test]
    async fn third_identical_call_in_a_row_is_blocked_with_an_instruction() {
        let mut harness = Harness::new(
            vec![
                read_a("call_1"),
                read_a("call_2"),
                read_a("call_3"),
                text_script("port = 4821"),
            ],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
        )
        .await;
        std::fs::write(harness.path("a.txt"), "port = 4821\n").unwrap();

        let events = harness.turn("какой порт?").await;
        assert_eq!(stop_reason(&events), StopReason::Completed);
        assert_eq!(started_count(&events), 2, "третий вызов не исполнялся");

        let results = tool_results(&events);
        assert_eq!(results.len(), 3);
        assert!(!results[0].2 && !results[1].2);
        assert_eq!(results[2].0, "call_3");
        assert!(results[2].2, "блокировка — ошибка тула");
        assert_eq!(results[2].1, repeated_call_result(2));
        assert!(
            results[2]
                .1
                .starts_with("You have already made this exact tool call 2 times in a row"),
            "{}",
            results[2].1
        );
        // Наружу — сигнал предохранителя, сразу за результатом вызова.
        assert_eq!(
            nudges(&events),
            [(
                "call_3".to_string(),
                Guard::LoopDetection,
                "3-й подряд одинаковый вызов `read` не исполнен".to_string()
            )]
        );
        assert_nudge_follows_its_result(&events);

        // Модель увидела блокировку обычным tool result.
        let requests = harness.provider.requests();
        assert_eq!(requests.len(), 4);
        let last = requests[3].messages.last().unwrap();
        assert_eq!(
            last.content,
            vec![ContentBlock::tool_error("call_3", repeated_call_result(2))]
        );
        harness.finish().await;
    }

    /// Модель, которая не слушает предупреждений, останавливается на 2×порог:
    /// ход — `Error`, а не 50 итераций до `max_turns`.
    #[tokio::test]
    async fn endless_identical_calls_stop_the_turn_at_twice_the_threshold() {
        // Последний сценарий повторяется бесконечно — как qwen со своим echo.
        let mut harness = Harness::new(
            vec![read_a("call_1")],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
        )
        .await;
        std::fs::write(harness.path("a.txt"), "port = 4821\n").unwrap();

        let events = harness.turn("какой порт?").await;
        assert_eq!(stop_reason(&events), StopReason::Error);
        assert_eq!(harness.provider.calls(), 6, "2 × DEFAULT_LOOP_DETECTION");
        assert_eq!(started_count(&events), 2);

        let errors: Vec<_> = tool_results(&events).iter().map(|r| r.2).collect();
        assert_eq!(errors, [false, false, true, true, true, true]);
        assert_eq!(
            tags(&events).iter().rev().take(2).collect::<Vec<_>>(),
            ["turn_finished", "error"]
        );
        assert!(events.iter().any(|event| matches!(
            event,
            Event::Error { message }
                if message.contains("зациклилась") && message.contains("6") && message.contains("`read`")
        )));

        // Последний tool_use тоже получил парный результат.
        let items = harness.finish().await;
        let history = messages(&items);
        let last = history.last().unwrap();
        assert_eq!(last.role, Role::Tool);
        assert_eq!(
            last.content,
            vec![ContentBlock::tool_error("call_1", repeated_call_result(5))]
        );
        assert_eq!(
            items.last().unwrap(),
            &SessionItem::turn_finished(turn_id(&items), StopReason::Error)
        );
    }

    /// Другие аргументы или другой тул между повторами сбрасывают счётчик.
    #[tokio::test]
    async fn a_different_call_in_between_resets_the_counter() {
        let mut harness = Harness::new(
            vec![
                read_a("call_1"),
                read_a("call_2"),
                tools_script("", &[("call_3", "read", json!({"path": "b.txt"}))]),
                read_a("call_4"),
                read_a("call_5"),
                tools_script("", &[("call_6", "bash", json!({"command": "true"}))]),
                read_a("call_7"),
                read_a("call_8"),
                text_script("готово"),
            ],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
        )
        .await;
        std::fs::write(harness.path("a.txt"), "a\n").unwrap();
        std::fs::write(harness.path("b.txt"), "b\n").unwrap();

        let events = harness.turn("почитай").await;
        assert_eq!(stop_reason(&events), StopReason::Completed);
        assert_eq!(started_count(&events), 8, "ни один вызов не заблокирован");
        assert!(tool_results(&events).iter().all(|r| !r.2), "{events:?}");
        harness.finish().await;
    }

    /// `loop_detection = 0` выключает детектор целиком.
    #[tokio::test]
    async fn zero_threshold_disables_loop_detection() {
        let mut harness = Harness::with_config(
            vec![
                read_a("call_1"),
                read_a("call_2"),
                read_a("call_3"),
                read_a("call_4"),
                read_a("call_5"),
                read_a("call_6"),
                read_a("call_7"),
                text_script("готово"),
            ],
            AgentConfig::new("mock-model")
                .with_policy(PermissionPolicy::Yolo)
                .with_loop_detection(0),
        )
        .await;
        std::fs::write(harness.path("a.txt"), "a\n").unwrap();

        let events = harness.turn("читай").await;
        assert_eq!(stop_reason(&events), StopReason::Completed);
        assert_eq!(started_count(&events), 7);
        harness.finish().await;
    }

    /// Повторы внутри одного ответа считаются так же; остановка посреди
    /// ответа оставляет парный результат и непройденным вызовам.
    #[tokio::test]
    async fn loop_abort_mid_message_still_pairs_the_remaining_calls() {
        let mut calls: Vec<(String, &str, Value)> = (1..=4)
            .map(|n| (format!("call_{n}"), "read", json!({"path": "a.txt"})))
            .collect();
        calls.push(("call_5".into(), "read", json!({"path": "b.txt"})));
        let calls: Vec<(&str, &str, Value)> = calls
            .iter()
            .map(|(id, name, input)| (id.as_str(), *name, input.clone()))
            .collect();

        let mut harness = Harness::with_config(
            vec![tools_script("", &calls)],
            AgentConfig::new("mock-model")
                .with_policy(PermissionPolicy::Yolo)
                .with_loop_detection(2),
        )
        .await;
        std::fs::write(harness.path("a.txt"), "a\n").unwrap();
        std::fs::write(harness.path("b.txt"), "b\n").unwrap();

        let events = harness.turn("читай").await;
        assert_eq!(stop_reason(&events), StopReason::Error);
        assert_eq!(harness.provider.calls(), 1);
        assert_eq!(started_count(&events), 1, "порог 2: исполнен только первый");

        let items = harness.finish().await;
        let history = messages(&items);
        let last = history.last().unwrap();
        assert_eq!(last.role, Role::Tool);
        assert!(matches!(
            &last.content[0],
            ContentBlock::ToolResult { tool_use_id, is_error: false, .. } if tool_use_id == "call_1"
        ));
        assert_eq!(
            last.content[1..],
            [
                ContentBlock::tool_error("call_2", repeated_call_result(1)),
                ContentBlock::tool_error("call_3", repeated_call_result(2)),
                ContentBlock::tool_error("call_4", repeated_call_result(3)),
                ContentBlock::tool_error("call_5", LOOP_ABORTED_RESULT),
            ]
        );
    }

    #[test]
    fn loop_detector_counts_consecutive_identical_calls() {
        let a = json!({"path": "a"});
        let b = json!({"path": "b"});

        let mut detector = LoopDetector::new(3);
        assert_eq!(detector.observe("read", &a), LoopVerdict::Run);
        assert_eq!(detector.observe("read", &a), LoopVerdict::Run);
        assert_eq!(
            detector.observe("read", &a),
            LoopVerdict::Block { repeats: 3 }
        );
        // То же имя, другие аргументы — сброс.
        assert_eq!(detector.observe("read", &b), LoopVerdict::Run);
        // Те же аргументы, другой тул — тоже сброс.
        assert_eq!(detector.observe("cat", &b), LoopVerdict::Run);
        for _ in 0..2 {
            detector.observe("cat", &b);
        }
        assert_eq!(
            detector.observe("cat", &b),
            LoopVerdict::Block { repeats: 4 }
        );
        assert_eq!(
            detector.observe("cat", &b),
            LoopVerdict::Block { repeats: 5 }
        );
        assert_eq!(
            detector.observe("cat", &b),
            LoopVerdict::Abort { repeats: 6 }
        );

        // Порядок ключей — часть сериализованного ключа: это другой вызов.
        let mut detector = LoopDetector::new(2);
        detector.observe("write", &json!({"path": "a", "content": "x"}));
        assert_eq!(
            detector.observe("write", &json!({"content": "x", "path": "a"})),
            LoopVerdict::Run
        );

        // 0 — выключено; 1 ведёт себя как 2.
        let mut off = LoopDetector::new(0);
        for _ in 0..10 {
            assert_eq!(off.observe("read", &a), LoopVerdict::Run);
        }
        let mut one = LoopDetector::new(1);
        assert_eq!(one.observe("read", &a), LoopVerdict::Run);
        assert_eq!(one.observe("read", &a), LoopVerdict::Block { repeats: 2 });
        assert_eq!(one.observe("read", &a), LoopVerdict::Block { repeats: 3 });
        assert_eq!(one.observe("read", &a), LoopVerdict::Abort { repeats: 4 });
    }

    // ───────────────────────── сенсор write-churn ─────────────────────────

    /// `write` файла `path` одним ответом модели.
    fn write_script(id: &str, path: &str, content: &str) -> Script {
        tools_script(
            "",
            &[(id, "write", json!({"path": path, "content": content}))],
        )
    }

    /// `edit` (search/replace) одним ответом модели.
    fn edit_script(id: &str, path: &str, old: &str, new: &str) -> Script {
        tools_script(
            "",
            &[(
                id,
                "edit",
                json!({"path": path, "old_string": old, "new_string": new}),
            )],
        )
    }

    fn bash_script(id: &str, command: &str) -> Script {
        tools_script("", &[(id, "bash", json!({"command": command}))])
    }

    /// `n` подряд записей `a.py` с разным содержимым (`call_1`…`call_n`).
    fn churning_writes(n: usize) -> Vec<Script> {
        (1..=n)
            .map(|i| write_script(&format!("call_{i}"), "a.py", &format!("v{i}")))
            .collect()
    }

    /// Все `Nudge` как `(id, guard, message)`.
    fn nudges(events: &[Event]) -> Vec<(String, Guard, String)> {
        events
            .iter()
            .filter_map(|event| match event {
                Event::Nudge { id, guard, message } => Some((id.clone(), *guard, message.clone())),
                _ => None,
            })
            .collect()
    }

    /// Id вызовов, получивших подсказку write-churn.
    fn churn_ids(events: &[Event]) -> Vec<String> {
        nudges(events)
            .into_iter()
            .filter(|(_, guard, _)| *guard == Guard::WriteChurn)
            .map(|(id, ..)| id)
            .collect()
    }

    /// Каждый `Nudge` идёт сразу за `ToolCallFinished` своего вызова.
    fn assert_nudge_follows_its_result(events: &[Event]) {
        for (index, event) in events.iter().enumerate() {
            if let Event::Nudge { id, .. } = event {
                assert!(
                    matches!(
                        index.checked_sub(1).map(|prev| &events[prev]),
                        Some(Event::ToolCallFinished { id: finished, .. }) if finished == id
                    ),
                    "Nudge {id} не следует за своим результатом: {events:?}"
                );
            }
        }
    }

    /// Третья подряд запись в один файл без запуска между ними исполняется
    /// как обычно, но к её результату приложена мягкая подсказка — и модель
    /// видит её в следующем запросе.
    #[tokio::test]
    async fn third_write_to_one_path_without_a_run_gets_a_soft_nudge() {
        let mut scripts = churning_writes(3);
        scripts.push(text_script("готово"));
        let mut harness = Harness::new(scripts, PermissionPolicy::Yolo, DEFAULT_MAX_TURNS).await;

        let events = harness.turn("напиши тест").await;
        assert_eq!(stop_reason(&events), StopReason::Completed);
        assert_eq!(started_count(&events), 3, "запись не блокируется");
        assert_eq!(std::fs::read_to_string(harness.path("a.py")).unwrap(), "v3");

        let results = tool_results(&events);
        assert!(results.iter().all(|r| !r.2), "подсказка — не ошибка");
        let expected = churn_nudge(
            ChurnVerdict::Soft { writes: 3 },
            &json!({"path": "a.py"}),
            harness.dir.path(),
        )
        .unwrap();
        assert!(!results[0].1.contains(&expected.note));
        assert!(!results[1].1.contains(&expected.note));
        assert!(
            results[2].1.starts_with("Overwrote ") && results[2].1.contains("a.py (2 bytes)"),
            "результат самой записи на месте: {}",
            results[2].1
        );
        assert!(
            results[2].1.ends_with(&format!("\n\n{}", expected.note)),
            "{}",
            results[2].1
        );
        assert!(
            expected
                .note
                .starts_with("Note: this is write #3 to a.py in a row"),
            "{}",
            expected.note
        );

        assert_eq!(
            nudges(&events),
            [(
                "call_3".to_string(),
                Guard::WriteChurn,
                "3-я запись подряд в a.py без запуска между ними — подсказка запустить".to_string()
            )]
        );
        assert_nudge_follows_its_result(&events);

        // Модель увидела подсказку внутри обычного успешного tool result.
        let requests = harness.provider.requests();
        assert_eq!(requests.len(), 4);
        let last = requests[3].messages.last().unwrap();
        assert!(matches!(
            &last.content[..],
            [ContentBlock::ToolResult { tool_use_id, content, is_error: false }]
                if tool_use_id == "call_3" && content.ends_with(&expected.note)
        ));
        harness.finish().await;
    }

    /// Любой исполненный `bash` между записями — сброс: модель что-то
    /// запустила и увидела настоящий вывод. Упавшая команда — тоже запуск.
    #[tokio::test]
    async fn a_bash_run_between_writes_resets_the_churn_counter() {
        let mut harness = Harness::new(
            vec![
                write_script("call_1", "a.py", "v1"),
                write_script("call_2", "a.py", "v2"),
                bash_script("call_3", "true"),
                write_script("call_4", "a.py", "v3"),
                write_script("call_5", "a.py", "v4"),
                bash_script("call_6", "exit 1"),
                write_script("call_7", "a.py", "v5"),
                write_script("call_8", "a.py", "v6"),
                text_script("готово"),
            ],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
        )
        .await;

        let events = harness.turn("напиши").await;
        assert_eq!(stop_reason(&events), StopReason::Completed);
        assert_eq!(started_count(&events), 8);
        assert!(tool_results(&events)[5].2, "`exit 1` — упавший запуск");
        assert_eq!(nudges(&events), []);
        harness.finish().await;
    }

    /// `read` между записями счётчик **не** сбрасывает: перечитать свой файл —
    /// не то же, что его запустить.
    #[tokio::test]
    async fn reading_between_writes_does_not_reset_the_churn_counter() {
        let read = |id: &str| tools_script("", &[(id, "read", json!({"path": "a.py"}))]);
        let mut harness = Harness::new(
            vec![
                write_script("call_1", "a.py", "v1"),
                read("call_2"),
                write_script("call_3", "a.py", "v2"),
                read("call_4"),
                write_script("call_5", "a.py", "v3"),
                text_script("готово"),
            ],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
        )
        .await;

        let events = harness.turn("напиши").await;
        assert_eq!(stop_reason(&events), StopReason::Completed);
        assert_eq!(churn_ids(&events), ["call_5"]);
        harness.finish().await;
    }

    /// Запись в другой путь начинает отсчёт заново: чередование файлов —
    /// это работа над проектом, а не метание вокруг одного.
    #[tokio::test]
    async fn writes_to_different_files_do_not_nudge() {
        let mut harness = Harness::new(
            vec![
                write_script("call_1", "a.py", "v1"),
                write_script("call_2", "a.py", "v2"),
                write_script("call_3", "b.py", "v1"),
                write_script("call_4", "a.py", "v3"),
                write_script("call_5", "a.py", "v4"),
                write_script("call_6", "b.py", "v2"),
                write_script("call_7", "c.py", "v1"),
                text_script("готово"),
            ],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
        )
        .await;

        let events = harness.turn("напиши проект").await;
        assert_eq!(stop_reason(&events), StopReason::Completed);
        assert_eq!(started_count(&events), 7);
        assert_eq!(nudges(&events), []);
        harness.finish().await;
    }

    /// С удвоенного порога подсказка жёсткая, но ход идёт дальше: в отличие
    /// от тупого повтора, переписывание файла — законная работа. Детектор
    /// зацикливания на разные записи не реагирует.
    #[tokio::test]
    async fn sixth_write_in_a_row_gets_a_hard_nudge_and_the_turn_goes_on() {
        let mut scripts = churning_writes(7);
        scripts.push(text_script("готово"));
        let mut harness = Harness::new(scripts, PermissionPolicy::Yolo, DEFAULT_MAX_TURNS).await;

        let events = harness.turn("напиши тест").await;
        assert_eq!(stop_reason(&events), StopReason::Completed);
        assert_eq!(started_count(&events), 7, "ни одна запись не заблокирована");
        assert_eq!(std::fs::read_to_string(harness.path("a.py")).unwrap(), "v7");

        let all = nudges(&events);
        assert!(
            all.iter().all(|(_, guard, _)| *guard == Guard::WriteChurn),
            "{all:?}"
        );
        assert_eq!(
            churn_ids(&events),
            ["call_3", "call_4", "call_5", "call_6", "call_7"]
        );
        assert_nudge_follows_its_result(&events);

        let cwd = harness.dir.path();
        let input = json!({"path": "a.py"});
        let results = tool_results(&events);
        assert!(results.iter().all(|r| !r.2));
        for (index, writes) in [(2, 3), (3, 4), (4, 5)] {
            let soft = churn_nudge(ChurnVerdict::Soft { writes }, &input, cwd).unwrap();
            assert!(
                results[index].1.ends_with(&soft.note),
                "{}",
                results[index].1
            );
        }
        for (index, writes) in [(5, 6), (6, 7)] {
            let hard = churn_nudge(ChurnVerdict::Hard { writes }, &input, cwd).unwrap();
            assert!(
                results[index].1.ends_with(&hard.note),
                "{}",
                results[index].1
            );
        }
        assert!(results[5].1.contains("Warning: 6 writes to a.py in a row"));
        assert!(results[5].1.contains("Change strategy"));
        assert_eq!(
            all[3].2,
            "6-я запись подряд в a.py без запуска между ними — жёсткая подсказка сменить \
             стратегию"
        );
        harness.finish().await;
    }

    /// `write` и `edit` в один путь считаются вместе, как бы модель этот
    /// путь ни писала (`a.py`, `./a.py`).
    #[tokio::test]
    async fn edits_and_writes_to_one_path_count_together() {
        let mut harness = Harness::new(
            vec![
                write_script("call_1", "app.py", "print(1)\n"),
                edit_script("call_2", "./app.py", "print(1)", "print(2)"),
                edit_script("call_3", "app.py", "print(2)", "print(3)"),
                text_script("готово"),
            ],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
        )
        .await;

        let events = harness.turn("поправь").await;
        assert_eq!(stop_reason(&events), StopReason::Completed);
        assert_eq!(
            std::fs::read_to_string(harness.path("app.py")).unwrap(),
            "print(3)\n"
        );
        assert_eq!(churn_ids(&events), ["call_3"]);
        let results = tool_results(&events);
        assert!(!results[2].2);
        assert!(
            results[2].1.starts_with("Replaced 1 occurrence in "),
            "{}",
            results[2].1
        );
        assert!(
            results[2].1.contains("write #3 to app.py"),
            "{}",
            results[2].1
        );
        harness.finish().await;
    }

    /// После подсказки модель запустила файл — счётчик чист: следующие две
    /// записи тихие, а новая серия считается с нуля.
    #[tokio::test]
    async fn a_run_after_the_nudge_starts_the_count_over() {
        let mut harness = Harness::new(
            vec![
                write_script("call_1", "a.py", "v1"),
                write_script("call_2", "a.py", "v2"),
                write_script("call_3", "a.py", "v3"),
                bash_script("call_4", "cat a.py"),
                write_script("call_5", "a.py", "v4"),
                write_script("call_6", "a.py", "v5"),
                write_script("call_7", "a.py", "v6"),
                text_script("готово"),
            ],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
        )
        .await;

        let events = harness.turn("напиши").await;
        assert_eq!(stop_reason(&events), StopReason::Completed);
        assert_eq!(churn_ids(&events), ["call_3", "call_7"]);
        let results = tool_results(&events);
        assert!(results[2].1.contains("write #3 to a.py"));
        assert!(
            results[6].1.contains("write #3 to a.py"),
            "серия после запуска считается заново: {}",
            results[6].1
        );
        harness.finish().await;
    }

    /// Детектор зацикливания и сенсор write-churn друг друга не душат:
    /// заблокированная повторная запись не исполнялась — сенсор её не видит, а
    /// следующая, уже другая, запись не повтор — детектор её пропускает.
    #[tokio::test]
    async fn write_churn_and_loop_detection_keep_separate_counters() {
        let mut harness = Harness::new(
            vec![
                write_script("call_1", "a.py", "same"),
                write_script("call_2", "a.py", "same"),
                write_script("call_3", "a.py", "same"),
                write_script("call_4", "a.py", "other"),
                text_script("готово"),
            ],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
        )
        .await;

        let events = harness.turn("напиши").await;
        assert_eq!(stop_reason(&events), StopReason::Completed);
        assert_eq!(started_count(&events), 3, "третий повтор не исполнялся");

        let results = tool_results(&events);
        assert_eq!(
            results[2],
            ("call_3".to_string(), repeated_call_result(2), true)
        );
        assert!(!results[3].2);
        assert!(
            results[3].1.contains("write #3 to a.py"),
            "{}",
            results[3].1
        );

        let guards: Vec<(String, Guard)> = nudges(&events)
            .into_iter()
            .map(|(id, guard, _)| (id, guard))
            .collect();
        assert_eq!(
            guards,
            [
                ("call_3".to_string(), Guard::LoopDetection),
                ("call_4".to_string(), Guard::WriteChurn),
            ]
        );
        assert_nudge_follows_its_result(&events);
        assert_eq!(
            std::fs::read_to_string(harness.path("a.py")).unwrap(),
            "other"
        );
        harness.finish().await;
    }

    /// Абляция: `[agent] write_churn = false` выключает сенсор целиком — ни
    /// подсказок, ни событий даже на 7 перезаписях подряд; `true` и дефолт
    /// (ключа нет) — сенсор как обычно. Путь тот же, что в CLI: TOML →
    /// `AgentSettings` → `AgentConfig::write_churn`.
    #[tokio::test]
    async fn write_churn_switch_in_the_config_turns_the_sensor_off_and_on() {
        let on = vec!["call_3", "call_4", "call_5", "call_6", "call_7"];
        for (agent_section, expected) in [
            ("write_churn = false", vec![]),
            ("write_churn = true", on.clone()),
            ("", on.clone()),
        ] {
            let settings =
                crate::config::Config::parse(&format!("[agent]\n{agent_section}\n"), "t")
                    .unwrap()
                    .agent;
            let mut config = AgentConfig::new("mock-model").with_policy(PermissionPolicy::Yolo);
            if let Some(threshold) = settings.write_churn {
                config = config.with_write_churn(threshold);
            }

            let mut scripts = churning_writes(7);
            scripts.push(text_script("готово"));
            let mut harness = Harness::with_config(scripts, config).await;
            let events = harness.turn("напиши").await;
            assert_eq!(stop_reason(&events), StopReason::Completed);
            assert_eq!(started_count(&events), 7, "{agent_section}");
            assert_eq!(churn_ids(&events), expected, "{agent_section:?}");
            if expected.is_empty() {
                assert_eq!(nudges(&events), [], "{agent_section}");
                for (_, content, _) in tool_results(&events) {
                    assert!(
                        content.starts_with("Wrote ") || content.starts_with("Overwrote "),
                        "{content}"
                    );
                    assert!(!content.contains("in a row"), "{content}");
                }
            }
            harness.finish().await;
        }
    }

    #[test]
    fn write_churn_detector_counts_consecutive_writes_to_one_path() {
        let cwd = Path::new("/work");
        let at = |path: &str| json!({"path": path, "content": "x"});
        let ok = false;
        let failed = true;

        let mut churn = WriteChurnDetector::new(3);
        // Относительный, `./` и абсолютный путь — один файл; write и edit — одна
        // серия.
        assert_eq!(
            churn.observe("write", &at("a.py"), ok, cwd),
            ChurnVerdict::Quiet
        );
        assert_eq!(
            churn.observe("edit", &at("./a.py"), ok, cwd),
            ChurnVerdict::Quiet
        );
        assert_eq!(
            churn.observe("write", &at("/work/a.py"), ok, cwd),
            ChurnVerdict::Soft { writes: 3 }
        );
        // `read` и упавшая запись нейтральны: не сбрасывают и не считаются.
        assert_eq!(
            churn.observe("read", &at("a.py"), ok, cwd),
            ChurnVerdict::Quiet
        );
        assert_eq!(
            churn.observe("edit", &at("a.py"), failed, cwd),
            ChurnVerdict::Quiet
        );
        assert_eq!(
            churn.observe("edit", &at("a.py"), ok, cwd),
            ChurnVerdict::Soft { writes: 4 }
        );
        // Как и незнакомый тул и запись без строкового `path`.
        assert_eq!(
            churn.observe("browse", &at("a.py"), ok, cwd),
            ChurnVerdict::Quiet
        );
        assert_eq!(
            churn.observe("write", &json!({}), ok, cwd),
            ChurnVerdict::Quiet
        );
        assert_eq!(
            churn.observe("write", &at("a.py"), ok, cwd),
            ChurnVerdict::Soft { writes: 5 }
        );
        assert_eq!(
            churn.observe("write", &at("a.py"), ok, cwd),
            ChurnVerdict::Hard { writes: 6 }
        );
        assert_eq!(
            churn.observe("write", &at("a.py"), ok, cwd),
            ChurnVerdict::Hard { writes: 7 }
        );

        // `bash` — сброс, даже упавший.
        assert_eq!(
            churn.observe("bash", &json!({"command": "python3 a.py"}), failed, cwd),
            ChurnVerdict::Quiet
        );
        assert_eq!(
            churn.observe("write", &at("a.py"), ok, cwd),
            ChurnVerdict::Quiet
        );
        assert_eq!(
            churn.observe("write", &at("a.py"), ok, cwd),
            ChurnVerdict::Quiet
        );
        // Другой путь — отсчёт заново, и для старого тоже.
        assert_eq!(
            churn.observe("write", &at("b.py"), ok, cwd),
            ChurnVerdict::Quiet
        );
        assert_eq!(
            churn.observe("write", &at("a.py"), ok, cwd),
            ChurnVerdict::Quiet
        );
        // `..` сворачивается лексически.
        assert_eq!(
            churn.observe("write", &at("sub/../a.py"), ok, cwd),
            ChurnVerdict::Quiet
        );
        assert_eq!(
            churn.observe("write", &at("/work/./a.py"), ok, cwd),
            ChurnVerdict::Soft { writes: 3 }
        );

        // 0 — выключено; 1 ведёт себя как 2.
        let mut off = WriteChurnDetector::new(0);
        for _ in 0..10 {
            assert_eq!(
                off.observe("write", &at("a.py"), ok, cwd),
                ChurnVerdict::Quiet
            );
        }
        let mut one = WriteChurnDetector::new(1);
        assert_eq!(
            one.observe("write", &at("a.py"), ok, cwd),
            ChurnVerdict::Quiet
        );
        assert_eq!(
            one.observe("write", &at("a.py"), ok, cwd),
            ChurnVerdict::Soft { writes: 2 }
        );
        assert_eq!(
            one.observe("write", &at("a.py"), ok, cwd),
            ChurnVerdict::Soft { writes: 3 }
        );
        assert_eq!(
            one.observe("write", &at("a.py"), ok, cwd),
            ChurnVerdict::Hard { writes: 4 }
        );
    }

    #[test]
    fn churn_nudge_speaks_to_the_model_and_to_the_human() {
        let cwd = Path::new("/work");
        let input = json!({"path": "/work/tests/test_chat.py", "content": "x"});
        assert_eq!(churn_nudge(ChurnVerdict::Quiet, &input, cwd), None);

        // Модели — путь в её же написании и что делать вместо переписывания.
        let soft = churn_nudge(ChurnVerdict::Soft { writes: 3 }, &input, cwd).unwrap();
        assert!(
            soft.note
                .starts_with("Note: this is write #3 to /work/tests/test_chat.py in a row"),
            "{}",
            soft.note
        );
        assert!(soft.note.contains("run it"), "{}", soft.note);
        assert!(soft.note.contains("read the actual error"), "{}", soft.note);
        // Человеку — путь от cwd.
        assert_eq!(
            soft.message,
            "3-я запись подряд в tests/test_chat.py без запуска между ними — подсказка запустить"
        );

        let hard = churn_nudge(ChurnVerdict::Hard { writes: 6 }, &input, cwd).unwrap();
        assert!(
            hard.note
                .starts_with("Warning: 6 writes to /work/tests/test_chat.py in a row"),
            "{}",
            hard.note
        );
        assert!(hard.note.contains("Change strategy"), "{}", hard.note);
        assert!(hard.note.contains("make the file smaller"), "{}", hard.note);
        assert_eq!(
            hard.message,
            "6-я запись подряд в tests/test_chat.py без запуска между ними — жёсткая подсказка \
             сменить стратегию"
        );

        // Файл вне cwd человеку показывается полным путём.
        let outside = churn_nudge(
            ChurnVerdict::Soft { writes: 3 },
            &json!({"path": "/tmp/x.py"}),
            cwd,
        )
        .unwrap();
        assert!(
            outside.message.contains(" /tmp/x.py "),
            "{}",
            outside.message
        );
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

    // ──────────────────────────── память ────────────────────────────

    /// Нет файла памяти — промпт агента ровно тот же, что до памяти:
    /// [`prompt::system_prompt`] байт в байт.
    #[test]
    fn without_a_memory_file_the_prompt_is_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Agent::new(
            MockProvider::new(vec![text_script("ок")]),
            ToolRegistry::new(),
            AgentConfig::new("mock-model").with_cwd(dir.path()),
        );
        assert_eq!(agent.memory(), None);
        assert_eq!(agent.system_prompt(), prompt::system_prompt(dir.path()));
        assert_eq!(agent.system_prompt(), prompt::base_prompt(dir.path()));
    }

    /// Файл памяти в `cwd` — в конце промпта; явный `memory_file` главнее
    /// поиска; `with_system_prompt` память выбрасывает вместе с промптом.
    #[test]
    fn memory_is_injected_at_build_time() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(memory::MEMORY_FILE),
            "# Memory\n- staging port 7342\n",
        )
        .unwrap();
        let agent = Agent::new(
            MockProvider::new(vec![text_script("ок")]),
            ToolRegistry::new(),
            AgentConfig::new("mock-model").with_cwd(dir.path()),
        );
        let memory = agent.memory().expect("память найдена");
        assert_eq!(memory.path, dir.path().join(memory::MEMORY_FILE));
        assert!(
            agent
                .system_prompt()
                .starts_with(&prompt::system_prompt(dir.path()))
        );
        assert!(agent.system_prompt().contains(prompt::MEMORY_SECTION));
        assert!(agent.system_prompt().ends_with("- staging port 7342"));

        let replaced = agent.with_system_prompt("s");
        assert_eq!(replaced.memory(), None);
        assert_eq!(replaced.system_prompt(), "s");

        std::fs::write(dir.path().join("other.md"), "- явная память\n").unwrap();
        let agent = Agent::new(
            MockProvider::new(vec![text_script("ок")]),
            ToolRegistry::new(),
            AgentConfig::new("mock-model")
                .with_cwd(dir.path())
                .with_memory_file("other.md"),
        );
        assert!(agent.system_prompt().ends_with("- явная память"));
        assert!(!agent.system_prompt().contains("7342"));
    }

    /// Память — в системном промпте, а компакция его не трогает: после сжатия
    /// истории следующий запрос несёт тот же промпт с той же памятью.
    #[tokio::test]
    async fn memory_survives_compaction() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(memory::MEMORY_FILE),
            "# Memory\n- The staging deploy port is 7342.\n",
        )
        .unwrap();
        let provider = MockProvider::new(vec![
            text_script_with_usage("первый ответ", Usage::new(4000, 96)),
            text_script("Task: разобраться. Next: продолжить"),
            text_script("7342"),
        ]);
        let agent = Agent::new(
            provider.clone(),
            ToolRegistry::with_defaults(EditStrategy::SearchReplace),
            AgentConfig::new("mock-model")
                .with_cwd(dir.path())
                .with_compaction(CompactionConfig::new().with_context_limit(5000)),
        );
        let system = agent.system_prompt().to_string();
        assert!(system.contains("The staging deploy port is 7342."));

        let (handle, mut events) = agent.spawn();
        let mut tags_seen = Vec::new();
        for text in ["первый вопрос", "какой порт staging?"] {
            handle.submit(Op::user_input(text));
            loop {
                let event = tokio::time::timeout(EVENT_TIMEOUT, events.recv())
                    .await
                    .unwrap()
                    .unwrap();
                let finished = matches!(event, Event::TurnFinished { .. });
                tags_seen.push(serde_json::to_value(&event).unwrap()["type"].clone());
                if finished {
                    break;
                }
            }
        }
        handle.shutdown().await;
        assert!(tags_seen.contains(&json!("compacted")), "{tags_seen:?}");

        let requests = provider.requests();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].system, system);
        assert_eq!(requests[1].system, compact::SUMMARY_SYSTEM_PROMPT);
        // После компакции: история — саммари, а промпт с памятью — байт в байт прежний.
        assert!(is_summary(&requests[2].messages[0]));
        assert_eq!(requests[2].system, system);
        // Файл памяти компакция тоже не трогает.
        assert_eq!(
            std::fs::read_to_string(dir.path().join(memory::MEMORY_FILE)).unwrap(),
            "# Memory\n- The staging deploy port is 7342.\n"
        );
    }

    /// Память не влезла в кап — первым событием, ещё до хода, идёт
    /// предупреждение; дальше ход как обычно.
    #[tokio::test]
    async fn truncated_memory_is_announced_by_a_warning_event() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(memory::MEMORY_FILE),
            "- one more fact\n".repeat(1000),
        )
        .unwrap();
        let (handle, mut events) = Agent::new(
            MockProvider::new(vec![text_script("ок")]),
            ToolRegistry::new(),
            AgentConfig::new("mock-model").with_cwd(dir.path()),
        )
        .spawn();
        handle.submit(Op::user_input("привет"));

        let mut collected = Vec::new();
        loop {
            let event = tokio::time::timeout(EVENT_TIMEOUT, events.recv())
                .await
                .unwrap()
                .unwrap();
            let finished = matches!(event, Event::TurnFinished { .. });
            collected.push(event);
            if finished {
                break;
            }
        }
        handle.shutdown().await;

        let Event::Warning { message } = &collected[0] else {
            panic!("первым должно быть предупреждение: {collected:?}");
        };
        assert!(message.contains(memory::MEMORY_FILE), "{message}");
        assert!(matches!(collected[1], Event::TurnStarted { .. }));
        assert_eq!(stop_reason(&collected), StopReason::Completed);
        let warnings = collected
            .iter()
            .filter(|event| matches!(event, Event::Warning { .. }))
            .count();
        assert_eq!(warnings, 1);
    }

    // ─────────────────────────── компакция ───────────────────────────

    /// Порог: перед сэмплингом история сжимается, и в следующий запрос уходит
    /// саммари вместо переписки.
    #[tokio::test]
    async fn threshold_compacts_before_sampling_and_the_next_request_carries_the_summary() {
        let mut harness = Harness::with_compaction(
            vec![
                // Ход 1 набивает контекст: 4096 токенов при лимите 5000.
                text_script_with_usage("первый ответ", Usage::new(4000, 96)),
                text_script("Task: разобраться. Next: продолжить"),
                text_script("второй ответ"),
            ],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
            CompactionConfig::new().with_context_limit(5000),
        )
        .await;

        let events = harness.turn("первый вопрос").await;
        assert_eq!(stop_reason(&events), StopReason::Completed);
        assert!(
            !tags(&events).contains(&"compacted".to_string()),
            "на пустой истории сжимать нечего"
        );

        let events = harness.turn("второй вопрос").await;
        assert_eq!(
            tags(&events),
            [
                "turn_started",
                "compacted",
                "text_delta",
                "message_complete",
                "usage",
                "turn_finished",
            ]
        );
        assert_eq!(stop_reason(&events), StopReason::Completed);

        let (before, after) = compaction(&events);
        assert_eq!(before, 4096, "оценка из последнего usage");
        assert!(before > after, "{before} → {after}");

        let requests = harness.provider.requests();
        assert_eq!(requests.len(), 3);

        // Запрос суммаризации: свой короткий system, вся видимая история и
        // инструкция последним сообщением, без тулов.
        assert_eq!(requests[1].system, compact::SUMMARY_SYSTEM_PROMPT);
        assert!(requests[1].tools.is_empty());
        assert_eq!(requests[1].messages.len(), 3);
        assert_eq!(requests[1].messages[0], Message::user("первый вопрос"));
        assert_eq!(requests[1].messages[1], Message::assistant("первый ответ"));
        assert_eq!(
            requests[1].messages[2],
            Message::user(compact::SUMMARY_INSTRUCTIONS)
        );

        // Следующий запрос агента: саммари плюс свежий ввод, старое отфильтровано.
        assert_eq!(requests[2].system, "тестовый системный промпт");
        assert!(!requests[2].tools.is_empty(), "тулы на месте");
        assert_eq!(requests[2].messages.len(), 2);
        assert!(is_summary(&requests[2].messages[0]));
        assert!(requests[2].messages[0].text().contains("Next: продолжить"));
        assert_eq!(requests[2].messages[1], Message::user("второй вопрос"));

        harness.finish().await;
    }

    /// Без `context_limit` порог молчит, каким бы длинным ни был диалог.
    #[tokio::test]
    async fn without_a_context_limit_the_threshold_never_fires() {
        let mut harness = Harness::new(
            vec![text_script_with_usage("ответ", Usage::new(1_000_000, 0))],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
        )
        .await;

        harness.turn("первый вопрос").await;
        let events = harness.turn("второй вопрос").await;
        assert!(!tags(&events).contains(&"compacted".to_string()));
        assert_eq!(harness.provider.calls(), 2, "суммаризации не было");
        harness.finish().await;
    }

    /// Фолбэк оценки: провайдер не вернул счётчики — считаем по байтам истории.
    #[tokio::test]
    async fn byte_estimate_triggers_compaction_when_the_provider_reports_no_usage() {
        let mut harness = Harness::with_compaction(
            vec![
                text_script_with_usage("ок", Usage::default()),
                text_script("саммари"),
                text_script("второй ответ"),
            ],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
            CompactionConfig::new().with_context_limit(1000),
        )
        .await;

        // ~4000 байт истории ≈ 1000 токенов при пороге 800.
        harness.turn(&"щ".repeat(2000)).await;
        let events = harness.turn("второй вопрос").await;

        let (before, after) = compaction(&events);
        assert!(before > 1000, "байтовая оценка: {before}");
        assert!(before > after, "{before} → {after}");
        assert_eq!(stop_reason(&events), StopReason::Completed);

        let requests = harness.provider.requests();
        assert_eq!(requests.len(), 3);
        assert!(is_summary(&requests[2].messages[0]));
        harness.finish().await;
    }

    /// Реактивный путь: переполнение → компакция → один повтор → успех.
    #[tokio::test]
    async fn context_length_exceeded_compacts_and_retries_once() {
        let mut harness = Harness::new(
            vec![
                text_script_with_usage("первый ответ", Usage::new(4000, 96)),
                Script::ContextOverflow,
                text_script("Task: разобраться. Next: продолжить"),
                text_script("второй ответ"),
            ],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
        )
        .await;

        harness.turn("первый вопрос").await;
        let events = harness.turn("второй вопрос").await;
        assert_eq!(
            tags(&events),
            [
                "turn_started",
                "compacted",
                "text_delta",
                "message_complete",
                "usage",
                "turn_finished",
            ],
            "ошибки наружу нет — переполнение вылечено компакцией"
        );
        assert_eq!(stop_reason(&events), StopReason::Completed);
        let (before, after) = compaction(&events);
        assert!(before > after, "{before} → {after}");

        let requests = harness.provider.requests();
        assert_eq!(requests.len(), 4, "запрос, суммаризация, повтор");
        // Реактивная компакция случается, когда ввод уже в истории, — он тоже
        // уходит в скрытые, а его содержание живёт в саммари.
        assert_eq!(requests[1].messages.len(), 3);
        assert_eq!(requests[2].system, compact::SUMMARY_SYSTEM_PROMPT);
        assert_eq!(requests[3].messages.len(), 1);
        assert!(is_summary(&requests[3].messages[0]));

        // Скрытые сообщения остались в истории — их видно в сессии.
        let items = harness.finish().await;
        let history = messages(&items);
        assert!(history.iter().any(is_summary));
        assert!(
            history
                .iter()
                .any(|m| m.text() == "второй вопрос" && !m.is_agent_visible())
        );
    }

    /// Переполнение, пришедшее error-фреймом посреди SSE (llama-server), —
    /// тот же путь: компакция и один повтор.
    #[tokio::test]
    async fn context_overflow_mid_stream_compacts_and_retries_once() {
        let mut harness = Harness::new(
            vec![
                text_script_with_usage("первый ответ", Usage::new(4000, 96)),
                Script::StreamOverflow(vec![StreamChunk::TextDelta("начал и упёрся".into())]),
                text_script("Task: разобраться. Next: продолжить"),
                text_script("второй ответ"),
            ],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
        )
        .await;

        harness.turn("первый вопрос").await;
        let events = harness.turn("второй вопрос").await;
        assert_eq!(
            tags(&events),
            [
                "turn_started",
                "text_delta",
                "compacted",
                "text_delta",
                "message_complete",
                "usage",
                "turn_finished",
            ],
            "дельты до обрыва уже отданы, повтор идёт после компакции"
        );
        assert_eq!(stop_reason(&events), StopReason::Completed);
        assert_eq!(harness.provider.calls(), 4);

        let requests = harness.provider.requests();
        assert_eq!(requests[3].messages.len(), 1);
        assert!(is_summary(&requests[3].messages[0]));
        harness.finish().await;
    }

    /// Переполнение и после компакции — ход падает, но цикл не зацикливается.
    #[tokio::test]
    async fn a_second_overflow_after_compaction_fails_the_turn() {
        let mut harness = Harness::new(
            vec![
                Script::ContextOverflow,
                text_script("саммари"),
                // Последний сценарий повторяется: если бы повтор был не один,
                // тест висел бы вечно.
                Script::ContextOverflow,
            ],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
        )
        .await;

        let events = harness.turn("вопрос").await;
        assert_eq!(
            tags(&events),
            ["turn_started", "compacted", "error", "turn_finished"]
        );
        assert_eq!(stop_reason(&events), StopReason::Error);
        assert!(matches!(
            &events[2],
            Event::Error { message } if message.contains("контекст модели переполнен")
        ));
        assert_eq!(harness.provider.calls(), 3, "запрос, суммаризация, повтор");

        // Агент жив: следующий ход идёт своим чередом.
        assert_eq!(
            stop_reason(&harness.turn("ещё раз").await),
            StopReason::Error
        );
        harness.finish().await;
    }

    /// Переполнение на самой суммаризации — тоже `Error`, без второго круга.
    #[tokio::test]
    async fn overflow_during_summarization_fails_the_turn() {
        let mut harness = Harness::new(
            vec![Script::ContextOverflow],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
        )
        .await;

        let events = harness.turn("вопрос").await;
        assert_eq!(tags(&events), ["turn_started", "error", "turn_finished"]);
        assert_eq!(stop_reason(&events), StopReason::Error);
        assert!(matches!(
            &events[1],
            Event::Error { message } if message.contains("суммаризация не удалась")
        ));
        assert_eq!(harness.provider.calls(), 2, "запрос и суммаризация");
        harness.finish().await;
    }

    /// Ручная компакция вне хода: `Op::Compact` → событие и саммари в истории.
    #[tokio::test]
    async fn manual_compact_op_summarizes_the_history() {
        let mut harness = Harness::new(
            vec![
                text_script_with_usage("первый ответ", Usage::new(4000, 96)),
                text_script("Task: разобраться. Next: продолжить"),
                text_script("второй ответ"),
            ],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
        )
        .await;

        harness.turn("первый вопрос").await;

        harness.submit(Op::Compact);
        let event = harness.next_event().await;
        let (before, after) = compaction(&[event]);
        assert_eq!(before, 4096);
        assert!(before > after, "{before} → {after}");

        let events = harness.turn("второй вопрос").await;
        assert_eq!(stop_reason(&events), StopReason::Completed);
        assert!(
            !tags(&events).contains(&"compacted".to_string()),
            "порог выключен — второй компакции нет"
        );

        let requests = harness.provider.requests();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[1].system, compact::SUMMARY_SYSTEM_PROMPT);
        assert_eq!(requests[2].messages.len(), 2);
        assert!(is_summary(&requests[2].messages[0]));
        assert_eq!(requests[2].messages[1], Message::user("второй вопрос"));
        harness.finish().await;
    }

    /// Пустую историю компактить нечем — `Op::Compact` молча ничего не делает.
    #[tokio::test]
    async fn manual_compact_on_an_empty_history_does_nothing() {
        let mut harness = Harness::new(
            vec![text_script("ответ")],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
        )
        .await;

        harness.submit(Op::Compact);
        let events = harness.turn("вопрос").await;
        assert!(!tags(&events).contains(&"compacted".to_string()));
        assert_eq!(harness.provider.calls(), 1, "суммаризации не было");
        harness.finish().await;
    }

    /// Флаги видимости и append-only сессия: старое остаётся пользователю,
    /// саммари — модели, ничего не переписано.
    #[tokio::test]
    async fn compaction_visibility_flags_survive_the_session_round_trip() {
        let mut harness = Harness::new(
            vec![
                text_script_with_usage("первый ответ", Usage::new(4000, 96)),
                text_script("Task: разобраться. Next: продолжить"),
            ],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
        )
        .await;

        harness.turn("первый вопрос").await;
        harness.submit(Op::Compact);
        let (before, after) = compaction(&[harness.next_event().await]);

        let items = harness.finish().await;

        // Маркер компакции в сессии повторяет событие.
        assert!(items.contains(&SessionItem::Compacted {
            tokens_before: before,
            tokens_after: after,
        }));

        // Сессия дописана, а не переписана: сперва исходные сообщения, следом
        // их скрытые копии и саммари.
        let history = messages(&items);
        assert_eq!(history.len(), 5);
        assert_eq!(history[0], Message::user("первый вопрос"));
        assert_eq!(history[1], Message::assistant("первый ответ"));

        for (original, hidden) in history[..2].iter().zip(&history[2..4]) {
            assert_eq!(original.content, hidden.content, "содержимое то же");
            assert!(!hidden.is_agent_visible(), "модель больше это не видит");
            assert!(hidden.is_user_visible(), "пользователь видит по-прежнему");
        }

        let summary = &history[4];
        assert!(is_summary(summary));
        assert!(summary.text().contains("Next: продолжить"));
        assert!(summary.is_agent_visible(), "саммари — для модели");
        assert!(!summary.is_user_visible(), "и не для пользователя");
    }

    // ─────────────────────── компакция внутри хода ───────────────────────

    /// Единственный ход на пустой истории (как в `-p`): контекст набивается
    /// раундом тулов, и порог срабатывает между итерациями — до следующего
    /// запроса, а не только перед ходом.
    #[tokio::test]
    async fn threshold_compacts_between_iterations_of_a_single_turn() {
        let mut harness = Harness::with_compaction(
            vec![
                tools_script_with_usage(
                    "читаю",
                    &[("call_1", "read", json!({"path": "a.txt"}))],
                    usage_over_threshold(),
                ),
                text_script("Task: прочитать a.txt. Next: ответить"),
                text_script("готово"),
            ],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
            small_context(),
        )
        .await;
        std::fs::write(harness.path("a.txt"), "hello\n").unwrap();

        let events = harness.turn("прочитай a.txt").await;
        assert_eq!(
            tags(&events),
            [
                "turn_started",
                "text_delta",
                "message_complete",
                "usage",
                "tool_call_started",
                "tool_call_finished",
                "compacted",
                "text_delta",
                "message_complete",
                "usage",
                "turn_finished",
            ],
            "компакция — между раундом тулов и следующим запросом"
        );
        assert_eq!(stop_reason(&events), StopReason::Completed);
        let (before, after) = compaction(&events);
        assert_eq!(before, 4096, "оценка из usage первой итерации");
        assert!(before > after, "{before} → {after}");

        let requests = harness.provider.requests();
        assert_eq!(requests.len(), 3, "итерация, суммаризация, итерация");

        // Суммаризация видит раунд целиком: просьбу, tool_use и его результат.
        assert_eq!(requests[1].system, compact::SUMMARY_SYSTEM_PROMPT);
        assert_eq!(requests[1].messages.len(), 4);
        assert_eq!(requests[1].messages[0], Message::user("прочитай a.txt"));
        assert_eq!(requests[1].messages[1].tool_uses().count(), 1);
        assert_eq!(requests[1].messages[2].role, Role::Tool);
        assert_eq!(
            requests[1].messages[3],
            Message::user(compact::SUMMARY_INSTRUCTIONS)
        );
        assert_tool_calls_paired(&requests[1].messages);

        // Следующая итерация — на сжатой истории: только саммари, скрытое
        // (включая tool_use и tool_result) в запрос не попало.
        assert_eq!(requests[2].system, "тестовый системный промпт");
        assert!(!requests[2].tools.is_empty(), "тулы на месте");
        assert_eq!(requests[2].messages.len(), 1);
        assert!(is_summary(&requests[2].messages[0]));
        assert!(requests[2].messages[0].text().contains("Next: ответить"));

        // Сессия: маркер компакции встал между результатом тула и ответом.
        let items = harness.finish().await;
        let position = |wanted: &dyn Fn(&SessionItem) -> bool| {
            items
                .iter()
                .position(wanted)
                .expect("элемента нет в сессии")
        };
        let marker = position(&|item| {
            *item
                == SessionItem::Compacted {
                    tokens_before: before,
                    tokens_after: after,
                }
        });
        let tool_result = position(
            &|item| matches!(item, SessionItem::Message { message } if message.role == Role::Tool),
        );
        let answer = position(
            &|item| matches!(item, SessionItem::Message { message } if *message == Message::assistant("готово")),
        );
        assert!(
            tool_result < marker && marker < answer,
            "{tool_result} < {marker} < {answer}"
        );

        // Append-only: исходный раунд, его скрытые копии, саммари, ответ.
        let history = messages(&items);
        assert_eq!(history.len(), 8);
        for (original, hidden) in history[..3].iter().zip(&history[3..6]) {
            assert_eq!(original.content, hidden.content, "содержимое то же");
            assert!(original.is_agent_visible());
            assert!(!hidden.is_agent_visible(), "модель больше это не видит");
            assert!(hidden.is_user_visible(), "пользователь видит по-прежнему");
        }
        assert_tool_calls_paired(&history[3..6]);
        assert!(is_summary(&history[6]));
        assert_eq!(history[7], Message::assistant("готово"));
    }

    /// Ниже порога раунды тулов идут без компакции, история растёт целиком.
    #[tokio::test]
    async fn tool_rounds_below_the_threshold_do_not_compact() {
        let below = Usage::new(3000, 96);
        let mut harness = Harness::with_compaction(
            vec![
                tools_script_with_usage("", &[("call_1", "read", json!({"path": "a.txt"}))], below),
                tools_script_with_usage("", &[("call_2", "read", json!({"path": "b.txt"}))], below),
                text_script("готово"),
            ],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
            small_context(),
        )
        .await;
        std::fs::write(harness.path("a.txt"), "a\n").unwrap();
        std::fs::write(harness.path("b.txt"), "b\n").unwrap();

        let events = harness.turn("прочитай оба файла").await;
        assert_eq!(stop_reason(&events), StopReason::Completed);
        assert_eq!(compactions(&events), 0);

        let requests = harness.provider.requests();
        assert_eq!(requests.len(), 3, "суммаризации не было");
        assert_eq!(requests[2].messages.len(), 5, "история без потерь");
        assert!(
            requests
                .iter()
                .all(|r| r.system == "тестовый системный промпт")
        );
        assert_tool_calls_paired(&requests[2].messages);
        harness.finish().await;
    }

    /// `-p` с огромным промптом и сервером без счётчиков: первую итерацию
    /// порог не трогает — модель обязана увидеть просьбу как есть, — а между
    /// итерациями срабатывает по байтовой оценке.
    #[tokio::test]
    async fn the_first_iteration_keeps_the_prompt_and_bytes_compact_later() {
        let mut harness = Harness::with_compaction(
            vec![
                tools_script_with_usage(
                    "",
                    &[("call_1", "read", json!({"path": "a.txt"}))],
                    Usage::default(),
                ),
                text_script("саммари"),
                text_script("готово"),
            ],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
            // ~4000 байт ввода ≈ 1000 токенов при пороге 800.
            CompactionConfig::new().with_context_limit(1000),
        )
        .await;
        std::fs::write(harness.path("a.txt"), "a\n").unwrap();

        let prompt = "щ".repeat(2000);
        let events = harness.turn(&prompt).await;
        assert_eq!(stop_reason(&events), StopReason::Completed);
        assert_eq!(compactions(&events), 1);
        let (before, _) = compaction(&events);
        assert!(before > 1000, "байтовая оценка: {before}");

        let requests = harness.provider.requests();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].messages, vec![Message::user(prompt)]);
        assert_eq!(requests[1].system, compact::SUMMARY_SYSTEM_PROMPT);
        assert_eq!(requests[2].messages.len(), 1);
        assert!(is_summary(&requests[2].messages[0]));
        harness.finish().await;
    }

    /// Последняя разрешённая итерация не компактит впустую: запроса после неё
    /// всё равно не будет.
    #[tokio::test]
    async fn no_compaction_after_the_last_allowed_iteration() {
        let mut harness = Harness::with_compaction(
            vec![tools_script_with_usage(
                "",
                &[("call_1", "read", json!({"path": "a.txt"}))],
                usage_over_threshold(),
            )],
            PermissionPolicy::Yolo,
            1,
            small_context(),
        )
        .await;

        let events = harness.turn("прочитай a.txt").await;
        assert_eq!(stop_reason(&events), StopReason::MaxTurns);
        assert_eq!(compactions(&events), 0);
        assert_eq!(harness.provider.calls(), 1, "суммаризации не было");
        harness.finish().await;
    }

    /// Шторм-защита: после компакции порог молчит, пока провайдер не пришлёт
    /// свежий `Usage`, даже если байтовая оценка саммари всё ещё выше порога.
    /// Пришёл `Usage` — порог снова работает.
    #[tokio::test]
    async fn threshold_waits_for_fresh_usage_before_compacting_again() {
        // Саммари больше порога само по себе: ~5000 токенов по байтам.
        let long_summary = format!("Task: длинная задача. {}", "x".repeat(20_000));
        let mut harness = Harness::with_compaction(
            vec![
                tools_script_with_usage(
                    "",
                    &[("call_1", "read", json!({"path": "a.txt"}))],
                    usage_over_threshold(),
                ),
                text_script(&long_summary),
                // Ход 1 падает сразу после компакции — свежего usage нет.
                Script::Fail("сеть легла".into()),
                // Ход 2: порог молчит; ответ приносит свежий usage.
                text_script_with_usage("второй ответ", usage_over_threshold()),
                // Ход 3: порог снова в деле.
                text_script("Task: второе саммари"),
                text_script("третий ответ"),
            ],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
            small_context(),
        )
        .await;
        std::fs::write(harness.path("a.txt"), "a\n").unwrap();

        let events = harness.turn("первый вопрос").await;
        assert_eq!(stop_reason(&events), StopReason::Error);
        assert_eq!(compactions(&events), 1);
        let (_, after) = compaction(&events);
        assert!(
            small_context().should_compact(after),
            "оценка сразу после компакции всё ещё выше порога: {after}"
        );

        let events = harness.turn("второй вопрос").await;
        assert_eq!(stop_reason(&events), StopReason::Completed);
        assert_eq!(
            compactions(&events),
            0,
            "без свежего usage вторая компакция подряд — шторм"
        );

        let events = harness.turn("третий вопрос").await;
        assert_eq!(stop_reason(&events), StopReason::Completed);
        assert_eq!(compactions(&events), 1, "свежий usage снова включил порог");

        let requests = harness.provider.requests();
        assert_eq!(requests.len(), 6);
        let summarizations = requests
            .iter()
            .filter(|r| r.system == compact::SUMMARY_SYSTEM_PROMPT)
            .count();
        assert_eq!(summarizations, 2);
        // Ход 2 ушёл с первым саммари и свежим вводом.
        assert_eq!(requests[3].messages.len(), 2);
        assert!(is_summary(&requests[3].messages[0]));
        assert_eq!(requests[3].messages[1], Message::user("второй вопрос"));
        // Ход 3 — уже со вторым саммари.
        assert_eq!(requests[5].messages.len(), 2);
        assert!(requests[5].messages[0].text().contains("второе саммари"));
        assert_eq!(requests[5].messages[1], Message::user("третий вопрос"));
        harness.finish().await;
    }

    /// Компакция в итерации одна: переполнение сразу после проактивной
    /// компакции не сжимает саммари второй раз, а валит ход.
    #[tokio::test]
    async fn overflow_right_after_an_in_turn_compaction_fails_the_turn() {
        let mut harness = Harness::with_compaction(
            vec![
                tools_script_with_usage(
                    "",
                    &[("call_1", "read", json!({"path": "a.txt"}))],
                    usage_over_threshold(),
                ),
                text_script("саммари"),
                // Повторяется: лишняя компакция и повтор повесили бы тест.
                Script::ContextOverflow,
            ],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
            small_context(),
        )
        .await;
        std::fs::write(harness.path("a.txt"), "a\n").unwrap();

        let events = harness.turn("вопрос").await;
        assert_eq!(
            tags(&events),
            [
                "turn_started",
                "message_complete",
                "usage",
                "tool_call_started",
                "tool_call_finished",
                "compacted",
                "error",
                "turn_finished",
            ]
        );
        assert_eq!(stop_reason(&events), StopReason::Error);
        assert!(events.iter().any(|event| matches!(
            event,
            Event::Error { message } if message.contains("контекст модели переполнен")
        )));
        assert_eq!(
            harness.provider.calls(),
            3,
            "итерация, суммаризация, повтор"
        );
        harness.finish().await;
    }

    /// Неудачная компакция между итерациями хода не валит: ошибка наружу,
    /// история не тронута, следующий запрос уходит как есть.
    #[tokio::test]
    async fn a_failed_in_turn_compaction_is_reported_and_the_turn_goes_on() {
        let mut harness = Harness::with_compaction(
            vec![
                tools_script_with_usage(
                    "",
                    &[("call_1", "read", json!({"path": "a.txt"}))],
                    usage_over_threshold(),
                ),
                // Пустое саммари — заменять историю нечем.
                text_script("   "),
                text_script("готово"),
            ],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
            small_context(),
        )
        .await;
        std::fs::write(harness.path("a.txt"), "a\n").unwrap();

        let events = harness.turn("вопрос").await;
        assert_eq!(
            tags(&events),
            [
                "turn_started",
                "message_complete",
                "usage",
                "tool_call_started",
                "tool_call_finished",
                "error",
                "text_delta",
                "message_complete",
                "usage",
                "turn_finished",
            ]
        );
        assert_eq!(stop_reason(&events), StopReason::Completed);
        assert!(events.iter().any(|event| matches!(
            event,
            Event::Error { message } if message.contains("пустое саммари")
        )));

        let requests = harness.provider.requests();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[2].messages.len(), 3, "история не тронута");
        assert_tool_calls_paired(&requests[2].messages);
        harness.finish().await;
    }

    /// Прерывание посреди компакции между итерациями: ход `Cancelled`,
    /// история не тронута, пары тул-коллов целы.
    #[tokio::test]
    async fn interrupt_during_an_in_turn_compaction_cancels_the_turn() {
        let mut harness = Harness::with_compaction(
            vec![
                tools_script_with_usage(
                    "",
                    &[("call_1", "read", json!({"path": "a.txt"}))],
                    usage_over_threshold(),
                ),
                // Суммаризатор «думает» вечно.
                Script::Hang(Vec::new()),
            ],
            PermissionPolicy::Yolo,
            DEFAULT_MAX_TURNS,
            small_context(),
        )
        .await;
        std::fs::write(harness.path("a.txt"), "a\n").unwrap();

        harness.submit(Op::user_input("вопрос"));
        // Прерываем именно компакцию: ждём, пока суммаризатор примет запрос.
        tokio::time::timeout(EVENT_TIMEOUT, async {
            while harness.provider.calls() < 2 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("суммаризация не началась");
        assert_eq!(
            harness.provider.requests()[1].system,
            compact::SUMMARY_SYSTEM_PROMPT
        );
        harness.submit(Op::Interrupt);
        let events = harness.drain_turn().await;

        assert_eq!(
            tags(&events),
            [
                "turn_started",
                "message_complete",
                "usage",
                "tool_call_started",
                "tool_call_finished",
                "turn_finished",
            ]
        );
        assert_eq!(stop_reason(&events), StopReason::Cancelled);

        let items = harness.finish().await;
        assert!(
            !items
                .iter()
                .any(|item| matches!(item, SessionItem::Compacted { .. }))
        );
        let history = messages(&items);
        assert_eq!(history.len(), 3, "просьба, tool_use, tool_result");
        assert!(history.iter().all(Message::is_agent_visible));
        assert_tool_calls_paired(&history);
    }

    #[test]
    fn config_defaults_and_builders() {
        let config = AgentConfig::default();
        assert_eq!(config.max_turns, DEFAULT_MAX_TURNS);
        assert_eq!(config.policy, PermissionPolicy::AskWrites);
        assert!(config.max_tokens.is_none());
        assert!(config.temperature.is_none());
        assert_eq!(config.compaction, CompactionConfig::default());
        assert_eq!(config.loop_detection, DEFAULT_LOOP_DETECTION);
        assert_eq!(DEFAULT_LOOP_DETECTION, 3);
        assert_eq!(config.write_churn, DEFAULT_WRITE_CHURN);
        assert_eq!(DEFAULT_WRITE_CHURN, 3);

        let config = AgentConfig::new("qwen3-coder")
            .with_cwd(Path::new("/work"))
            .with_policy(PermissionPolicy::Yolo)
            .with_max_turns(7)
            .with_max_tokens(2048)
            .with_temperature(0.2)
            .with_compaction(CompactionConfig::new().with_context_limit(32_768))
            .with_loop_detection(0)
            .with_write_churn(5);
        assert_eq!(config.loop_detection, 0);
        assert_eq!(config.write_churn, 5);
        assert_eq!(config.model, "qwen3-coder");
        assert_eq!(config.cwd, PathBuf::from("/work"));
        assert_eq!(config.policy, PermissionPolicy::Yolo);
        assert_eq!(config.max_turns, 7);
        assert_eq!(config.max_tokens, Some(2048));
        assert_eq!(config.temperature, Some(0.2));
        assert_eq!(config.compaction.context_limit, Some(32_768));
        assert_eq!(config.compaction.threshold, compact::DEFAULT_THRESHOLD);
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
