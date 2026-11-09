//! TUI: полноэкранный чат поверх Event-канала ядра — второй потребитель
//! SQ/EQ после CLI.
//!
//! Запуск — `mark2` без PROMPT в терминале (stdin и stdout — TTY) или
//! `mark2 --tui [PROMPT]`. Конфиг, провайдер, реестр тулов и JSONL-сессия —
//! тот же путь сборки, что у print и interactive mode (`main.rs`); TUI
//! получает готовые [`AgentHandle`] и приёмник [`Event`].
//!
//! Устройство:
//!
//! * [`app`] — состояние и его обновление: чистые функции от [`Event`] ядра
//!   и событий терминала, на выходе — [`Effect`] (отправить [`Op`], выйти);
//! * [`ui`] — отрисовка состояния ratatui-виджетами;
//! * [`input`] — поле ввода; [`wrap`] — перенос и санитария текста;
//! * [`terminal`] — raw mode / alternate screen и их восстановление на
//!   всех путях выхода, включая панику;
//! * [`run`] — цикл: `select!` по событиям ядра, клавишам, таймеру спиннера
//!   и сигналам; после каждой пачки событий — кадр.
//!
//! Клавиши: Enter — отправить; Alt+Enter или Ctrl+J — перевод строки;
//! Esc или Ctrl+C во время хода — [`Op::Interrupt`]; Ctrl+C на пустом поле
//! вне хода, Ctrl+C дважды подряд или Ctrl+D на пустом поле — выход;
//! PgUp/PgDn и колесо — лента (у хвоста лента снова следует за стримом);
//! Ctrl+T — показать/скрыть рассуждения. В модалке подтверждения: `y` —
//! разрешить, `n`/Esc — запретить, Ctrl+C — прервать ход, ↑↓/PgUp/PgDn —
//! листать длинный вопрос.
//!
//! Многоходовость: цикл ядра ([`mark2_core::Agent::spawn`]) после
//! `TurnFinished` ждёт следующий [`Op::UserInput`], история копится. Ввод
//! посреди хода TUI не отдаёт ядру сразу, а держит в своей очереди до
//! `TurnFinished` — почему, см. [`App::submit_text`].
//!
//! Логи `tracing` пишутся в stderr, а stderr в TUI — тот же экран. Поэтому
//! при stderr-терминале `main` их не включает; нужны логи —
//! `RUST_LOG=mark2=debug mark2 2>mark2.log`.

mod app;
mod input;
mod terminal;
mod ui;
mod wrap;

use std::io::Stdout;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use crossterm::event::{Event as TermEvent, EventStream, KeyEventKind, MouseEventKind};
use futures::StreamExt;
use mark2_core::message::Usage;
use mark2_core::{AgentHandle, Event};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::MissedTickBehavior;

pub use app::{App, Effect, NoticeKind};
use terminal::TerminalGuard;

/// Период кадров спиннера, пока идёт ход.
const TICK: Duration = Duration::from_millis(120);

/// Сколько уже пришедших событий ядра обрабатывается до перерисовки: дельты
/// стрима сыплются пачками, и кадр на каждую был бы пустой тратой.
const MAX_BATCH: usize = 512;

/// С чем запускается TUI.
#[derive(Debug, Clone)]
pub struct Options {
    /// Модель — для статус-бара.
    pub model: String,
    /// Рабочая директория агента — для статус-бара.
    pub cwd: String,
    /// Показывать рассуждения с самого начала (`--show-thinking`).
    pub show_thinking: bool,
    /// Первое сообщение, отправляемое сразу (`mark2 --tui "задача"`).
    pub prompt: Option<String>,
    /// Служебные строки в начало ленты (куда пишется сессия и т.п.).
    pub notes: Vec<(NoticeKind, String)>,
}

/// Итог сессии TUI — для строки в stderr после выхода.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Расход токенов за всю сессию.
    pub usage: Usage,
    /// Сколько тул-коллов спасло приведение аргументов.
    pub coerced: usize,
}

