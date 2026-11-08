//! `mark2` — headless CLI поверх [`mark2_core`].
//!
//! Здесь живёт только сборка: разобрать флаги, сложить слои конфига, поднять
//! провайдера, реестр тулов, сессию и агента — и отдать поток [`Event`] одному
//! из двух фронтендов ([`interactive`] или [`print`]). Никакой логики хода в
//! CLI нет и быть не должно: он первый потребитель SQ/EQ-канала, а не его
//! часть.
//!
//! Слои настроек складываются по возрастанию приоритета — пользовательский
//! конфиг, ближайший `.mark2.toml`, флаги, — причём флаги превращаются в такой
//! же [`Config`] и мёржатся последними ([`Args::overrides`]).
//!
//! Коды возврата: `0` — ход завершён, `1` — ошибка или упёрлись в `max_turns`,
//! `2` — ошибка в аргументах (clap), `130` — прервано пользователем (как
//! принято у Ctrl+C в шелле).

mod interactive;
mod print;
mod render;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser, ValueEnum};
use mark2_core::config::SEARXNG_URL_ENV;
use mark2_core::provider::Provider;
use mark2_core::session::{SessionMeta, SessionRecorder, default_base_dir};
use mark2_core::tools::{EditStrategy, ToolRegistry};
use mark2_core::{
    Agent, AgentConfig, AgentSettings, Config, Memory, ModelSettings, Op, PermissionPolicy,
    ProviderConfig, StopReason, WebConfig,
};
use mark2_providers::openai_compat::{CompatOptions, OpenAiCompatProvider, PROVIDER_NAME};

use crate::render::RenderOptions;

/// Значение `PROMPT`, означающее «прочитать промпт из stdin до EOF».
pub const STDIN_PROMPT: &str = "-";

/// Агент для локальных моделей: одна задача — один запуск.
#[derive(Debug, Clone, Parser)]
#[command(
    name = "mark2",
    version,
    about = "Агент для локальных моделей (llama.cpp, ollama, LM Studio) и любого OpenAI-совместимого API",
    after_help = "Конфиг (по возрастанию приоритета): ~/.config/mark2/config.toml, ближайший \
                  ./.mark2.toml, флаги.\nСессии пишутся в ~/.mark2/sessions/YYYY/MM/DD/*.jsonl."
)]
pub struct Args {
    /// Задача агенту; `-` — прочитать её из stdin до EOF (только с -p).
    #[arg(value_name = "PROMPT")]
    pub prompt: String,

    /// Идентификатор модели в терминах сервера.
    #[arg(long, value_name = "ID")]
    pub model: Option<String>,

    /// База OpenAI-совместимого API.
    #[arg(long, value_name = "URL")]
    pub base_url: Option<String>,

    /// Переменная окружения, из которой брать API-ключ.
    #[arg(long, value_name = "VAR")]
    pub api_key_env: Option<String>,

    /// Ничего не спрашивать: тулы выполняются без подтверждения.
    #[arg(long, conflicts_with = "policy")]
    pub yolo: bool,

    /// Политика доступа к тулам.
    #[arg(long, value_name = "POLICY", value_enum)]
    pub policy: Option<PolicyArg>,

    /// Максимум обращений к модели за ход.
    #[arg(long, value_name = "N")]
    pub max_turns: Option<usize>,

    /// Температура сэмплинга, от 0 до 2; 0 — самый воспроизводимый ответ.
    // allow_negative_numbers: `--temperature -1` — значение вне диапазона, а
    // не «неизвестный флаг -1».
    #[arg(long, value_name = "T", value_parser = parse_temperature, allow_negative_numbers = true)]
    pub temperature: Option<f32>,

    /// Print mode: только ответ модели в stdout, вопросов не задаём.
    #[arg(short = 'p', long)]
    pub print: bool,

    /// Не писать сессию в ~/.mark2/sessions.
    #[arg(long)]
    pub no_session: bool,

    /// Рабочая директория агента.
    #[arg(short = 'C', long, value_name = "DIR")]
    pub cwd: Option<PathBuf>,

    /// Показывать рассуждения модели (приглушённо).
    #[arg(long)]
    pub show_thinking: bool,
}

/// Политика доступа в терминах командной строки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PolicyArg {
    /// Ничего не спрашивать.
    Yolo,
    /// Спрашивать перед bash/edit/write и сетевыми web_fetch/web_search.
    AskWrites,
    /// Только чтение.
    ReadOnly,
}

/// Какую температуру принимает `--temperature`: OpenAI-совместимые API выше 2
/// запрос отвергают, а отрицательная бессмысленна.
const TEMPERATURE_RANGE: std::ops::RangeInclusive<f32> = 0.0..=2.0;

