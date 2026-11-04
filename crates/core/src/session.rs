//! Append-only JSONL-сессии: запись хода агента на диск и чтение обратно.
//!
//! Формат намеренно примитивен — **одна строка файла = один JSON-объект**
//! `{ts, type, …}`. Из такого формата бесплатно падают resume, fork и эвалы:
//! читать можно построчно, дописывать — только в конец, ничего не переписывая
//! (тот же принцип, что у истории сообщений: append-only ради стабильного
//! префикса промпта).
//!
//! Разделение обязанностей:
//!
//! * [`SessionItem`] — что произошло (плоский serde-tagged enum по полю `type`);
//! * [`SessionLine`] — `ts` + `#[serde(flatten)]` элемента, то есть ровно то,
//!   что лежит в файле;
//! * [`SessionRecorder`] — писатель: [`SessionRecorder::record`] не блокирует
//!   вызывающего (элемент уходит в неограниченный канал), фоновая задача пишет
//!   буферизованно и делает `flush` после каждого
//!   [`SessionItem::TurnFinished`] — то есть после завершения хода файл на
//!   диске полон;
//! * [`load_session`] — читатель, который **не падает** на битом файле:
//!   повреждённые строки пропускаются с `tracing::warn` и считаются в
//!   [`LoadedSession::skipped`]. Сессия — журнал, а не база: недописанный
//!   хвост после `kill -9` не должен стоить пользователю всей истории.
//!
//! Ошибки записи внутри фоновой задачи не паникуют и не роняют агента: они
//! логируются через `tracing::error`. Потеря журнала не повод убивать ход.

use std::io;
use std::path::{Path, PathBuf};

use jiff::{Timestamp, Zoned};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::event::StopReason;
use crate::message::Message;

/// Размер буфера писателя: строки сессии мелкие, а `flush` и так делается
/// после каждого завершённого хода.
const WRITE_BUFFER_BYTES: usize = 64 * 1024;

/// Ошибки записи и чтения сессий.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// Не удалось создать директорию `YYYY/MM/DD`.
    #[error("не удалось создать директорию сессии {}: {source}", path.display())]
    CreateDir {
        /// Путь директории.
        path: PathBuf,
        /// Исходная ошибка ввода-вывода.
        source: io::Error,
    },
    /// Не удалось открыть файл сессии на запись.
    #[error("не удалось открыть файл сессии {}: {source}", path.display())]
    OpenFile {
        /// Путь файла.
        path: PathBuf,
        /// Исходная ошибка ввода-вывода.
        source: io::Error,
    },
    /// Не удалось записать строку в файл сессии.
    #[error("не удалось записать в файл сессии {}: {source}", path.display())]
    Write {
        /// Путь файла.
        path: PathBuf,
        /// Исходная ошибка ввода-вывода.
        source: io::Error,
    },
    /// Не удалось прочитать файл сессии.
    #[error("не удалось прочитать файл сессии {}: {source}", path.display())]
    Read {
        /// Путь файла.
        path: PathBuf,
        /// Исходная ошибка ввода-вывода.
        source: io::Error,
    },
    /// Не удалось сериализовать элемент сессии в JSON.
    #[error("не удалось сериализовать элемент сессии: {0}")]
    Serialize(#[source] serde_json::Error),
    /// Не удалось определить домашнюю директорию для [`default_base_dir`].
    #[error("не удалось определить домашнюю директорию для ~/.mark2/sessions")]
    NoHomeDir,
}

/// Результат операций модуля.
pub type SessionResult<T> = std::result::Result<T, SessionError>;

/// Шапка сессии: с чем её запускали.
///
/// В файл уходит первой строкой как [`SessionItem::Meta`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMeta {
    /// Идентификатор сессии. Если это валидный uuid — он же попадёт в имя
    /// файла, так что сессию можно найти по идентификатору из логов.
    pub id: String,
    /// Рабочая директория, в которой запущен агент.
    pub cwd: String,
    /// Идентификатор модели.
    pub model: String,
    /// Имя провайдера.
    pub provider: String,
    /// Память агента, инъектированная в системный промпт (для отладки:
    /// «что модель знала на старте»); `None` — памяти не было.
    pub memory: Option<SessionMemory>,
}

/// Какая память ушла в системный промпт сессии ([`crate::memory`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMemory {
    /// Путь файла памяти.
    pub path: String,
    /// Сколько байт содержимого инъектировано (после капа).
    pub bytes: u64,
}