/// Путь для статус-бара: `$HOME` → `~`.
pub fn display_path(path: &Path) -> String {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    match home
        .as_deref()
        .and_then(|home| path.strip_prefix(home).ok())
    {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// Прогнать TUI до выхода пользователя.
///
/// Терминал возвращается в обычный режим до возврата из функции — и при
/// успехе, и при ошибке (см. [`terminal`]).
pub async fn run(
    handle: &AgentHandle,
    events: &mut UnboundedReceiver<Event>,
    options: Options,
) -> Result<Outcome> {
    let mut app = App::new(options.model, options.cwd, options.show_thinking);
    for (kind, text) in options.notes {
        app.notice(kind, text);
    }
    let initial = options
        .prompt
        .map(|prompt| app.submit_text(prompt))
        .unwrap_or_default();

    let guard = TerminalGuard::enter().context("не удалось включить полноэкранный режим")?;
    let result = match Terminal::new(CrosstermBackend::new(std::io::stdout())) {
        Ok(mut terminal) => event_loop(&mut terminal, &mut app, handle, events, initial).await,
        Err(error) => Err(error).context("не удалось открыть терминал"),
    };
    drop(guard);
    result?;
    Ok(Outcome {
        usage: app.session_usage,
        coerced: app.session_coerced,
    })
}

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    handle: &AgentHandle,
    events: &mut UnboundedReceiver<Event>,
    initial: Vec<Effect>,
) -> Result<()> {
    if apply(handle, initial) {
        return Ok(());
    }
    let mut keys = EventStream::new();
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);

    loop {
        if !terminal::is_active() {
            bail!("терминал восстановлен аварийно (паника в другом потоке)");
        }
        terminal
            .draw(|frame| ui::draw(frame, app))
            .context("не удалось нарисовать кадр")?;

        let effects = tokio::select! {
            event = events.recv() => {
                let Some(event) = event else {
                    bail!("agent loop остановился неожиданно");
                };
                let now = Instant::now();
                let mut effects = app.on_agent_event(event, now);
                for _ in 0..MAX_BATCH {
                    match events.try_recv() {
                        Ok(event) => effects.extend(app.on_agent_event(event, now)),
                        Err(_) => break,
                    }
                }
                effects
            }
            input = keys.next() => match input {
                Some(Ok(event)) => {
                    if !needs_handling(&event) {
                        continue;
                    }
                    app.on_terminal_event(event, Instant::now())
                }
                Some(Err(error)) => return Err(error).context("не удалось прочитать терминал"),
                None => vec![Effect::Quit],
            },
            _ = tick.tick(), if app.busy => {
                app.on_tick();
                Vec::new()
            }
            () = &mut shutdown => vec![Effect::Quit],
        };
        if apply(handle, effects) {
            return Ok(());
        }
    }
}

/// Исполнить эффекты; `true` — пора выходить.
fn apply(handle: &AgentHandle, effects: Vec<Effect>) -> bool {
    for effect in effects {
        match effect {
            Effect::Submit(op) => handle.submit(op),
            Effect::Quit => return true,
        }
    }
    false
}

/// Стоит ли событие терминала обработки и кадра: движения мыши (захват
/// включён ради колеса, а с ним приходят и они), фокус и отпускания клавиш
/// — нет.
fn needs_handling(event: &TermEvent) -> bool {
    match event {
        TermEvent::Mouse(mouse) => matches!(
            mouse.kind,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        ),
        TermEvent::Key(key) => key.kind != KeyEventKind::Release,
        TermEvent::FocusGained | TermEvent::FocusLost => false,
        TermEvent::Paste(_) | TermEvent::Resize(..) => true,
    }
}