/// Разбор `--temperature`. Ошибка становится обычной ошибкой clap (код 2) — до
/// запроса к модели, а не отказом сервера посреди хода. `nan` и `inf` `f32`
/// разбирает, но в диапазон они не входят.
fn parse_temperature(value: &str) -> Result<f32, String> {
    match value.parse::<f32>() {
        Ok(temperature) if TEMPERATURE_RANGE.contains(&temperature) => Ok(temperature),
        _ => Err("ожидается число от 0 до 2 включительно".to_string()),
    }
}

impl From<PolicyArg> for PermissionPolicy {
    fn from(policy: PolicyArg) -> Self {
        match policy {
            PolicyArg::Yolo => PermissionPolicy::Yolo,
            PolicyArg::AskWrites => PermissionPolicy::AskWrites,
            PolicyArg::ReadOnly => PermissionPolicy::ReadOnly,
        }
    }
}

impl Args {
    /// Разобрать argv и проверить то, чего clap не выражает декларативно.
    ///
    /// Ошибка — обычная ошибка clap: `main` печатает её с usage и выходит с
    /// кодом 2, как при любой опечатке во флагах.
    pub fn try_parse_and_validate<I, T>(argv: I) -> Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        let args = Self::try_parse_from(argv)?;
        args.validate()?;
        Ok(args)
    }

    /// `-` вместо промпта — только в print mode: интерактивный режим читает
    /// stdin сам (подтверждения `y/N`), и делить его с промптом нечестно.
    fn validate(&self) -> Result<(), clap::Error> {
        if self.prompt == STDIN_PROMPT && !self.print {
            return Err(Self::command().error(
                clap::error::ErrorKind::MissingRequiredArgument,
                "PROMPT `-` (промпт из stdin) работает только вместе с -p: \
                 интерактивный режим читает stdin сам",
            ));
        }
        Ok(())
    }

    /// Промпт хода: позиционный аргумент как есть, а для `-` — весь stdin.
    fn resolve_prompt(&self, stdin: &mut dyn Read) -> Result<String> {
        if self.prompt == STDIN_PROMPT {
            read_prompt(stdin)
        } else {
            Ok(self.prompt.clone())
        }
    }

    /// Флаги как слой конфига — старший из трёх.
    pub fn overrides(&self) -> Config {
        Config {
            provider: ProviderConfig {
                base_url: self.base_url.clone(),
                api_key_env: self.api_key_env.clone(),
                cache_prompt: None,
                slot_id: None,
            },
            agent: AgentSettings {
                model: self.model.clone(),
                max_turns: self.max_turns,
                policy: self.policy_override(),
                max_tokens: None,
                temperature: self.temperature,
                loop_detection: None,
                write_churn: None,
                arg_coercion: None,
                memory_file: None,
                post_edit_command: None,
                post_edit_timeout_seconds: None,
            },
            web: WebConfig::default(),
            models: BTreeMap::new(),
        }
    }

    /// Политика из флагов: `--yolo` — короткая форма `--policy yolo`.
    fn policy_override(&self) -> Option<PermissionPolicy> {
        if self.yolo {
            return Some(PermissionPolicy::Yolo);
        }
        self.policy.map(PermissionPolicy::from)
    }

    /// Рабочая директория агента: `-C` или текущая.
    fn resolve_cwd(&self) -> Result<PathBuf> {
        let cwd = match &self.cwd {
            Some(dir) => dir.clone(),
            None => std::env::current_dir().context("не удалось определить текущую директорию")?,
        };
        let cwd = cwd
            .canonicalize()
            .with_context(|| format!("рабочая директория {} недоступна", cwd.display()))?;
        if !cwd.is_dir() {
            bail!("{} — не директория", cwd.display());
        }
        Ok(cwd)
    }

    /// Как рендерить поток событий.
    fn render_options(&self) -> RenderOptions {
        if self.print {
            RenderOptions::print()
        } else {
            RenderOptions::interactive(self.show_thinking, use_color())
        }
    }
}

/// Прочитать промпт до EOF (`mark2 -p -`).
///
/// Большой промпт так не упирается в лимит argv (на Linux — 128 КиБ на один
/// аргумент) и не требует экранирования. Текст уходит модели байт-в-байт;
/// пустой или из одних пробельных символов — ошибка: такой ход модель
/// потратила бы на догадки, что от неё хотят.
fn read_prompt(reader: &mut dyn Read) -> Result<String> {
    let mut prompt = String::new();
    reader
        .read_to_string(&mut prompt)
        .context("не удалось прочитать промпт из stdin (ожидается UTF-8)")?;
    if prompt.trim().is_empty() {
        bail!("empty prompt: stdin пуст, а `-p -` берёт промпт из stdin до EOF");
    }
    Ok(prompt)
}