impl SessionMeta {
    /// Шапка новой сессии со свежим uuid v4 в качестве идентификатора.
    pub fn new(
        cwd: impl Into<String>,
        model: impl Into<String>,
        provider: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            cwd: cwd.into(),
            model: model.into(),
            provider: provider.into(),
            memory: None,
        }
    }

    /// Задать идентификатор явно (например при fork существующей сессии).
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// Записать в шапку инъектированную память: путь и размер.
    #[must_use]
    pub fn with_memory(mut self, path: impl Into<String>, bytes: u64) -> Self {
        self.memory = Some(SessionMemory {
            path: path.into(),
            bytes,
        });
        self
    }
}

/// Элемент сессии — содержимое строки JSONL без временной метки.
///
/// Serde-тег `type` в snake_case: `meta`, `message`, `turn_started`,
/// `turn_finished`, `compacted`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionItem {
    /// Шапка сессии, всегда первая строка файла.
    Meta {
        /// Идентификатор сессии.
        id: String,
        /// Рабочая директория.
        cwd: String,
        /// Идентификатор модели.
        model: String,
        /// Имя провайдера.
        provider: String,
        /// Инъектированная память; без неё ключа в строке нет вовсе — шапки
        /// сессий без памяти байт-в-байт прежние, старые читаются как есть.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        memory: Option<SessionMemory>,
    },
    /// Сообщение диалога в нейтральном формате (включая флаги видимости —
    /// сессия хранит и то, что скрыто компакцией).
    Message {
        /// Сообщение.
        message: Message,
    },
    /// Маркер начала хода.
    TurnStarted {
        /// Идентификатор хода — тот же, что в [`crate::Event::TurnStarted`].
        turn_id: String,
    },
    /// Маркер завершения хода; после него писатель делает `flush`.
    TurnFinished {
        /// Идентификатор хода.
        turn_id: String,
        /// Причина завершения.
        reason: StopReason,
    },
    /// Компакция истории.
    Compacted {
        /// Оценка занятости контекста до компакции.
        tokens_before: u64,
        /// Оценка занятости контекста после компакции.
        tokens_after: u64,
    },
}

impl SessionItem {
    /// Элемент с сообщением диалога.
    pub fn message(message: Message) -> Self {
        Self::Message { message }
    }

    /// Маркер начала хода.
    pub fn turn_started(turn_id: impl Into<String>) -> Self {
        Self::TurnStarted {
            turn_id: turn_id.into(),
        }
    }

    /// Маркер завершения хода.
    pub fn turn_finished(turn_id: impl Into<String>, reason: StopReason) -> Self {
        Self::TurnFinished {
            turn_id: turn_id.into(),
            reason,
        }
    }
}

impl From<SessionMeta> for SessionItem {
    fn from(meta: SessionMeta) -> Self {
        let SessionMeta {
            id,
            cwd,
            model,
            provider,
            memory,
        } = meta;
        Self::Meta {
            id,
            cwd,
            model,
            provider,
            memory,
        }
    }
}

/// Строка файла сессии: временная метка плюс сам элемент.
///
/// `item` разложен через `#[serde(flatten)]`, поэтому в файле лежит плоский
/// объект вида `{"ts":"…","type":"message","message":{…}}` — без лишнего
/// уровня вложенности, читаемо `jq` и любым построчным инструментом.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionLine {
    /// Момент записи в RFC 3339 (UTC, суффикс `Z`).
    pub ts: String,
    /// Что произошло.
    #[serde(flatten)]
    pub item: SessionItem,
}

impl SessionLine {
    /// Строка с явной меткой времени.
    pub fn new(ts: Timestamp, item: SessionItem) -> Self {
        Self {
            ts: ts.to_string(),
            item,
        }
    }

    /// Строка с меткой «сейчас».
    pub fn now(item: SessionItem) -> Self {
        Self::new(Timestamp::now(), item)
    }
}

/// Прочитанная сессия.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedSession {
    /// Успешно разобранные строки в порядке файла.
    pub lines: Vec<SessionLine>,
    /// Сколько строк пропущено из-за повреждений.
    pub skipped: usize,
}

/// Директория сессий по умолчанию: `~/.mark2/sessions`.
pub fn default_base_dir() -> SessionResult<PathBuf> {
    let home = dirs::home_dir().ok_or(SessionError::NoHomeDir)?;
    Ok(home.join(".mark2").join("sessions"))
}