/// SIGTERM, SIGHUP или SIGINT: выйти штатно, восстановив терминал. В raw
/// mode Ctrl+C приходит клавишей, а не сигналом, — SIGINT бывает только
/// снаружи (`kill -INT`).
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut streams = Vec::new();
        for kind in [
            SignalKind::terminate(),
            SignalKind::hangup(),
            SignalKind::interrupt(),
        ] {
            match signal(kind) {
                Ok(stream) => streams.push(stream),
                Err(error) => tracing::warn!(%error, "не удалось подписаться на сигнал"),
            }
        }
        if !streams.is_empty() {
            let waits = streams.iter_mut().map(|stream| Box::pin(stream.recv()));
            futures::future::select_all(waits).await;
            return;
        }
    }
    std::future::pending::<()>().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
    use mark2_core::message::{ContentBlock, Message, Role};
    use mark2_core::provider::{
        MessageStream, Provider, ProviderError, ProviderRequest, StreamChunk,
    };
    use mark2_core::tools::ToolRegistry;
    use mark2_core::{Agent, AgentConfig, StopReason};
    use tokio::sync::Notify;

    use super::app::Entry;

    /// Провайдер со скриптом ответов; ответ может ждать «ворота» — так ход
    /// держится открытым, пока тест набирает ввод.
    struct ScriptedProvider {
        script: Mutex<VecDeque<(Option<Arc<Notify>>, String)>>,
        seen: Mutex<Vec<ProviderRequest>>,
    }

    #[async_trait]
    impl Provider for ScriptedProvider {
        fn name(&self) -> &str {
            "scripted"
        }

        async fn stream(&self, req: ProviderRequest) -> Result<MessageStream, ProviderError> {
            self.seen.lock().unwrap().push(req);
            let (gate, text) = self
                .script
                .lock()
                .unwrap()
                .pop_front()
                .expect("скрипт провайдера кончился");
            let chunks = vec![
                StreamChunk::TextDelta(text.clone()),
                StreamChunk::Complete {
                    message: Message::new(Role::Assistant, vec![ContentBlock::text(&text)]),
                    usage: Usage::new(10, 2),
                },
            ];
            let wait = futures::stream::once(async move {
                if let Some(gate) = gate {
                    gate.notified().await;
                }
            })
            .filter_map(|()| futures::future::ready(None::<Result<StreamChunk, ProviderError>>));
            Ok(Box::pin(
                wait.chain(futures::stream::iter(chunks.into_iter().map(Ok))),
            ))
        }
    }

    type Script<'a> = Vec<(Option<Arc<Notify>>, &'a str)>;

    /// Агент на скриптованном провайдере; `cwd` — пустая временная
    /// директория (живёт, пока жив третий элемент).
    fn agent(script: Script) -> (Agent, Arc<ScriptedProvider>, tempfile::TempDir) {
        let provider = Arc::new(ScriptedProvider {
            script: Mutex::new(
                script
                    .into_iter()
                    .map(|(gate, text)| (gate, text.to_string()))
                    .collect(),
            ),
            seen: Mutex::new(Vec::new()),
        });
        let dir = tempfile::tempdir().unwrap();
        let agent = Agent::new(
            Arc::clone(&provider) as Arc<dyn Provider>,
            ToolRegistry::new(),
            AgentConfig::new("mock").with_cwd(dir.path()),
        )
        // Системный промпт фиксирован: иначе тест зависел бы от AGENTS.md
        // выше по дереву.
        .with_system_prompt("test");
        (agent, provider, dir)
    }

    /// Разобрать события ядра до `TurnFinished`, исполняя эффекты — как цикл
    /// TUI, только без терминала.
    async fn pump_turn(
        app: &mut App,
        handle: &AgentHandle,
        events: &mut UnboundedReceiver<Event>,
    ) -> StopReason {
        loop {
            let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .expect("ядро молчит")
                .expect("канал событий закрыт");
            let reason = match &event {
                Event::TurnFinished { reason } => Some(*reason),
                _ => None,
            };
            assert!(!apply(handle, app.on_agent_event(event, Instant::now())));
            if let Some(reason) = reason {
                return reason;
            }
        }
    }

    fn enter(app: &mut App, text: &str) -> Vec<Effect> {
        app.input.insert_str(text);
        app.on_terminal_event(
            TermEvent::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Instant::now(),
        )
    }

    fn answers(app: &App) -> Vec<&str> {
        app.entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Assistant(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Живое ядро: после `TurnFinished` цикл принимает следующий
    /// `UserInput`, и второй запрос к модели несёт историю первого хода.
    #[tokio::test]
    async fn the_core_loop_takes_a_second_turn_after_turn_finished() {
        let (agent, provider, _dir) = agent(vec![(None, "ответ 1"), (None, "ответ 2")]);
        let (handle, mut events) = agent.spawn();
        let mut app = App::new("mock", "/tmp", false);

        assert!(!apply(&handle, enter(&mut app, "первый")));
        assert_eq!(
            pump_turn(&mut app, &handle, &mut events).await,
            StopReason::Completed
        );
        assert!(!app.busy);

        assert!(!apply(&handle, enter(&mut app, "второй")));
        assert_eq!(
            pump_turn(&mut app, &handle, &mut events).await,
            StopReason::Completed
        );
        assert_eq!(answers(&app), ["ответ 1", "ответ 2"]);

        let requests = provider.seen.lock().unwrap().clone();
        assert_eq!(requests.len(), 2);
        let history: Vec<String> = requests[1]
            .messages
            .iter()
            .map(|message| message.text())
            .collect();
        assert_eq!(history, ["первый", "ответ 1", "второй"]);
        handle.shutdown().await;
    }

    /// Живое ядро: ввод посреди хода ждёт в очереди TUI и уходит отдельным
    /// ходом после `TurnFinished` — модель видит его после ответа на первый.
    #[tokio::test]
    async fn input_typed_mid_turn_goes_to_the_core_after_turn_finished() {
        let gate = Arc::new(Notify::new());
        let (agent, provider, _dir) = agent(vec![(Some(Arc::clone(&gate)), "долго"), (None, "ок")]);
        let (handle, mut events) = agent.spawn();
        let mut app = App::new("mock", "/tmp", false);

        assert!(!apply(&handle, enter(&mut app, "долгая задача")));
        // Ход начался и висит на воротах провайдера.
        let started = events.recv().await.expect("TurnStarted");
        assert!(matches!(started, Event::TurnStarted { .. }));
        app.on_agent_event(started, Instant::now());

        assert!(enter(&mut app, "уточнение").is_empty());
        assert_eq!(app.queue, ["уточнение"]);

        gate.notify_one();
        assert_eq!(
            pump_turn(&mut app, &handle, &mut events).await,
            StopReason::Completed
        );
        // `TurnFinished` отправил очередь — второй ход уже идёт.
        assert!(app.queue.is_empty());
        assert!(app.busy);
        assert_eq!(
            pump_turn(&mut app, &handle, &mut events).await,
            StopReason::Completed
        );
        assert_eq!(answers(&app), ["долго", "ок"]);

        let requests = provider.seen.lock().unwrap().clone();
        let history: Vec<String> = requests[1]
            .messages
            .iter()
            .map(|message| message.text())
            .collect();
        assert_eq!(history, ["долгая задача", "долго", "уточнение"]);
        handle.shutdown().await;
    }

    #[test]
    fn mouse_motion_focus_and_key_release_skip_the_redraw() {
        let mouse = |kind| {
            TermEvent::Mouse(MouseEvent {
                kind,
                column: 1,
                row: 1,
                modifiers: KeyModifiers::NONE,
            })
        };
        assert!(needs_handling(&mouse(MouseEventKind::ScrollUp)));
        assert!(needs_handling(&mouse(MouseEventKind::ScrollDown)));
        assert!(!needs_handling(&mouse(MouseEventKind::Moved)));
        assert!(!needs_handling(&TermEvent::FocusLost));
        assert!(needs_handling(&TermEvent::Resize(80, 24)));
        let mut release = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        assert!(!needs_handling(&TermEvent::Key(release)));
    }

    #[test]
    fn home_is_shortened_to_a_tilde() {
        let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
            return;
        };
        assert_eq!(display_path(&home), "~");
        assert_eq!(display_path(&home.join("work/app")), "~/work/app");
        assert_eq!(display_path(Path::new("/opt/x")), "/opt/x");
    }

    #[tokio::test]
    async fn apply_submits_until_quit() {
        let (agent, _provider, _dir) = agent(vec![]);
        let (handle, _events) = agent.spawn();
        assert!(!apply(&handle, vec![]));
        assert!(apply(
            &handle,
            vec![Effect::Submit(mark2_core::Op::Interrupt), Effect::Quit]
        ));
        handle.shutdown().await;
    }
}
