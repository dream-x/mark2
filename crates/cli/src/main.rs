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
//! `130` — прервано пользователем (как принято у Ctrl+C в шелле).

mod interactive;
mod print;
mod render;

use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use mark2_core::provider::Provider;
use mark2_core::session::{SessionMeta, SessionRecorder, default_base_dir};
use mark2_core::tools::ToolRegistry;
use mark2_core::{
    Agent, AgentConfig, AgentSettings, Config, Op, PermissionPolicy, ProviderConfig, StopReason,
};
use mark2_providers::openai_compat::{CompatOptions, OpenAiCompatProvider, PROVIDER_NAME};

use crate::render::RenderOptions;

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
    /// Задача агенту.
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
    /// Спрашивать перед bash/edit/write.
    AskWrites,
    /// Только чтение.
    ReadOnly,
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
                temperature: None,
                loop_detection: None,
                write_churn: None,
                arg_coercion: None,
            },
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
    let args = Args::parse();
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

    let provider = OpenAiCompatProvider::new(
        config.base_url(),
        api_key(&config),
        CompatOptions {
            cache_prompt: config.provider.cache_prompt.unwrap_or(false),
            slot_id: config.provider.slot_id,
        },
    );

    let mut agent_config = AgentConfig::new(&model)
        .with_cwd(&cwd)
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
    if let Some(limit) = settings.context_limit {
        agent_config = agent_config
            .with_compaction(mark2_core::CompactionConfig::new().with_context_limit(limit));
    }

    let mut agent = Agent::new(
        Arc::new(provider) as Arc<dyn Provider>,
        ToolRegistry::with_defaults(settings.edit_strategy)
            .with_arg_coercion(config.agent.arg_coercion.unwrap_or(true)),
        agent_config,
    );
    if !args.no_session
        && let Some(recorder) = open_session(&cwd, &model, opts).await
    {
        tracing::debug!(path = %recorder.path().display(), "сессия пишется");
        agent = agent.with_recorder(recorder);
    }

    let (handle, mut events) = agent.spawn();
    handle.submit(Op::user_input(args.prompt.clone()));

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

/// Открыть сессию на запись. Не смогли — работаем без журнала: потеря записи
/// не повод не выполнить задачу.
async fn open_session(cwd: &Path, model: &str, opts: RenderOptions) -> Option<SessionRecorder> {
    let base_dir = match default_base_dir() {
        Ok(dir) => dir,
        Err(error) => {
            warn_no_session(&error.to_string(), opts);
            return None;
        }
    };
    let meta = SessionMeta::new(cwd.display().to_string(), model, PROVIDER_NAME);
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
    use clap::CommandFactory;

    fn parse(argv: &[&str]) -> Args {
        Args::try_parse_from(argv).expect("аргументы должны разбираться")
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

    #[test]
    fn help_and_version_are_available() {
        let help = Args::command().render_help().to_string();
        assert!(help.contains("--model"), "{help}");
        assert!(help.contains("--yolo"), "{help}");
        assert!(help.contains(".mark2.toml"), "{help}");
        assert!(!Args::command().get_version().unwrap_or_default().is_empty());
    }
}