/// Писатель сессии: владеет файлом и фоновой задачей записи.
///
/// [`SessionRecorder::record`] только кладёт элемент в канал, поэтому agent
/// loop никогда не ждёт диск. Дисциплина durability — `flush` после каждого
/// [`SessionItem::TurnFinished`] и при закрытии.
#[derive(Debug)]
pub struct SessionRecorder {
    path: PathBuf,
    tx: UnboundedSender<SessionItem>,
    writer: JoinHandle<()>,
}

impl SessionRecorder {
    /// Создать сессию в `base_dir`: файл
    /// `<base_dir>/YYYY/MM/DD/<unix_ts>-<uuid>.jsonl` (директории создаются),
    /// первой строкой синхронно пишется и сбрасывается на диск
    /// [`SessionItem::Meta`].
    ///
    /// Дата директории — локальная (сессии группируются по дню пользователя),
    /// `ts` внутри строк — UTC.
    pub async fn create(base_dir: &Path, meta: SessionMeta) -> SessionResult<Self> {
        let now = Zoned::now();
        let dir = base_dir
            .join(format!("{:04}", now.year()))
            .join(format!("{:02}", now.month()))
            .join(format!("{:02}", now.day()));
        fs::create_dir_all(&dir)
            .await
            .map_err(|source| SessionError::CreateDir {
                path: dir.clone(),
                source,
            })?;

        // Имя файла берёт uuid из идентификатора сессии, если он им является:
        // так файл находится по id из логов, и в путь заведомо не попадают
        // сепараторы из произвольной строки.
        let file_uuid = Uuid::parse_str(&meta.id).unwrap_or_else(|_| Uuid::new_v4());
        let path = dir.join(format!(
            "{}-{}.jsonl",
            now.timestamp().as_second(),
            file_uuid
        ));

        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|source| SessionError::OpenFile {
                path: path.clone(),
                source,
            })?;

        let meta_line = encode_line(&SessionLine::new(now.timestamp(), meta.into()))?;
        file.write_all(meta_line.as_bytes())
            .await
            .map_err(|source| SessionError::Write {
                path: path.clone(),
                source,
            })?;
        file.flush().await.map_err(|source| SessionError::Write {
            path: path.clone(),
            source,
        })?;

        let (tx, rx) = mpsc::unbounded_channel();
        let writer = tokio::spawn(write_loop(
            BufWriter::with_capacity(WRITE_BUFFER_BYTES, file),
            rx,
            path.clone(),
        ));

        Ok(Self { path, tx, writer })
    }

    /// Путь файла сессии.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Записать элемент. Не блокирует: элемент уходит в канал, реальная запись
    /// происходит в фоновой задаче. Сбой доставки логируется, а не паникует.
    pub fn record(&self, item: SessionItem) {
        if let Err(error) = self.tx.send(item) {
            tracing::error!(
                path = %self.path.display(),
                %error,
                "писатель сессии остановлен, элемент потерян"
            );
        }
    }

    /// Корректно закрыть сессию: дослать всё, что уже в канале, сбросить
    /// буфер на диск и дождаться завершения фоновой задачи.
    pub async fn close(self) {
        let Self { path, tx, writer } = self;
        // Закрытие канала — сигнал писателю дочитать очередь и сделать flush.
        drop(tx);
        if let Err(error) = writer.await {
            tracing::error!(
                path = %path.display(),
                %error,
                "задача записи сессии завершилась аварийно"
            );
        }
    }
}

/// Фоновая запись: строка за строкой, `flush` после завершения хода и в конце.
async fn write_loop(
    mut writer: BufWriter<fs::File>,
    mut rx: UnboundedReceiver<SessionItem>,
    path: PathBuf,
) {
    while let Some(item) = rx.recv().await {
        let flush_after = matches!(item, SessionItem::TurnFinished { .. });

        match encode_line(&SessionLine::now(item)) {
            Ok(line) => {
                if let Err(error) = writer.write_all(line.as_bytes()).await {
                    tracing::error!(
                        path = %path.display(),
                        %error,
                        "не удалось записать строку сессии"
                    );
                    continue;
                }
            }
            Err(error) => {
                tracing::error!(
                    path = %path.display(),
                    %error,
                    "не удалось сериализовать элемент сессии"
                );
                continue;
            }
        }

        if flush_after {
            flush(&mut writer, &path).await;
        }
    }

    flush(&mut writer, &path).await;
}

async fn flush(writer: &mut BufWriter<fs::File>, path: &Path) {
    if let Err(error) = writer.flush().await {
        tracing::error!(
            path = %path.display(),
            %error,
            "не удалось сбросить буфер сессии на диск"
        );
    }
}