/// Раскрашивать ли вывод: только в терминал и только если не запрещено
/// `NO_COLOR` (https://no-color.org).
fn use_color() -> bool {
    std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

/// Код возврата по причине завершения хода.
fn exit_code(reason: StopReason) -> ExitCode {
    match reason {
        StopReason::Completed => ExitCode::SUCCESS,
        // Упереться в предохранитель — тоже не «успех»: задача не доделана.
        StopReason::Error | StopReason::MaxTurns => ExitCode::FAILURE,
        StopReason::Cancelled => ExitCode::from(130),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();
    let args =
        Args::try_parse_and_validate(std::env::args_os()).unwrap_or_else(|error| error.exit());
    let opts = args.render_options();
    match run(args).await {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{}", render::error_line(&format!("{error:#}"), opts));
            ExitCode::FAILURE
        }
    }
}

/// Логи — в stderr и только по запросу: `RUST_LOG=mark2=debug`.
fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .without_time()
        .try_init();
}

/// Собрать агента из флагов и конфига и прогнать один ход.
async fn run(args: Args) -> Result<ExitCode> {
    let cwd = args.resolve_cwd()?;
    let config = Config::load(&cwd)?.merge(args.overrides());
    let opts = args.render_options();

    let model = config.agent.model.clone().context(
        "не задана модель: укажи --model <id> или model = \"<id>\" в секции [agent] конфига \
         (~/.config/mark2/config.toml или ./.mark2.toml)",
    )?;
    let settings = config.resolve(&model);
    tracing::debug!(
        %model,
        edit_strategy = ?settings.edit_strategy,
        context_limit = ?settings.context_limit,
        "настройки модели"
    );
    // До сессии и агента: пустой stdin не должен оставлять за собой журнал.
    // Блокирующее чтение здесь безвредно — больше в рантайме ничего не идёт.
    let prompt = args.resolve_prompt(&mut std::io::stdin().lock())?;

    let provider = OpenAiCompatProvider::new(
        config.base_url(),
        api_key(&config),
        CompatOptions {
            cache_prompt: config.provider.cache_prompt.unwrap_or(false),
            slot_id: config.provider.slot_id,
        },
    );

    let registry = tool_registry(
        settings.edit_strategy,
        config.agent.arg_coercion.unwrap_or(true),
        config.web.fetch_enabled(),
        config.search_base_url().as_deref(),
    )?;
    let mut agent = Agent::new(
        Arc::new(provider) as Arc<dyn Provider>,
        registry,
        agent_config(&config, &model, &cwd, settings),
    );
    if let Some(memory) = agent.memory() {
        tracing::debug!(
            path = %memory.path.display(),
            bytes = memory.injected_bytes(),
            truncated = memory.truncated,
            "память в системном промпте"
        );
    }
    if !args.no_session
        && let Some(recorder) = open_session(session_meta(&cwd, &model, agent.memory()), opts).await
    {
        tracing::debug!(path = %recorder.path().display(), "сессия пишется");
        agent = agent.with_recorder(recorder);
    }

    let (handle, mut events) = agent.spawn();
    handle.submit(Op::user_input(prompt));

    let reason = if args.print {
        let mut stdout = std::io::stdout();
        let mut stderr = std::io::stderr();
        print::run(&handle, &mut events, &mut stdout, &mut stderr).await
    } else {
        interactive::run(&handle, &mut events, opts).await
    };

    // Ждём, пока агент закроет файл сессии.
    handle.shutdown().await;
    Ok(exit_code(reason))
}

/// Реестр тулов: базовые (без `web_fetch` при `web_fetch == false` —
/// `[web] fetch = false`) плюс `web_search`, если задан адрес SearXNG
/// (`search_base_url` — уже с учётом [`SEARXNG_URL_ENV`]). Битый адрес —
/// ошибка запуска, а не тул, который на каждый вызов отвечает ошибкой.
fn tool_registry(
    edit_strategy: EditStrategy,
    arg_coercion: bool,
    web_fetch: bool,
    search_base_url: Option<&str>,
) -> Result<ToolRegistry> {
    let mut registry = ToolRegistry::with_defaults(edit_strategy).with_arg_coercion(arg_coercion);
    if !web_fetch {
        registry = registry.without_web_fetch();
    }
    registry.with_web_search(search_base_url).with_context(|| {
        format!("web_search: проверь [web] search_base_url в конфиге или {SEARXNG_URL_ENV}")
    })
}

/// Настройки агента из сложенных слоёв конфига; чего слои не сказали — дефолт
/// ядра.
fn agent_config(config: &Config, model: &str, cwd: &Path, settings: ModelSettings) -> AgentConfig {
    let mut agent_config = AgentConfig::new(model)
        .with_cwd(cwd)
        .with_policy(config.agent.policy.unwrap_or_default());
    if let Some(max_turns) = config.agent.max_turns {
        agent_config.max_turns = max_turns;
    }
    if let Some(threshold) = config.agent.loop_detection {
        agent_config.loop_detection = threshold;
    }
    if let Some(threshold) = config.agent.write_churn {
        agent_config.write_churn = threshold;
    }
    agent_config.max_tokens = config.agent.max_tokens;
    agent_config.temperature = config.agent.temperature;
    // Относительный путь уже развёрнут от директории своего конфиг-файла.
    agent_config.memory_file = config.agent.memory_file.clone();
    // Нет команды ни в одном слое (или старший её погасил пустой строкой) —
    // сенсора нет вовсе.
    agent_config.post_edit = config.agent.post_edit_check();
    if let Some(limit) = settings.context_limit {
        agent_config = agent_config
            .with_compaction(mark2_core::CompactionConfig::new().with_context_limit(limit));
    }
    agent_config
}

/// API-ключ из переменной окружения, названной в конфиге.
///
/// Ключ никогда не читается из самого конфига: `.mark2.toml` лежит в
/// репозитории проекта, а секреты — нет.
fn api_key(config: &Config) -> Option<String> {
    let variable = config.provider.api_key_env.as_deref()?;
    match std::env::var(variable) {
        Ok(key) if !key.trim().is_empty() => Some(key),
        _ => {
            tracing::warn!(%variable, "переменная с API-ключом пуста или не задана");
            None
        }
    }
}

/// Шапка сессии: где, какой моделью и с какой памятью в промпте.
fn session_meta(cwd: &Path, model: &str, memory: Option<&Memory>) -> SessionMeta {
    let meta = SessionMeta::new(cwd.display().to_string(), model, PROVIDER_NAME);
    match memory {
        Some(memory) => meta.with_memory(
            memory.path.display().to_string(),
            memory.injected_bytes() as u64,
        ),
        None => meta,
    }
}

/// Открыть сессию на запись. Не смогли — работаем без журнала: потеря записи
/// не повод не выполнить задачу.
async fn open_session(meta: SessionMeta, opts: RenderOptions) -> Option<SessionRecorder> {
    let base_dir = match default_base_dir() {
        Ok(dir) => dir,
        Err(error) => {
            warn_no_session(&error.to_string(), opts);
            return None;
        }
    };
    match SessionRecorder::create(&base_dir, meta).await {
        Ok(recorder) => Some(recorder),
        Err(error) => {
            warn_no_session(&error.to_string(), opts);
            None
        }
    }
}

/// Предупредить, что ход не будет записан.
fn warn_no_session(reason: &str, opts: RenderOptions) {
    tracing::warn!(%reason, "сессия не пишется");
    eprintln!(
        "{}",
        render::note_line(
            &format!("сессия не пишется ({reason}); работаю без записи"),
            opts
        )
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Тем же путём, что и `main`: разбор плюс валидация.
    fn parse(argv: &[&str]) -> Args {
        Args::try_parse_and_validate(argv).expect("аргументы должны разбираться")
    }

    #[test]
    fn prompt_is_required_and_positional() {
        let args = parse(&["mark2", "почини тесты"]);
        assert_eq!(args.prompt, "почини тесты");
        assert!(!args.print);
        assert!(!args.yolo);
        assert!(!args.no_session);
        assert!(!args.show_thinking);
        assert_eq!(args.model, None);
        assert_eq!(args.policy, None);
        assert_eq!(args.temperature, None);
        assert_eq!(args.cwd, None);

        assert!(Args::try_parse_from(["mark2"]).is_err());
    }

    #[test]
    fn every_flag_parses() {
        let args = parse(&[
            "mark2",
            "--model",
            "qwen3-coder",
            "--base-url",
            "http://localhost:11434/v1",
            "--api-key-env",
            "OPENROUTER_API_KEY",
            "--policy",
            "read-only",
            "--max-turns",
            "7",
            "--temperature",
            "0.7",
            "--print",
            "--no-session",
            "--cwd",
            "/tmp",
            "--show-thinking",
            "задача",
        ]);

        assert_eq!(args.prompt, "задача");
        assert_eq!(args.model.as_deref(), Some("qwen3-coder"));
        assert_eq!(args.base_url.as_deref(), Some("http://localhost:11434/v1"));
        assert_eq!(args.api_key_env.as_deref(), Some("OPENROUTER_API_KEY"));
        assert_eq!(args.policy, Some(PolicyArg::ReadOnly));
        assert_eq!(args.max_turns, Some(7));
        assert_eq!(args.temperature, Some(0.7));
        assert!(args.print);
        assert!(args.no_session);
        assert_eq!(args.cwd, Some(PathBuf::from("/tmp")));
        assert!(args.show_thinking);
    }

    #[test]
    fn short_flags_match_the_long_ones() {
        let short = parse(&["mark2", "-p", "-C", "/tmp", "задача"]);
        assert!(short.print);
        assert_eq!(short.cwd, Some(PathBuf::from("/tmp")));
    }

    /// `-` — промпт из stdin, и только в print mode; без `-p` — ошибка clap
    /// с объяснением и кодом 2, как у любой ошибки аргументов.
    #[test]
    fn dash_prompt_means_stdin_and_requires_print_mode() {
        assert_eq!(parse(&["mark2", "-p", "-"]).prompt, STDIN_PROMPT);
        assert_eq!(parse(&["mark2", "-", "--print"]).prompt, STDIN_PROMPT);
        assert_eq!(parse(&["mark2", "-p", "--", "-"]).prompt, STDIN_PROMPT);

        for argv in [&["mark2", "-"][..], &["mark2", "--yolo", "--", "-"][..]] {
            let error = Args::try_parse_and_validate(argv).unwrap_err();
            assert_eq!(error.exit_code(), 2, "{error}");
            let message = error.to_string();
            assert!(message.contains("stdin"), "{message}");
            assert!(message.contains("-p"), "{message}");
        }
        // Обычный промпт без -p валиден, как и раньше.
        assert_eq!(
            parse(&["mark2", "задача - с дефисом"]).prompt,
            "задача - с дефисом"
        );
    }

    /// stdin читается до EOF и уходит модели байт-в-байт — в том числе больше
    /// лимита argv Linux на один аргумент (128 КиБ).
    #[test]
    fn stdin_prompt_is_read_to_eof_byte_for_byte() {
        let args = parse(&["mark2", "-p", "-"]);
        let text = "Строка 1\n\n  отступ\nпоследняя без перевода строки";
        assert_eq!(args.resolve_prompt(&mut text.as_bytes()).unwrap(), text);

        let big = format!("{}КОНЕЦ\n", "x".repeat(200 * 1024));
        assert_eq!(args.resolve_prompt(&mut big.as_bytes()).unwrap(), big);
    }

    #[test]
    fn empty_or_broken_stdin_prompt_is_an_error() {
        let args = parse(&["mark2", "-p", "-"]);
        for empty in ["", " \n\t\n"] {
            let error = args.resolve_prompt(&mut empty.as_bytes()).unwrap_err();
            assert!(error.to_string().starts_with("empty prompt"), "{error}");
        }
        // Не UTF-8 — ошибка с причиной, а не мусор в промпте.
        let error = args.resolve_prompt(&mut &b"\xff\xfe"[..]).unwrap_err();
        assert!(format!("{error:#}").contains("UTF-8"), "{error:#}");
    }

    /// Обычный позиционный промпт stdin не трогает: тот остаётся
    /// интерактивному режиму под подтверждения.
    #[test]
    fn positional_prompt_does_not_read_stdin() {
        struct Untouchable;
        impl Read for Untouchable {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                panic!("stdin не должен читаться");
            }
        }
        for argv in [&["mark2", "задача"][..], &["mark2", "-p", "задача"][..]] {
            let args = parse(argv);
            assert_eq!(args.resolve_prompt(&mut Untouchable).unwrap(), "задача");
        }
    }

    #[test]
    fn policy_values_are_kebab_case() {
        for (flag, expected) in [
            ("yolo", PolicyArg::Yolo),
            ("ask-writes", PolicyArg::AskWrites),
            ("read-only", PolicyArg::ReadOnly),
        ] {
            let args = parse(&["mark2", "--policy", flag, "задача"]);
            assert_eq!(args.policy, Some(expected));
        }
        assert!(Args::try_parse_from(["mark2", "--policy", "ask_writes", "x"]).is_err());
    }

    #[test]
    fn yolo_and_policy_cannot_be_combined() {
        assert!(Args::try_parse_from(["mark2", "--yolo", "--policy", "read-only", "x"]).is_err());
        assert_eq!(
            parse(&["mark2", "--yolo", "x"]).policy_override(),
            Some(PermissionPolicy::Yolo)
        );
        assert_eq!(
            parse(&["mark2", "--policy", "read-only", "x"]).policy_override(),
            Some(PermissionPolicy::ReadOnly)
        );
        assert_eq!(parse(&["mark2", "x"]).policy_override(), None);
    }

    #[test]
    fn overrides_only_mention_what_was_asked_for() {
        let empty = parse(&["mark2", "задача"]).overrides();
        assert_eq!(empty, Config::default());

        let args = parse(&["mark2", "--model", "m", "--max-turns", "3", "задача"]);
        let overrides = args.overrides();
        assert_eq!(overrides.agent.model.as_deref(), Some("m"));
        assert_eq!(overrides.agent.max_turns, Some(3));
        assert_eq!(overrides.provider.base_url, None);
    }

    /// Главный контракт слоёв: флаги старше файлов, но перекрывают только то,
    /// что назвали.
    #[test]
    fn flags_win_over_files_field_by_field() {
        let files = Config::parse(
            r#"
            [provider]
            base_url = "http://file/v1"
            api_key_env = "FILE_KEY"
            cache_prompt = true

            [agent]
            model = "file-model"
            max_turns = 99
            policy = "read_only"
            temperature = 0.4

            [models."flag-model"]
            edit_strategy = "whole_file"
            "#,
            "test.toml",
        )
        .unwrap();

        let args = parse(&["mark2", "--model", "flag-model", "--yolo", "задача"]);
        let merged = files.merge(args.overrides());

        assert_eq!(merged.agent.model.as_deref(), Some("flag-model"));
        assert_eq!(merged.agent.policy, Some(PermissionPolicy::Yolo));
        // Про что флаги молчали — осталось из файла.
        assert_eq!(merged.base_url(), "http://file/v1");
        assert_eq!(merged.provider.api_key_env.as_deref(), Some("FILE_KEY"));
        assert_eq!(merged.agent.max_turns, Some(99));
        assert_eq!(merged.agent.temperature, Some(0.4));
        // Настройки модели читаются под ту модель, что победила.
        assert_eq!(
            merged.resolve("flag-model").edit_strategy,
            mark2_core::tools::EditStrategy::WholeFile
        );
    }

    /// `--temperature` — слой флагов, как `--model`: перекрывает
    /// `[agent] temperature` из файла и доходит до агента; без флага остаётся
    /// значение из файла. Ради `0` флаг и заведён — ревью детерминированное.
    #[test]
    fn temperature_flag_wins_over_the_config_and_reaches_the_agent() {
        let files = Config::parse("[agent]\ntemperature = 0.4\n", "test.toml").unwrap();
        let cwd = Path::new("/work");

        for (flag, expected) in [("0", 0.0), ("0.0", 0.0), ("1.25", 1.25), ("2", 2.0)] {
            let args = parse(&["mark2", "--temperature", flag, "задача"]);
            assert_eq!(args.overrides().agent.temperature, Some(expected), "{flag}");
            let merged = files.clone().merge(args.overrides());
            assert_eq!(merged.agent.temperature, Some(expected), "{flag}");
            let agent = agent_config(&merged, "m", cwd, merged.resolve("m"));
            assert_eq!(agent.temperature, Some(expected), "{flag}");
        }

        let silent = files.clone().merge(parse(&["mark2", "задача"]).overrides());
        assert_eq!(silent.agent.temperature, Some(0.4));
        let agent = agent_config(&silent, "m", cwd, silent.resolve("m"));
        assert_eq!(agent.temperature, Some(0.4));

        // Ни флага, ни файла — температуру выбирает сервер.
        let nothing = Config::default().merge(parse(&["mark2", "задача"]).overrides());
        let agent = agent_config(&nothing, "m", cwd, nothing.resolve("m"));
        assert_eq!(agent.temperature, None);
    }

    /// Вне 0..=2 (и не число вовсе) — ошибка clap с кодом 2, как у любой
    /// опечатки во флагах, а не отказ сервера посреди хода.
    #[test]
    fn temperature_outside_zero_to_two_is_a_usage_error() {
        let bad = [
            "2.01", "3", "-0.5", "-1", "abc", "", "nan", "inf", "0,5", "1e3",
        ];
        for value in bad {
            let joined = format!("--temperature={value}");
            for argv in [
                vec!["mark2", joined.as_str(), "задача"],
                // Отдельным словом — тоже, в том числе отрицательное: это
                // значение вне диапазона, а не «неизвестный флаг -1».
                vec!["mark2", "--temperature", value, "задача"],
            ] {
                let error = Args::try_parse_and_validate(&argv).expect_err(value);
                assert_eq!(error.exit_code(), 2, "{argv:?}: {error}");
                assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
                let message = error.to_string();
                assert!(message.contains("--temperature"), "{message}");
                assert!(message.contains("от 0 до 2"), "{message}");
            }
        }
    }

    #[test]
    fn missing_model_is_an_error_that_says_what_to_do() {
        let config = Config::default();
        let error = config
            .agent
            .model
            .clone()
            .context("не задана модель: укажи --model <id>")
            .unwrap_err();
        assert!(error.to_string().contains("--model"), "{error}");
    }

    #[test]
    fn cwd_flag_must_point_at_an_existing_directory() {
        let dir = tempfile::tempdir().unwrap();
        let args = parse(&["mark2", "-C", dir.path().to_str().unwrap(), "задача"]);
        assert_eq!(
            args.resolve_cwd().unwrap(),
            dir.path().canonicalize().unwrap()
        );

        let missing = dir.path().join("нет-такой");
        let args = parse(&["mark2", "-C", missing.to_str().unwrap(), "задача"]);
        let error = args.resolve_cwd().unwrap_err();
        assert!(error.to_string().contains("недоступна"), "{error}");
    }

    #[test]
    fn render_options_follow_the_mode() {
        let print = parse(&["mark2", "-p", "--show-thinking", "задача"]).render_options();
        // В print mode рассуждения не показываются даже по флагу.
        assert_eq!(print, RenderOptions::print());

        let interactive = parse(&["mark2", "--show-thinking", "задача"]).render_options();
        assert!(interactive.show_thinking);
        assert!(!interactive.text_only);
    }

    #[test]
    fn exit_codes_distinguish_failure_from_interruption() {
        assert_eq!(
            format!("{:?}", exit_code(StopReason::Completed)),
            format!("{:?}", ExitCode::SUCCESS)
        );
        assert_eq!(
            format!("{:?}", exit_code(StopReason::Error)),
            format!("{:?}", ExitCode::FAILURE)
        );
        assert_eq!(
            format!("{:?}", exit_code(StopReason::MaxTurns)),
            format!("{:?}", ExitCode::FAILURE)
        );
        assert_eq!(
            format!("{:?}", exit_code(StopReason::Cancelled)),
            format!("{:?}", ExitCode::from(130))
        );
    }

    #[test]
    fn api_key_comes_from_the_named_variable_only() {
        // Имя переменной не задано — ключа нет.
        assert_eq!(api_key(&Config::default()), None);

        let variable = "MARK2_TEST_API_KEY";
        // SAFETY: тест однопоточный по этой переменной и убирает её за собой.
        unsafe { std::env::set_var(variable, "secret") };
        let config = Config::parse(
            &format!("[provider]\napi_key_env = \"{variable}\"\n"),
            "test.toml",
        )
        .unwrap();
        assert_eq!(api_key(&config), Some("secret".to_string()));

        unsafe { std::env::set_var(variable, "  ") };
        assert_eq!(api_key(&config), None);
        unsafe { std::env::remove_var(variable) };
        assert_eq!(api_key(&config), None);
    }

    /// В шапку сессии — путь и размер инъектированной памяти, если она есть.
    #[test]
    fn session_meta_records_the_injected_memory() {
        let cwd = Path::new("/work");
        let plain = session_meta(cwd, "m", None);
        assert_eq!(plain.memory, None);
        assert_eq!(plain.cwd, "/work");

        let memory = Memory::from_text("/work/.mark2.memory.md", "# Memory\n- факт\n").unwrap();
        let meta = session_meta(cwd, "m", Some(&memory));
        let recorded = meta.memory.expect("память в шапке");
        assert_eq!(recorded.path, "/work/.mark2.memory.md");
        assert_eq!(recorded.bytes, "# Memory\n- факт".len() as u64);
    }

    /// `memory_file` из проектного конфига доходит до агента: относительный
    /// путь резолвится от директории конфига, а не от `-C`.
    #[test]
    fn memory_file_from_the_config_reaches_the_agent() {
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();
        let cwd = root_path.join("sub");
        std::fs::create_dir_all(cwd.join("notes")).unwrap();
        std::fs::create_dir_all(root_path.join("notes")).unwrap();
        std::fs::write(
            root_path.join(".mark2.toml"),
            "[agent]\nmemory_file = \"notes/mem.md\"\n",
        )
        .unwrap();
        std::fs::write(root_path.join("notes").join("mem.md"), "- из корня\n").unwrap();
        std::fs::write(cwd.join("notes").join("mem.md"), "- из sub\n").unwrap();

        let path = mark2_core::config::find_project_config(&cwd).unwrap();
        let config = Config::load_file(&path).unwrap().unwrap();
        let agent_config = agent_config(&config, "m", &cwd, config.resolve("m"));
        assert_eq!(
            agent_config.memory_file.as_deref(),
            Some(root_path.join("notes").join("mem.md").as_path())
        );
        let agent = Agent::new(
            Arc::new(OpenAiCompatProvider::new(
                "http://127.0.0.1:9/v1",
                None,
                CompatOptions::default(),
            )) as Arc<dyn Provider>,
            ToolRegistry::new(),
            agent_config,
        );
        let memory = agent.memory().expect("память найдена");
        assert_eq!(memory.path, root_path.join("notes").join("mem.md"));
        assert!(agent.system_prompt().ends_with("- из корня"));
    }

    /// Post-edit сенсор: команда из конфига доходит до агента с таймаутом;
    /// без ключа (и с пустой строкой) — сенсора нет.
    #[test]
    fn post_edit_command_from_the_config_reaches_the_agent() {
        let cwd = Path::new("/work");
        let build = |text: &str| {
            let config = Config::parse(text, "/work/.mark2.toml").unwrap();
            agent_config(&config, "m", cwd, config.resolve("m")).post_edit
        };

        assert_eq!(build(""), None);
        assert_eq!(build("[agent]\npost_edit_command = \"\"\n"), None);
        assert_eq!(
            build(
                "[agent]\npost_edit_command = \"python3 -B test.py\"\npost_edit_timeout_seconds = 5\n"
            ),
            Some(
                mark2_core::PostEditCheck::new("python3 -B test.py")
                    .with_timeout(std::time::Duration::from_secs(5))
            )
        );

        // Флаги CLI сенсор не трогают: слой флагов про него молчит.
        let files = Config::parse("[agent]\npost_edit_command = \"make check\"\n", "t").unwrap();
        let merged = files.merge(parse(&["mark2", "--model", "m", "x"]).overrides());
        assert_eq!(
            agent_config(&merged, "m", cwd, merged.resolve("m"))
                .post_edit
                .map(|check| check.command),
            Some("make check".to_string())
        );
    }

    /// `web_fetch` есть всегда, `web_search` — только с адресом SearXNG;
    /// битый адрес — ошибка запуска с подсказкой, где его чинить.
    #[test]
    fn tool_registry_adds_web_search_only_with_a_backend() {
        let names = |registry: ToolRegistry| -> Vec<String> {
            registry.names().into_iter().map(str::to_string).collect()
        };
        let plain = tool_registry(EditStrategy::SearchReplace, true, true, None).unwrap();
        assert!(plain.arg_coercion());
        let plain = names(plain);
        assert!(plain.contains(&"web_fetch".to_string()), "{plain:?}");
        assert!(!plain.contains(&"web_search".to_string()), "{plain:?}");

        let full = tool_registry(
            EditStrategy::WholeFile,
            false,
            true,
            Some("http://localhost:8888"),
        )
        .unwrap();
        assert!(!full.arg_coercion());
        assert_eq!(names(full).last().map(String::as_str), Some("web_search"));

        let error = tool_registry(
            EditStrategy::SearchReplace,
            true,
            true,
            Some("localhost:8888"),
        )
        .unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("[web] search_base_url"), "{text}");
        assert!(text.contains(SEARXNG_URL_ENV), "{text}");
        assert!(text.contains("localhost:8888"), "{text}");
    }

    /// Адрес поиска из файла доходит до реестра; флаги CLI про него молчат.
    #[test]
    fn search_base_url_from_the_config_reaches_the_registry() {
        let files = Config::parse("[web]\nsearch_base_url = \"http://searx:8888\"\n", "t").unwrap();
        let merged = files.merge(parse(&["mark2", "--model", "m", "x"]).overrides());
        assert_eq!(
            merged.web.search_base_url_with_env(None).as_deref(),
            Some("http://searx:8888")
        );
        let registry = tool_registry(
            EditStrategy::SearchReplace,
            true,
            merged.web.fetch_enabled(),
            merged.web.search_base_url_with_env(None).as_deref(),
        )
        .unwrap();
        assert!(registry.get("web_search").is_some());
        assert!(registry.get("web_fetch").is_some());
    }

    /// `[web] fetch = false` из файла убирает `web_fetch` из реестра; прочие
    /// тулы и `web_search` остаются на своих местах.
    #[test]
    fn web_fetch_switch_from_the_config_reaches_the_registry() {
        let names = |registry: &ToolRegistry| -> Vec<String> {
            registry.names().into_iter().map(str::to_string).collect()
        };
        let build = |toml: &str| {
            let files = Config::parse(toml, "t").unwrap();
            let merged = files.merge(parse(&["mark2", "--model", "m", "x"]).overrides());
            tool_registry(
                EditStrategy::SearchReplace,
                true,
                merged.web.fetch_enabled(),
                merged.web.search_base_url_with_env(None).as_deref(),
            )
            .unwrap()
        };

        let on = build("");
        let off = build("[web]\nfetch = false\n");
        assert_eq!(names(&off), ["bash", "edit", "grep", "read", "write"]);
        assert_eq!(names(&on).last().map(String::as_str), Some("web_fetch"));

        let search_only = build("[web]\nfetch = false\nsearch_base_url = \"http://searx:8888\"\n");
        assert_eq!(
            names(&search_only),
            ["bash", "edit", "grep", "read", "write", "web_search"]
        );
    }

    #[test]
    fn help_and_version_are_available() {
        let help = Args::command().render_help().to_string();
        assert!(help.contains("--model"), "{help}");
        assert!(help.contains("--yolo"), "{help}");
        assert!(help.contains("--temperature"), "{help}");
        assert!(help.contains(".mark2.toml"), "{help}");
        assert!(help.contains("stdin"), "{help}");
        assert!(!Args::command().get_version().unwrap_or_default().is_empty());
    }
}