/// Сериализовать строку сессии вместе с завершающим переводом строки.
fn encode_line(line: &SessionLine) -> SessionResult<String> {
    let mut encoded = serde_json::to_string(line).map_err(SessionError::Serialize)?;
    encoded.push('\n');
    Ok(encoded)
}

/// Прочитать сессию.
///
/// Повреждённые строки не валят загрузку: каждая такая строка логируется через
/// `tracing::warn` и считается в [`LoadedSession::skipped`]. Пустые строки
/// игнорируются молча и в счётчик не попадают.
pub async fn load_session(path: &Path) -> SessionResult<LoadedSession> {
    let raw = fs::read_to_string(path)
        .await
        .map_err(|source| SessionError::Read {
            path: path.to_path_buf(),
            source,
        })?;

    let mut lines = Vec::new();
    let mut skipped = 0usize;

    for (index, raw_line) in raw.lines().enumerate() {
        if raw_line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<SessionLine>(raw_line) {
            Ok(line) => lines.push(line),
            Err(error) => {
                skipped += 1;
                tracing::warn!(
                    path = %path.display(),
                    line = index + 1,
                    %error,
                    "повреждённая строка сессии пропущена"
                );
            }
        }
    }

    Ok(LoadedSession { lines, skipped })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{ContentBlock, MessageMetadata};
    use serde_json::json;
    use std::time::Duration;

    /// Временная директория на std: `tempfile` намеренно не тянем, чтобы не
    /// конфликтовать с dev-зависимостями соседних тасков.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("mark2-session-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn meta() -> SessionMeta {
        SessionMeta::new("/work/mark2", "qwen3-coder", "openai_compat")
    }

    fn all_items() -> Vec<SessionItem> {
        vec![
            SessionItem::message(Message::user("сделай X")),
            SessionItem::turn_started("turn_1"),
            SessionItem::message(
                Message::new(
                    crate::message::Role::Assistant,
                    vec![
                        ContentBlock::thinking("надо позвать тул"),
                        ContentBlock::tool_use("call_1", "read", json!({"path": "/tmp/a.txt"})),
                    ],
                )
                .with_metadata(MessageMetadata::hidden_from_agent()),
            ),
            SessionItem::message(
                Message::assistant("саммари").with_metadata(MessageMetadata::hidden_from_user()),
            ),
            SessionItem::message(Message::tool_error("call_1", "file not found")),
            SessionItem::Compacted {
                tokens_before: 12_000,
                tokens_after: 3_400,
            },
            SessionItem::turn_finished("turn_1", StopReason::Completed),
        ]
    }

    /// Дождаться, пока фоновый писатель доведёт до диска ожидаемое число строк.
    async fn wait_for_lines(path: &Path, expected: usize) -> LoadedSession {
        for _ in 0..200 {
            let loaded = load_session(path).await.unwrap();
            if loaded.lines.len() >= expected {
                return loaded;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("не дождались {expected} строк в {}", path.display());
    }

    #[tokio::test]
    async fn write_close_read_round_trip() {
        let dir = TempDir::new();
        let meta = meta();
        let recorder = SessionRecorder::create(dir.path(), meta.clone())
            .await
            .unwrap();
        let path = recorder.path().to_path_buf();

        let items = all_items();
        for item in &items {
            recorder.record(item.clone());
        }
        recorder.close().await;

        let loaded = load_session(&path).await.unwrap();
        assert_eq!(loaded.skipped, 0);
        assert_eq!(loaded.lines.len(), items.len() + 1);

        // Первая строка — всегда meta.
        assert_eq!(loaded.lines[0].item, SessionItem::from(meta));

        let recorded: Vec<SessionItem> = loaded.lines[1..].iter().map(|l| l.item.clone()).collect();
        assert_eq!(recorded, items);

        // Каждая строка несёт разбираемый RFC 3339 timestamp.
        for line in &loaded.lines {
            line.ts
                .parse::<Timestamp>()
                .unwrap_or_else(|e| panic!("ts {:?} не RFC 3339: {e}", line.ts));
        }
    }

    #[tokio::test]
    async fn message_visibility_metadata_survives_round_trip() {
        let dir = TempDir::new();
        let recorder = SessionRecorder::create(dir.path(), meta()).await.unwrap();
        let path = recorder.path().to_path_buf();

        let hidden = Message::user("старое").with_agent_visible(false);
        let summary = Message::assistant("саммари").with_user_visible(false);
        recorder.record(SessionItem::message(hidden.clone()));
        recorder.record(SessionItem::message(summary.clone()));
        recorder.close().await;

        let loaded = load_session(&path).await.unwrap();
        let messages: Vec<Message> = loaded.lines[1..]
            .iter()
            .filter_map(|line| match &line.item {
                SessionItem::Message { message } => Some(message.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(messages, vec![hidden, summary]);
        assert!(!messages[0].is_agent_visible());
        assert!(!messages[1].is_user_visible());
    }

    #[tokio::test]
    async fn corrupted_line_in_the_middle_does_not_break_loading() {
        let dir = TempDir::new();
        let path = dir.path().join("broken.jsonl");
        let content = concat!(
            r#"{"ts":"2026-08-24T09:00:00Z","type":"meta","id":"s1","cwd":"/w","model":"m","provider":"p"}"#,
            "\n",
            r#"{"ts":"2026-08-24T09:00:01Z","type":"turn_started","turn_id":"t1"}"#,
            "\n",
            "это не json\n",
            r#"{"ts":"2026-08-24T09:00:02Z","type":"неизвестный_тип"}"#,
            "\n",
            "\n",
            r#"{"ts":"2026-08-24T09:00:03Z","type":"turn_finished","turn_id":"t1","reason":"completed"}"#,
            "\n",
        );
        std::fs::write(&path, content).unwrap();

        let loaded = load_session(&path).await.unwrap();
        assert_eq!(loaded.skipped, 2, "битая строка и неизвестный тип");
        assert_eq!(loaded.lines.len(), 3);
        assert_eq!(
            loaded.lines[0].item,
            SessionItem::Meta {
                id: "s1".into(),
                cwd: "/w".into(),
                model: "m".into(),
                provider: "p".into(),
                memory: None,
            }
        );
        assert_eq!(loaded.lines[1].item, SessionItem::turn_started("t1"));
        assert_eq!(
            loaded.lines[2].item,
            SessionItem::turn_finished("t1", StopReason::Completed)
        );
    }

    #[tokio::test]
    async fn truncated_tail_line_is_skipped_not_fatal() {
        let dir = TempDir::new();
        let recorder = SessionRecorder::create(dir.path(), meta()).await.unwrap();
        let path = recorder.path().to_path_buf();
        recorder.record(SessionItem::turn_started("t1"));
        recorder.close().await;

        // Имитация kill -9 посреди записи: недописанный JSON в конце файла.
        let mut raw = std::fs::read_to_string(&path).unwrap();
        raw.push_str(r#"{"ts":"2026-08-24T09:00:0"#);
        std::fs::write(&path, raw).unwrap();

        let loaded = load_session(&path).await.unwrap();
        assert_eq!(loaded.lines.len(), 2);
        assert_eq!(loaded.skipped, 1);
    }

    #[tokio::test]
    async fn file_lives_in_year_month_day_directories() {
        let dir = TempDir::new();
        let before = Zoned::now();
        let meta = meta();
        let recorder = SessionRecorder::create(dir.path(), meta.clone())
            .await
            .unwrap();
        let after = Zoned::now();

        let expected = |z: &Zoned| {
            dir.path()
                .join(format!("{:04}", z.year()))
                .join(format!("{:02}", z.month()))
                .join(format!("{:02}", z.day()))
        };
        let parent = recorder.path().parent().unwrap();
        assert!(
            parent == expected(&before) || parent == expected(&after),
            "неожиданная директория {}",
            parent.display()
        );

        let name = recorder
            .path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let stem = name.strip_suffix(".jsonl").expect("расширение .jsonl");
        let (unix_ts, uuid) = stem.split_once('-').expect("имя вида <unix_ts>-<uuid>");
        let unix_ts: i64 = unix_ts.parse().expect("unix ts — число");
        assert!(unix_ts >= before.timestamp().as_second());
        assert!(unix_ts <= after.timestamp().as_second());
        assert_eq!(uuid, meta.id, "uuid сессии попадает в имя файла");

        recorder.close().await;
    }

    #[tokio::test]
    async fn meta_is_on_disk_right_after_create() {
        let dir = TempDir::new();
        let meta = meta();
        let recorder = SessionRecorder::create(dir.path(), meta.clone())
            .await
            .unwrap();

        let loaded = load_session(recorder.path()).await.unwrap();
        assert_eq!(loaded.lines.len(), 1);
        assert_eq!(loaded.lines[0].item, SessionItem::from(meta));

        recorder.close().await;
    }

    #[tokio::test]
    async fn turn_finished_flushes_before_close() {
        let dir = TempDir::new();
        let recorder = SessionRecorder::create(dir.path(), meta()).await.unwrap();
        let path = recorder.path().to_path_buf();

        recorder.record(SessionItem::turn_started("t1"));
        recorder.record(SessionItem::message(Message::assistant("готово")));
        recorder.record(SessionItem::turn_finished("t1", StopReason::Completed));

        // Читаем ДО close: flush после TurnFinished обязан довести всё до диска.
        let loaded = wait_for_lines(&path, 4).await;
        assert_eq!(loaded.skipped, 0);
        assert_eq!(loaded.lines.len(), 4);
        assert_eq!(
            loaded.lines[3].item,
            SessionItem::turn_finished("t1", StopReason::Completed)
        );

        recorder.close().await;
    }

    /// `record` не ждёт диск: серия записей уходит в канал мгновенно, а `close`
    /// обязан дослать всё до единого элемента.
    #[tokio::test]
    async fn close_drains_everything_recorded_without_blocking() {
        let dir = TempDir::new();
        let recorder = SessionRecorder::create(dir.path(), meta()).await.unwrap();
        let path = recorder.path().to_path_buf();

        const COUNT: usize = 500;
        for index in 0..COUNT {
            recorder.record(SessionItem::turn_started(format!("turn_{index}")));
        }
        recorder.close().await;

        let loaded = load_session(&path).await.unwrap();
        assert_eq!(loaded.skipped, 0);
        assert_eq!(loaded.lines.len(), COUNT + 1, "meta + все записанные ходы");
        assert_eq!(
            loaded.lines[COUNT].item,
            SessionItem::turn_started(format!("turn_{}", COUNT - 1))
        );
    }

    #[test]
    fn session_line_is_flat_json() {
        let line = SessionLine {
            ts: "2026-08-24T09:00:00Z".into(),
            item: SessionItem::Meta {
                id: "s1".into(),
                cwd: "/w".into(),
                model: "m".into(),
                provider: "p".into(),
                memory: None,
            },
        };
        assert_eq!(
            serde_json::to_string(&line).unwrap(),
            r#"{"ts":"2026-08-24T09:00:00Z","type":"meta","id":"s1","cwd":"/w","model":"m","provider":"p"}"#
        );
    }

    /// Память в шапке: путь и размер; без памяти ключа нет вовсе.
    #[tokio::test]
    async fn meta_records_the_injected_memory() {
        let dir = TempDir::new();
        let meta = meta().with_memory("/work/mark2/.mark2.memory.md", 120);
        let recorder = SessionRecorder::create(dir.path(), meta.clone())
            .await
            .unwrap();
        let path = recorder.path().to_path_buf();
        recorder.close().await;

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            raw.contains(r#""memory":{"path":"/work/mark2/.mark2.memory.md","bytes":120}"#),
            "{raw}"
        );
        let loaded = load_session(&path).await.unwrap();
        assert_eq!(loaded.lines[0].item, SessionItem::from(meta));

        // Старая шапка без ключа читается: памяти не было.
        let old: SessionLine = serde_json::from_str(
            r#"{"ts":"2026-08-24T09:00:00Z","type":"meta","id":"s1","cwd":"/w","model":"m","provider":"p"}"#,
        )
        .unwrap();
        assert!(matches!(old.item, SessionItem::Meta { memory: None, .. }));
    }

    #[test]
    fn every_item_variant_round_trips_and_is_snake_case() {
        let mut items = all_items();
        items.push(SessionItem::from(meta()));
        let mut tags: Vec<String> = Vec::new();
        for item in items {
            let line = SessionLine::new(Timestamp::now(), item.clone());
            let json = serde_json::to_string(&line).unwrap();
            let back: SessionLine = serde_json::from_str(&json).unwrap();
            assert_eq!(back, line, "round-trip упал на {json}");
            let tag = serde_json::to_value(&item).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_string();
            if !tags.contains(&tag) {
                tags.push(tag);
            }
        }
        tags.sort();
        assert_eq!(
            tags,
            [
                "compacted",
                "message",
                "meta",
                "turn_finished",
                "turn_started"
            ]
        );
    }

    #[test]
    fn default_base_dir_points_at_dot_mark2_sessions() {
        let path = default_base_dir().unwrap();
        assert!(path.ends_with(".mark2/sessions"), "{}", path.display());
    }
}
