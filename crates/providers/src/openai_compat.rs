//! Провайдер диалекта OpenAI Chat Completions.
//!
//! Один код обслуживает llama-server (`--jinja`), ollama, LM Studio, vllm и
//! OpenRouter. Держится это на двух правилах:
//!
//! * **толерантный serde на ответах** — `#[serde(default)]` на всех
//!   необязательных полях, незнакомые поля молча игнорируются: сервера
//!   добавляют своё (`reasoning_content`, `timings`, `system_fingerprint`) и
//!   опускают чужое;
//! * **детерминированная сериализация запроса** — порядок полей задан
//!   объявлением структур, порядок ключей в аргументах тулов сохраняет
//!   `serde_json` с фичей `preserve_order`. Один и тот же вход даёт
//!   байт-в-байт один и тот же префикс промпта, KV-кэш локального сервера не
//!   инвалидируется.
//!
//! Диалектные поля прячутся за [`CompatOptions`]: `cache_prompt` и `id_slot`
//! понимает только llama-server, остальные их игнорируют.
//!
//! Ретраи — только **до** начала стрима (connect/таймаут/429/5xx), максимум
//! [`MAX_ATTEMPTS`] попыток: мид-стрим повтор продублировал бы уже отданные
//! дельты.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use mark2_core::message::{ContentBlock, Message, Role, Usage};
use mark2_core::provider::{
    MessageStream, Provider, ProviderError, ProviderRequest, StreamChunk, ToolDef,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::json_repair;
use crate::sse;

/// Имя провайдера в событиях и логах.
pub const PROVIDER_NAME: &str = "openai_compat";

/// Ключ, под которым в аргументах тула лежит неразобранная строка.
///
/// Ставится, когда провалились и строгий парс, и [`json_repair`]. Аргументы
/// намеренно не выбрасываются: валидация схемой в реестре тулов отклонит их и
/// вернёт модели текст ошибки (retry-with-error).
pub const RAW_ARGUMENTS_KEY: &str = "__mark2_raw_arguments";

/// Флаг «аргументы не разобраны» рядом с [`RAW_ARGUMENTS_KEY`].
pub const UNPARSED_MARKER_KEY: &str = "__mark2_unparsed";

/// Максимум попыток отправки запроса (первая + 2 ретрая).
pub const MAX_ATTEMPTS: u32 = 3;

/// База экспоненциального бэкоффа между попытками.
const BACKOFF_BASE: Duration = Duration::from_millis(250);

/// Потолок паузы между попытками (в т.ч. по `Retry-After`).
const BACKOFF_CAP: Duration = Duration::from_secs(20);

/// Сколько байт тела ошибки сохраняем в [`ProviderError::Http`].
const ERROR_BODY_LIMIT: usize = 2048;

/// Опции диалектов OpenAI-совместимых серверов.
///
/// Оба поля — про llama-server; прочие сервера их игнорируют, поэтому
/// включать их безопасно.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CompatOptions {
    /// Просить сервер переиспользовать KV-кэш промпта (`cache_prompt: true`).
    pub cache_prompt: bool,
    /// Пин на конкретный слот сервера (`id_slot`) — держит кэш сессии.
    pub slot_id: Option<i64>,
}

impl CompatOptions {
    /// Пресет llama-server: кэш промпта включён, слот опционально пинится.
    pub fn llama_server(slot_id: Option<i64>) -> Self {
        Self {
            cache_prompt: true,
            slot_id,
        }
    }
}

/// Провайдер OpenAI-совместимого сервера.
pub struct OpenAiCompatProvider {
    endpoint: String,
    api_key: Option<String>,
    options: CompatOptions,
    client: reqwest::Client,
}

impl std::fmt::Debug for OpenAiCompatProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Ключ в Debug не светим.
        f.debug_struct("OpenAiCompatProvider")
            .field("endpoint", &self.endpoint)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("options", &self.options)
            .finish()
    }
}

impl OpenAiCompatProvider {
    /// Провайдер поверх базового URL.
    ///
    /// `base_url` указывается ровно так, как его ждёт сервер:
    /// `http://localhost:8080/v1`, `http://localhost:11434/v1`,
    /// `https://openrouter.ai/api/v1`. Путь `/chat/completions` дописывается
    /// сам (если он уже есть в URL — не дублируется).
    pub fn new(base_url: impl Into<String>, api_key: Option<String>, opts: CompatOptions) -> Self {
        Self::with_client(base_url, api_key, opts, reqwest::Client::new())
    }

    /// То же, но со своим HTTP-клиентом (таймауты, прокси, пул соединений).
    pub fn with_client(
        base_url: impl Into<String>,
        api_key: Option<String>,
        opts: CompatOptions,
        client: reqwest::Client,
    ) -> Self {
        Self {
            endpoint: chat_completions_endpoint(&base_url.into()),
            api_key: api_key.filter(|key| !key.is_empty()),
            options: opts,
            client,
        }
    }

    /// Полный URL, на который уходит запрос.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Текущие опции диалекта.
    pub fn options(&self) -> CompatOptions {
        self.options
    }

    /// Отправить запрос, повторяя ретраебельные отказы до начала стрима.
    async fn send_with_retries(
        &self,
        body: &ChatRequest,
    ) -> Result<reqwest::Response, ProviderError> {
        let mut attempt = 1;
        loop {
            let mut request = self
                .client
                .post(&self.endpoint)
                .header("accept", "text/event-stream");
            if let Some(key) = &self.api_key {
                request = request.bearer_auth(key);
            }

            let (error, retry_after) = match request.json(body).send().await {
                Ok(response) if response.status().is_success() => return Ok(response),
                Ok(response) => {
                    let status = response.status().as_u16();
                    let retry_after = response
                        .headers()
                        .get("retry-after")
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_owned);
                    let text = response.text().await.unwrap_or_default();
                    let error = map_error_body(status, &text);
                    // Переполнение контекста не лечится повтором — это работа
                    // компакции (task 7), отдаём наверх сразу.
                    if !is_retryable_status(status)
                        || matches!(error, ProviderError::ContextLengthExceeded)
                    {
                        return Err(error);
                    }
                    (error, retry_after)
                }
                Err(err) => {
                    let error = ProviderError::Network(err.to_string());
                    if !is_retryable_transport(&err) {
                        return Err(error);
                    }
                    (error, None)
                }
            };

            if attempt >= MAX_ATTEMPTS {
                return Err(error);
            }
            let delay = retry_delay(attempt, retry_after.as_deref());
            tracing::warn!(
                attempt,
                delay_ms = delay.as_millis() as u64,
                error = %error,
                "повтор запроса к провайдеру"
            );
            tokio::time::sleep(delay).await;
            attempt += 1;
        }
    }
}

#[async_trait]
impl Provider for OpenAiCompatProvider {
    fn name(&self) -> &str {
        PROVIDER_NAME
    }

    async fn stream(&self, req: ProviderRequest) -> Result<MessageStream, ProviderError> {
        let body = to_wire(&req, self.options);
        let response = self.send_with_retries(&body).await?;

        // Часть серверов отвечает 200 и JSON-ошибкой вместо SSE. Без этой
        // проверки такой ответ дал бы пустой стрим и пустое сообщение.
        if is_json_response(
            response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
        ) {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            return Err(map_error_body(status, &text));
        }

        Ok(Box::pin(assemble(sse::response_data_stream(response))))
    }
}

/// Ответ пришёл JSON'ом, а не потоком событий.
///
/// Проверяем только явный `application/json`: отсутствующий или нестандартный
/// `content-type` считаем стримом, чтобы не ломаться на самодельных серверах.
fn is_json_response(content_type: Option<&str>) -> bool {
    content_type.is_some_and(|value| value.to_ascii_lowercase().contains("application/json"))
}

/// Дописать `/chat/completions` к базовому URL, не дублируя уже указанный путь.
fn chat_completions_endpoint(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/chat/completions") {
        base.to_string()
    } else {
        format!("{base}/chat/completions")
    }
}

// ───────────────────────────── wire-типы ─────────────────────────────

/// Тело запроса. Порядок полей фиксирован — от него зависит стабильность
/// префикса и, как следствие, попадание в KV-кэш.
#[derive(Debug, Clone, PartialEq, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<WireMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<WireTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    stream: bool,
    stream_options: StreamOptions,
    /// llama-server: переиспользовать KV-кэш промпта.
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_prompt: Option<bool>,
    /// llama-server: пин на слот сервера.
    #[serde(skip_serializing_if = "Option::is_none")]
    id_slot: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct StreamOptions {
    include_usage: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct WireMessage {
    role: String,
    /// `null` для assistant-сообщения, состоящего только из тул-коллов, —
    /// именно так его отдаёт и принимает сам OpenAI.
    #[serde(default)]
    content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<WireToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct WireToolCall {
    /// Есть только в стриминговых дельтах — по нему склеиваются фрагменты.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    #[serde(default)]
    function: WireFunctionCall,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct WireFunctionCall {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// Строка JSON: в стриме приходит кусками, склеивается конкатенацией.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    arguments: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct WireTool {
    #[serde(rename = "type")]
    kind: &'static str,
    function: WireToolFunction,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct WireToolFunction {
    name: String,
    description: String,
    parameters: Value,
}

/// Чанк стрима (`object: "chat.completion.chunk"`).
#[derive(Debug, Clone, Default, Deserialize)]
struct ChatChunk {
    #[serde(default)]
    choices: Vec<ChunkChoice>,
    #[serde(default)]
    usage: Option<WireUsage>,
    /// llama-server отдаёт свои счётчики отдельным полем.
    #[serde(default)]
    timings: Option<WireTimings>,
    /// Сервера умеют слать ошибку прямо в SSE-кадре.
    #[serde(default)]
    error: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ChunkChoice {
    #[serde(default)]
    delta: Delta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct Delta {
    #[serde(default)]
    content: Option<String>,
    /// llama-server / vllm / SGLang: канал рассуждений.
    #[serde(default)]
    reasoning_content: Option<String>,
    /// OpenRouter называет то же самое короче.
    #[serde(default)]
    reasoning: Option<String>,
    #[serde(default)]
    tool_calls: Vec<WireToolCall>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct WireUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    #[serde(default)]
    prompt_tokens_details: Option<PromptTokensDetails>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct PromptTokensDetails {
    #[serde(default)]
    cached_tokens: Option<u64>,
    #[serde(default)]
    cache_write_tokens: Option<u64>,
}

/// `timings` llama-server: фолбэк, когда сервер не прислал `usage`.
#[derive(Debug, Clone, Default, Deserialize)]
struct WireTimings {
    /// Токенов промпта, реально посчитанных (без попавших в кэш).
    #[serde(default)]
    prompt_n: Option<u64>,
    /// Сгенерированных токенов.
    #[serde(default)]
    predicted_n: Option<u64>,
    /// Токенов промпта, переиспользованных из KV-кэша.
    #[serde(default)]
    cache_n: Option<u64>,
}

// ───────────────────────────── to_wire ─────────────────────────────

/// Нейтральный запрос → тело Chat Completions.
///
/// * `system` уходит первым сообщением `role: "system"`;
/// * [`ContentBlock::ToolUse`] → `assistant.tool_calls`;
/// * [`ContentBlock::ToolResult`] → отдельные сообщения `role: "tool"` с
///   `tool_call_id`;
/// * [`ContentBlock::Thinking`] на wire не уходит: сервера не ждут чужой
///   reasoning в истории, а его присутствие ломает шаблоны.
fn to_wire(req: &ProviderRequest, opts: CompatOptions) -> ChatRequest {
    let mut messages = Vec::with_capacity(req.messages.len() + 1);
    if !req.system.is_empty() {
        messages.push(WireMessage {
            role: Role::System.as_str().to_string(),
            content: Some(req.system.clone()),
            ..WireMessage::default()
        });
    }
    for message in &req.messages {
        push_wire_messages(message, &mut messages);
    }

    ChatRequest {
        model: req.model.clone(),
        messages,
        tools: req.tools.iter().map(to_wire_tool).collect(),
        max_tokens: req.max_tokens,
        temperature: req.temperature,
        stream: true,
        stream_options: StreamOptions {
            include_usage: true,
        },
        cache_prompt: opts.cache_prompt.then_some(true),
        id_slot: opts.slot_id,
    }
}

fn to_wire_tool(tool: &ToolDef) -> WireTool {
    WireTool {
        kind: "function",
        function: WireToolFunction {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: tool.input_schema.clone(),
        },
    }
}

/// Одно нейтральное сообщение может дать несколько wire-сообщений: текст с
/// тул-коллами плюс по одному `role: "tool"` на каждый результат.
fn push_wire_messages(message: &Message, out: &mut Vec<WireMessage>) {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut results = Vec::new();

    for block in &message.content {
        match block {
            ContentBlock::Text { text: chunk } => text.push_str(chunk),
            ContentBlock::Thinking { .. } => {}
            ContentBlock::ToolUse { id, name, input } => tool_calls.push(WireToolCall {
                index: None,
                id: Some(id.clone()),
                kind: Some("function".to_string()),
                function: WireFunctionCall {
                    name: Some(name.clone()),
                    arguments: Some(serde_json::to_string(input).unwrap_or_else(|_| "{}".into())),
                },
            }),
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } => results.push(WireMessage {
                role: Role::Tool.as_str().to_string(),
                content: Some(content.clone()),
                tool_calls: Vec::new(),
                tool_call_id: Some(tool_use_id.clone()),
            }),
        }
    }

    if !text.is_empty() || !tool_calls.is_empty() {
        out.push(WireMessage {
            role: message.role.as_str().to_string(),
            content: (!text.is_empty()).then_some(text),
            tool_calls,
            tool_call_id: None,
        });
    }
    out.extend(results);
}

// ──────────────────────── сборка стрима ────────────────────────

/// Незавершённый тул-колл: собирается по индексу из дельт.
#[derive(Debug, Clone, Default)]
struct PartialToolCall {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

/// Инкрементальная сборка ответа из SSE-чанков.
///
/// Живёт ровно один стрим: копит текст, рассуждения и аргументы тул-коллов,
/// а в конце (`[DONE]`, `finish_reason` + конец потока или просто обрыв)
/// отдаёт готовое assistant-сообщение.
#[derive(Debug, Default)]
struct StreamAssembler {
    text: String,
    thinking: String,
    calls: Vec<PartialToolCall>,
    usage: Usage,
    finish_reason: Option<String>,
    completed: bool,
}

impl StreamAssembler {
    /// Разобрать один `data:`-payload и выдать дельты для потребителя.
    fn push(&mut self, payload: &str) -> Result<Vec<StreamChunk>, ProviderError> {
        let payload = payload.trim();
        if payload.is_empty() {
            return Ok(Vec::new());
        }
        let chunk: ChatChunk = serde_json::from_str(payload)
            .map_err(|err| ProviderError::Parse(format!("чанк стрима: {err}: {payload}")))?;

        if let Some(error) = chunk.error.filter(|error| !error.is_null()) {
            return Err(map_error_body(200, &error.to_string()));
        }
        self.absorb_usage(chunk.usage, chunk.timings);

        let mut out = Vec::new();
        for choice in chunk.choices {
            if let Some(text) = choice
                .delta
                .reasoning_content
                .or(choice.delta.reasoning)
                .filter(|text| !text.is_empty())
            {
                self.thinking.push_str(&text);
                out.push(StreamChunk::ThinkingDelta(text));
            }
            if let Some(text) = choice.delta.content.filter(|text| !text.is_empty()) {
                self.text.push_str(&text);
                out.push(StreamChunk::TextDelta(text));
            }
            for call in choice.delta.tool_calls {
                if let Some(delta) = self.absorb_tool_call(call) {
                    out.push(delta);
                }
            }
            if let Some(reason) = choice.finish_reason {
                self.finish_reason = Some(reason);
            }
        }
        Ok(out)
    }

    fn absorb_usage(&mut self, usage: Option<WireUsage>, timings: Option<WireTimings>) {
        if let Some(usage) = usage {
            self.usage.input_tokens = usage.prompt_tokens;
            self.usage.output_tokens = usage.completion_tokens;
            if let Some(details) = usage.prompt_tokens_details {
                if details.cached_tokens.is_some() {
                    self.usage.cache_read_tokens = details.cached_tokens;
                }
                if details.cache_write_tokens.is_some() {
                    self.usage.cache_write_tokens = details.cache_write_tokens;
                }
            }
        }
        if let Some(timings) = timings {
            if self.usage.input_tokens == 0 {
                self.usage.input_tokens =
                    timings.prompt_n.unwrap_or(0) + timings.cache_n.unwrap_or(0);
            }
            if self.usage.output_tokens == 0 {
                self.usage.output_tokens = timings.predicted_n.unwrap_or(0);
            }
            if self.usage.cache_read_tokens.is_none() {
                self.usage.cache_read_tokens = timings.cache_n;
            }
        }
    }

    /// Слить фрагмент тул-колла в состояние; вернуть дельту, если в нём что-то
    /// было.
    fn absorb_tool_call(&mut self, call: WireToolCall) -> Option<StreamChunk> {
        // `index` есть у OpenAI/vllm/llama-server; если сервер его не шлёт —
        // новый `id` открывает новый слот, иначе фрагмент идёт в последний.
        let index = call.index.unwrap_or_else(|| {
            if call.id.is_some() {
                self.calls.len()
            } else {
                self.calls.len().saturating_sub(1)
            }
        });
        while self.calls.len() <= index {
            self.calls.push(PartialToolCall::default());
        }
        let slot = &mut self.calls[index];

        let id = call.id.filter(|id| !id.is_empty());
        if let Some(id) = &id {
            slot.id = Some(id.clone());
        }
        // Имя приходит целиком в первом фрагменте; повторы игнорируем.
        let name = call.function.name.filter(|name| !name.is_empty());
        if let Some(name) = &name
            && slot.name.is_none()
        {
            slot.name = Some(name.clone());
        }
        let args_delta = call.function.arguments.unwrap_or_default();
        slot.arguments.push_str(&args_delta);

        if id.is_none() && name.is_none() && args_delta.is_empty() {
            return None;
        }
        Some(StreamChunk::ToolCallDelta {
            index,
            id,
            name,
            args_delta,
        })
    }

    /// Собрать итоговое сообщение. Вызывается ровно один раз.
    fn complete(&mut self) -> StreamChunk {
        self.completed = true;
        if self.finish_reason.as_deref() == Some("length") {
            tracing::warn!("ответ модели обрезан по лимиту токенов (finish_reason=length)");
        }

        let mut content = Vec::with_capacity(2 + self.calls.len());
        if !self.thinking.is_empty() {
            content.push(ContentBlock::thinking(self.thinking.clone()));
        }
        if !self.text.is_empty() {
            content.push(ContentBlock::text(self.text.clone()));
        }
        for (position, call) in self.calls.iter().enumerate() {
            let name = call.name.clone().unwrap_or_default();
            let id = call
                .id
                .clone()
                .unwrap_or_else(|| format!("call_{position}_{name}"));
            content.push(ContentBlock::tool_use(
                id,
                name.clone(),
                parse_arguments(&call.arguments, &name),
            ));
        }

        StreamChunk::Complete {
            message: Message::new(Role::Assistant, content),
            usage: self.usage,
        }
    }
}

/// Строка аргументов → [`Value`]: строгий парс, затем [`json_repair`], затем —
/// пометка «не разобрано».
fn parse_arguments(raw: &str, tool: &str) -> Value {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Value::Object(Map::new());
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(value) => unwrap_double_encoded(value),
        Err(_) => match json_repair::parse_tolerant(trimmed) {
            Ok(value) => {
                tracing::warn!(tool, "аргументы тул-колла восстановлены json_repair");
                unwrap_double_encoded(value)
            }
            Err(_) => {
                tracing::warn!(tool, "аргументы тул-колла не разобраны, отдаём строкой");
                unparsed_arguments(trimmed)
            }
        },
    }
}

/// Некоторые шаблоны отдают аргументы дважды закодированными:
/// `"{\"path\":\"a\"}"`. Разворачиваем, если внутри объект.
fn unwrap_double_encoded(value: Value) -> Value {
    if let Value::String(inner) = &value
        && let Ok(decoded @ Value::Object(_)) = serde_json::from_str::<Value>(inner)
    {
        return decoded;
    }
    value
}

fn unparsed_arguments(raw: &str) -> Value {
    let mut map = Map::new();
    map.insert(UNPARSED_MARKER_KEY.to_string(), Value::Bool(true));
    map.insert(
        RAW_ARGUMENTS_KEY.to_string(),
        Value::String(raw.to_string()),
    );
    Value::Object(map)
}

/// Состояние сборки: исходный поток payload'ов + очередь готовых чанков.
struct AssembleState<S> {
    data: Pin<Box<S>>,
    assembler: StreamAssembler,
    queue: VecDeque<Result<StreamChunk, ProviderError>>,
    finished: bool,
}

/// Поток `data:`-payload'ов → поток [`StreamChunk`].
///
/// Выделено отдельной функцией, чтобы тесты гоняли ровно тот же парсер, что и
/// боевой путь, но на байтовых фикстурах вместо HTTP.
fn assemble<S>(data: S) -> impl Stream<Item = Result<StreamChunk, ProviderError>> + Send
where
    S: Stream<Item = Result<String, ProviderError>> + Send + 'static,
{
    let state = AssembleState {
        data: Box::pin(data),
        assembler: StreamAssembler::default(),
        queue: VecDeque::new(),
        finished: false,
    };

    futures::stream::unfold(state, |mut state| async move {
        loop {
            if let Some(item) = state.queue.pop_front() {
                return Some((item, state));
            }
            if state.finished {
                return None;
            }
            match state.data.next().await {
                Some(Ok(payload)) => {
                    if sse::is_done(&payload) {
                        state.finished = true;
                        state.queue.push_back(Ok(state.assembler.complete()));
                    } else {
                        match state.assembler.push(&payload) {
                            Ok(chunks) => state.queue.extend(chunks.into_iter().map(Ok)),
                            Err(err) => {
                                state.finished = true;
                                state.queue.push_back(Err(err));
                            }
                        }
                    }
                }
                Some(Err(err)) => {
                    state.finished = true;
                    state.queue.push_back(Err(err));
                }
                None => {
                    // Поток кончился без `[DONE]` — так делают некоторые
                    // сервера; ответ всё равно собран.
                    state.finished = true;
                    if !state.assembler.completed {
                        state.queue.push_back(Ok(state.assembler.complete()));
                    }
                }
            }
        }
    })
}

// ──────────────────────── ошибки и ретраи ────────────────────────

/// Подстроки, по которым переполнение контекста опознаётся у разных серверов.
///
/// Формы обёртки разные (`{"error":{...}}` у OpenAI/llama.cpp/ollama/vllm,
/// `{"object":"error","message":...}` у старых vllm, plain text у самоделок),
/// поэтому ищем подстроку по всему телу в нижнем регистре. Формулировки
/// сверены с исходниками серверов, а не пересказами:
///
/// * OpenAI — `code: "context_length_exceeded"`, текст «This model's maximum
///   context length is N tokens…»;
/// * vllm — тот же текст «maximum context length is», но `type:
///   "BadRequestError"`; в свежих ветках добавилось «longer than the maximum
///   model length»;
/// * llama.cpp — `request (N tokens) exceeds the available context size (M
///   tokens), try increasing it`, `type: "exceed_context_size_error"`;
/// * ollama — «the prompt is longer than the context length currently
///   available to the model…» (в OpenAI-обёртке `invalid_request_error`).
///
/// Намеренно не включены расплывчатые формулировки вроде «context window» или
/// «too many tokens»: ложное срабатывание запустит бессмысленную компакцию.
const CONTEXT_OVERFLOW_MARKERS: &[&str] = &[
    // OpenAI / vllm / OpenRouter.
    "context_length_exceeded",
    "context length exceeded",
    "maximum context length",
    "longer than the maximum model length",
    // llama.cpp server.
    "exceeds the available context size",
    "exceed_context_size_error",
    // ollama.
    "longer than the context length",
    "input length exceeds the context length",
];

/// HTTP-статус + тело → [`ProviderError`].
///
/// Переполнение контекста вытаскивается отдельным вариантом: на нём построена
/// компакция (task 7).
fn map_error_body(status: u16, body: &str) -> ProviderError {
    if is_context_overflow(body) {
        return ProviderError::ContextLengthExceeded;
    }
    ProviderError::Http {
        status,
        body: truncate(body, ERROR_BODY_LIMIT),
    }
}

fn is_context_overflow(body: &str) -> bool {
    let body = body.to_lowercase();
    CONTEXT_OVERFLOW_MARKERS
        .iter()
        .any(|marker| body.contains(marker))
}

fn truncate(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = text[..end].to_string();
    let _ = write!(out, "… (обрезано, всего {} байт)", text.len());
    out
}

/// Ретраим только то, что имеет шанс пройти со второй попытки.
fn is_retryable_status(status: u16) -> bool {
    matches!(status, 408 | 409 | 425 | 429) || (500..600).contains(&status)
}

fn is_retryable_transport(err: &reqwest::Error) -> bool {
    err.is_connect() || err.is_timeout() || err.is_request()
}

/// Экспоненциальный бэкофф; `Retry-After` (секунды) уважается, если сервер его
/// прислал.
fn retry_delay(attempt: u32, retry_after: Option<&str>) -> Duration {
    if let Some(seconds) = retry_after.and_then(|value| value.trim().parse::<u64>().ok()) {
        return Duration::from_secs(seconds).min(BACKOFF_CAP);
    }
    let factor = 1u32 << (attempt.saturating_sub(1)).min(16);
    (BACKOFF_BASE * factor).min(BACKOFF_CAP)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mark2_core::provider::ToolDef;
    use serde_json::json;
    use std::convert::Infallible;

    // ─────────────────────── фикстуры и хелперы ───────────────────────

    fn read_tool() -> ToolDef {
        ToolDef {
            name: "read".into(),
            description: "Читает файл".into(),
            input_schema: json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }),
        }
    }

    /// Прогнать сырые SSE-байты через тот же путь, что и боевой стрим.
    async fn run_sse(raw: &str) -> Vec<Result<StreamChunk, ProviderError>> {
        let bytes: Vec<Result<Vec<u8>, Infallible>> = raw
            .as_bytes()
            .chunks(17) // намеренно рвём кадры по границам чанков
            .map(|c| Ok(c.to_vec()))
            .collect();
        assemble(sse::data_stream(futures::stream::iter(bytes)))
            .collect()
            .await
    }

    async fn run_ok(raw: &str) -> Vec<StreamChunk> {
        run_sse(raw)
            .await
            .into_iter()
            .map(|item| item.expect("стрим не должен падать"))
            .collect()
    }

    fn final_message(chunks: &[StreamChunk]) -> (&Message, &Usage) {
        match chunks.last().expect("пустой стрим") {
            StreamChunk::Complete { message, usage } => (message, usage),
            other => panic!("последним чанком ожидался Complete, получен {other:?}"),
        }
    }

    // ─────────────────────────── to_wire ───────────────────────────

    #[test]
    fn to_wire_snapshot_with_system_tools_and_tool_history() {
        let req = ProviderRequest::new("qwen3-coder", "ты агент mark2")
            .with_messages(vec![
                Message::user("почитай a.txt"),
                Message::new(
                    Role::Assistant,
                    vec![
                        ContentBlock::thinking("надо позвать read"),
                        ContentBlock::text("сейчас посмотрю"),
                        ContentBlock::tool_use("call_1", "read", json!({"path": "a.txt"})),
                    ],
                ),
                Message::tool_result("call_1", "содержимое"),
                Message::assistant("готово"),
            ])
            .with_tools(vec![read_tool()])
            .with_max_tokens(1024)
            .with_temperature(0.2);

        let body = to_wire(&req, CompatOptions::llama_server(Some(3)));
        let json = serde_json::to_string(&body).unwrap();

        assert_eq!(
            json,
            concat!(
                r#"{"model":"qwen3-coder","messages":["#,
                r#"{"role":"system","content":"ты агент mark2"},"#,
                r#"{"role":"user","content":"почитай a.txt"},"#,
                r#"{"role":"assistant","content":"сейчас посмотрю","tool_calls":[{"id":"call_1","type":"function","function":{"name":"read","arguments":"{\"path\":\"a.txt\"}"}}]},"#,
                r#"{"role":"tool","content":"содержимое","tool_call_id":"call_1"},"#,
                r#"{"role":"assistant","content":"готово"}"#,
                r#"],"tools":[{"type":"function","function":{"name":"read","description":"Читает файл","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}}],"#,
                r#""max_tokens":1024,"temperature":0.2,"stream":true,"stream_options":{"include_usage":true},"cache_prompt":true,"id_slot":3}"#,
            )
        );
    }

    #[test]
    fn to_wire_is_byte_stable() {
        let req = ProviderRequest::new("m", "s")
            .with_messages(vec![Message::new(
                Role::Assistant,
                vec![ContentBlock::tool_use(
                    "c1",
                    "write",
                    json!({"zebra": 1, "alpha": 2, "nested": {"z": 1, "a": 2}}),
                )],
            )])
            .with_tools(vec![read_tool()]);

        let first = serde_json::to_string(&to_wire(&req, CompatOptions::default())).unwrap();
        for _ in 0..16 {
            assert_eq!(
                serde_json::to_string(&to_wire(&req, CompatOptions::default())).unwrap(),
                first
            );
        }
        // Порядок ключей в аргументах тула не переставляется.
        assert!(first.contains(r#"\"zebra\":1,\"alpha\":2,\"nested\":{\"z\":1,\"a\":2}"#));
    }

    #[test]
    fn to_wire_omits_optional_and_dialect_fields_by_default() {
        let req = ProviderRequest::new("m", "").with_messages(vec![Message::user("hi")]);
        let json = serde_json::to_string(&to_wire(&req, CompatOptions::default())).unwrap();
        assert_eq!(
            json,
            r#"{"model":"m","messages":[{"role":"user","content":"hi"}],"stream":true,"stream_options":{"include_usage":true}}"#
        );
    }

    #[test]
    fn to_wire_assistant_with_only_tool_calls_sends_null_content() {
        let req = ProviderRequest::new("m", "").with_messages(vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::tool_use(
                "c1",
                "bash",
                json!({"command": "ls"}),
            )],
        )]);
        let json = serde_json::to_value(to_wire(&req, CompatOptions::default())).unwrap();
        assert_eq!(json["messages"][0]["content"], Value::Null);
        assert_eq!(json["messages"][0]["tool_calls"][0]["id"], json!("c1"));
    }

    #[test]
    fn to_wire_maps_several_tool_results_to_separate_tool_messages() {
        let req = ProviderRequest::new("m", "").with_messages(vec![Message::tool(vec![
            ContentBlock::tool_result("c1", "ok"),
            ContentBlock::tool_error("c2", "boom"),
        ])]);
        let json = serde_json::to_value(to_wire(&req, CompatOptions::default())).unwrap();
        let messages = json["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], json!("tool"));
        assert_eq!(messages[0]["tool_call_id"], json!("c1"));
        assert_eq!(messages[1]["content"], json!("boom"));
    }

    #[test]
    fn to_wire_drops_thinking_blocks() {
        let req = ProviderRequest::new("m", "").with_messages(vec![Message::new(
            Role::Assistant,
            vec![
                ContentBlock::thinking("секрет"),
                ContentBlock::text("ответ"),
            ],
        )]);
        let json = serde_json::to_string(&to_wire(&req, CompatOptions::default())).unwrap();
        assert!(!json.contains("секрет"), "{json}");
        assert!(json.contains("ответ"));
    }

    #[test]
    fn endpoint_is_built_without_duplication() {
        assert_eq!(
            chat_completions_endpoint("http://localhost:8080/v1"),
            "http://localhost:8080/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_endpoint("http://localhost:8080/v1/"),
            "http://localhost:8080/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_endpoint("http://localhost:8080/v1/chat/completions"),
            "http://localhost:8080/v1/chat/completions"
        );
    }

    #[test]
    fn provider_metadata_and_key_redaction() {
        let provider = OpenAiCompatProvider::new(
            "https://openrouter.ai/api/v1",
            Some("sk-secret".into()),
            CompatOptions::llama_server(Some(1)),
        );
        assert_eq!(provider.name(), PROVIDER_NAME);
        assert_eq!(
            provider.endpoint(),
            "https://openrouter.ai/api/v1/chat/completions"
        );
        assert_eq!(provider.options(), CompatOptions::llama_server(Some(1)));
        let debug = format!("{provider:?}");
        assert!(!debug.contains("sk-secret"), "{debug}");
        assert!(debug.contains("<redacted>"));

        // Пустой ключ равносилен отсутствию — заголовок не шлём.
        let anon = OpenAiCompatProvider::new(
            "http://localhost:8080/v1",
            Some(String::new()),
            CompatOptions::default(),
        );
        assert!(anon.api_key.is_none());
    }

    // ───────────────────────── разбор стрима ─────────────────────────

    /// Канонический стрим OpenAI: role-чанк, текст кусками, finish_reason,
    /// отдельный usage-чанк (`stream_options.include_usage`), `[DONE]`.
    const OPENAI_TEXT_STREAM: &str = concat!(
        "data: {\"id\":\"chatcmpl-123\",\"object\":\"chat.completion.chunk\",\"created\":1694268190,\"model\":\"gpt-4o-mini\",\"system_fingerprint\":\"fp_44709d6fcb\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"},\"logprobs\":null,\"finish_reason\":null}],\"usage\":null}\n\n",
        // `obfuscation` — новое поле OpenAI; незнакомые поля должны молча игнорироваться.
        "data: {\"id\":\"chatcmpl-123\",\"object\":\"chat.completion.chunk\",\"created\":1694268190,\"model\":\"gpt-4o-mini\",\"system_fingerprint\":\"fp_44709d6fcb\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Привет\"},\"logprobs\":null,\"finish_reason\":null}],\"usage\":null,\"obfuscation\":\"p9K3xT6w\"}\n\n",
        "data: {\"id\":\"chatcmpl-123\",\"object\":\"chat.completion.chunk\",\"created\":1694268190,\"model\":\"gpt-4o-mini\",\"system_fingerprint\":\"fp_44709d6fcb\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\", мир\"},\"logprobs\":null,\"finish_reason\":null}],\"usage\":null}\n\n",
        "data: {\"id\":\"chatcmpl-123\",\"object\":\"chat.completion.chunk\",\"created\":1694268190,\"model\":\"gpt-4o-mini\",\"system_fingerprint\":\"fp_44709d6fcb\",\"choices\":[{\"index\":0,\"delta\":{},\"logprobs\":null,\"finish_reason\":\"stop\"}],\"usage\":null}\n\n",
        "data: {\"id\":\"chatcmpl-123\",\"object\":\"chat.completion.chunk\",\"created\":1694268190,\"model\":\"gpt-4o-mini\",\"system_fingerprint\":\"fp_44709d6fcb\",\"choices\":[],\"usage\":{\"prompt_tokens\":1245,\"completion_tokens\":7,\"total_tokens\":1252,\"prompt_tokens_details\":{\"cached_tokens\":1024,\"audio_tokens\":0}}}\n\n",
        "data: [DONE]\n\n",
    );

    #[tokio::test]
    async fn openai_text_stream_assembles_message_and_usage() {
        let chunks = run_ok(OPENAI_TEXT_STREAM).await;
        assert_eq!(
            chunks[..2],
            [
                StreamChunk::TextDelta("Привет".into()),
                StreamChunk::TextDelta(", мир".into()),
            ]
        );

        let (message, usage) = final_message(&chunks);
        assert_eq!(message.role, Role::Assistant);
        assert_eq!(message.content, vec![ContentBlock::text("Привет, мир")]);
        assert_eq!(usage.input_tokens, 1245);
        assert_eq!(usage.output_tokens, 7);
        assert_eq!(usage.cache_read_tokens, Some(1024));
    }

    /// Тул-колл OpenAI: id и имя в первом фрагменте, аргументы — кусками.
    const OPENAI_TOOL_CALL_STREAM: &str = concat!(
        "data: {\"id\":\"chatcmpl-9\",\"object\":\"chat.completion.chunk\",\"created\":1699896916,\"model\":\"gpt-4o-mini\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":null,\"tool_calls\":[{\"index\":0,\"id\":\"call_abc123\",\"type\":\"function\",\"function\":{\"name\":\"read\",\"arguments\":\"\"}}]},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-9\",\"object\":\"chat.completion.chunk\",\"created\":1699896916,\"model\":\"gpt-4o-mini\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"pa\"}}]},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-9\",\"object\":\"chat.completion.chunk\",\"created\":1699896916,\"model\":\"gpt-4o-mini\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"th\\\": \\\"a.t\"}}]},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-9\",\"object\":\"chat.completion.chunk\",\"created\":1699896916,\"model\":\"gpt-4o-mini\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"xt\\\"}\"}}]},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-9\",\"object\":\"chat.completion.chunk\",\"created\":1699896916,\"model\":\"gpt-4o-mini\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    );

    #[tokio::test]
    async fn tool_call_split_across_chunks_is_reassembled() {
        let chunks = run_ok(OPENAI_TOOL_CALL_STREAM).await;

        let deltas: Vec<_> = chunks
            .iter()
            .filter_map(|chunk| match chunk {
                StreamChunk::ToolCallDelta {
                    index,
                    id,
                    name,
                    args_delta,
                } => Some((*index, id.clone(), name.clone(), args_delta.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            deltas,
            vec![
                (
                    0,
                    Some("call_abc123".into()),
                    Some("read".into()),
                    String::new()
                ),
                (0, None, None, "{\"pa".into()),
                (0, None, None, "th\": \"a.t".into()),
                (0, None, None, "xt\"}".into()),
            ]
        );

        let (message, _) = final_message(&chunks);
        assert_eq!(
            message.content,
            vec![ContentBlock::tool_use(
                "call_abc123",
                "read",
                json!({"path": "a.txt"})
            )]
        );
    }

    /// Два параллельных тул-колла различаются только по `index`.
    const PARALLEL_TOOL_CALLS_STREAM: &str = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_a\",\"type\":\"function\",\"function\":{\"name\":\"read\",\"arguments\":\"\"}}]},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":1,\"id\":\"call_b\",\"type\":\"function\",\"function\":{\"name\":\"bash\",\"arguments\":\"\"}}]},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"path\\\":\\\"a\\\"}\"}}]},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":1,\"function\":{\"arguments\":\"{\\\"command\\\":\\\"ls\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    );

    #[tokio::test]
    async fn parallel_tool_calls_are_kept_apart_by_index() {
        let chunks = run_ok(PARALLEL_TOOL_CALLS_STREAM).await;
        let (message, _) = final_message(&chunks);
        assert_eq!(
            message.content,
            vec![
                ContentBlock::tool_use("call_a", "read", json!({"path": "a"})),
                ContentBlock::tool_use("call_b", "bash", json!({"command": "ls"})),
            ]
        );
    }

    /// llama-server: `reasoning_content` в дельте и `timings` в финальном чанке.
    const LLAMA_SERVER_STREAM: &str = concat!(
        "data: {\"choices\":[{\"finish_reason\":null,\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":null,\"reasoning_content\":\"Надо \"}}],\"created\":1755000000,\"id\":\"chatcmpl-abc\",\"model\":\"qwen3-coder\",\"system_fingerprint\":\"b6100-1a2b3c4\",\"object\":\"chat.completion.chunk\"}\n\n",
        "data: {\"choices\":[{\"finish_reason\":null,\"index\":0,\"delta\":{\"reasoning_content\":\"прочитать файл.\"}}],\"created\":1755000000,\"id\":\"chatcmpl-abc\",\"model\":\"qwen3-coder\",\"object\":\"chat.completion.chunk\"}\n\n",
        "data: {\"choices\":[{\"finish_reason\":null,\"index\":0,\"delta\":{\"content\":\"Читаю a.txt\"}}],\"created\":1755000000,\"id\":\"chatcmpl-abc\",\"model\":\"qwen3-coder\",\"object\":\"chat.completion.chunk\"}\n\n",
        "data: {\"choices\":[{\"finish_reason\":\"stop\",\"index\":0,\"delta\":{}}],\"created\":1755000000,\"id\":\"chatcmpl-abc\",\"model\":\"qwen3-coder\",\"object\":\"chat.completion.chunk\",\"usage\":{\"completion_tokens\":9,\"prompt_tokens\":842,\"total_tokens\":851},\"timings\":{\"cache_n\":768,\"prompt_n\":74,\"prompt_ms\":121.5,\"prompt_per_token_ms\":1.64,\"prompt_per_second\":609.0,\"predicted_n\":9,\"predicted_ms\":210.3,\"predicted_per_token_ms\":23.4,\"predicted_per_second\":42.8}}\n\n",
        "data: [DONE]\n\n",
    );

    #[tokio::test]
    async fn llama_server_reasoning_and_timings_are_understood() {
        let chunks = run_ok(LLAMA_SERVER_STREAM).await;
        assert_eq!(
            chunks[..3],
            [
                StreamChunk::ThinkingDelta("Надо ".into()),
                StreamChunk::ThinkingDelta("прочитать файл.".into()),
                StreamChunk::TextDelta("Читаю a.txt".into()),
            ]
        );

        let (message, usage) = final_message(&chunks);
        assert_eq!(
            message.content,
            vec![
                ContentBlock::thinking("Надо прочитать файл."),
                ContentBlock::text("Читаю a.txt"),
            ]
        );
        assert_eq!(usage.input_tokens, 842);
        assert_eq!(usage.output_tokens, 9);
        // `cached_tokens` сервер не прислал — берём из timings.
        assert_eq!(usage.cache_read_tokens, Some(768));
    }

    /// Без `usage` (старые сборки llama-server) считаем по `timings`.
    #[tokio::test]
    async fn timings_are_usage_fallback() {
        let raw = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":null,\"index\":0}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\",\"index\":0}],\"timings\":{\"prompt_n\":100,\"cache_n\":20,\"predicted_n\":5}}\n\n",
            "data: [DONE]\n\n",
        );
        let chunks = run_ok(raw).await;
        let (_, usage) = final_message(&chunks);
        assert_eq!(usage.input_tokens, 120);
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(usage.cache_read_tokens, Some(20));
    }

    /// OpenRouter шлёт reasoning под коротким именем и пинги-комментарии.
    #[tokio::test]
    async fn openrouter_reasoning_field_and_ping_comments() {
        let raw = concat!(
            ": OPENROUTER PROCESSING\n\n",
            "data: {\"id\":\"gen-1\",\"provider\":\"Fireworks\",\"model\":\"qwen/qwen3-coder\",\"object\":\"chat.completion.chunk\",\"created\":1755000000,\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\",\"reasoning\":\"мысль\"},\"finish_reason\":null,\"native_finish_reason\":null,\"logprobs\":null}]}\n\n",
            ": OPENROUTER PROCESSING\n\n",
            "data: {\"id\":\"gen-1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ответ\"},\"finish_reason\":\"stop\",\"native_finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":2,\"total_tokens\":12}}\n\n",
            "data: [DONE]\n\n",
        );
        let chunks = run_ok(raw).await;
        assert_eq!(chunks[0], StreamChunk::ThinkingDelta("мысль".into()));
        let (message, usage) = final_message(&chunks);
        assert_eq!(
            message.content,
            vec![ContentBlock::thinking("мысль"), ContentBlock::text("ответ")]
        );
        assert_eq!(usage.total_tokens(), 12);
    }

    /// Сервер оборвал стрим без `[DONE]` — ответ всё равно собирается.
    #[tokio::test]
    async fn stream_without_done_marker_still_completes() {
        let raw = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"хвост\"},\"finish_reason\":\"stop\"}]}\n\n";
        let chunks = run_ok(raw).await;
        let (message, _) = final_message(&chunks);
        assert_eq!(message.content, vec![ContentBlock::text("хвост")]);
    }

    /// ollama не шлёт `index` в фрагментах тул-коллов.
    #[tokio::test]
    async fn tool_call_without_index_field_is_tracked_by_id() {
        let raw = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"bash\",\"arguments\":\"{\\\"command\\\":\"}}]},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"function\":{\"arguments\":\"\\\"ls -la\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let chunks = run_ok(raw).await;
        let (message, _) = final_message(&chunks);
        assert_eq!(
            message.content,
            vec![ContentBlock::tool_use(
                "call_1",
                "bash",
                json!({"command": "ls -la"})
            )]
        );
    }

    /// Реальный стрим локального сервера: первый фрагмент несёт `id` и пустые
    /// аргументы БЕЗ имени, имя приходит вторым фрагментом. Строгие парсеры на
    /// этом падают.
    const LATE_TOOL_NAME_STREAM: &str = concat!(
        "data: {\"choices\":[{\"finish_reason\":null,\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":null}}]}\n\n",
        "data: {\"choices\":[{\"finish_reason\":null,\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_123\",\"type\":\"function\",\"function\":{\"arguments\":\"\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"finish_reason\":null,\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_123\",\"type\":\"function\",\"function\":{\"name\":\"bash\",\"arguments\":\"{\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"finish_reason\":null,\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\u0022command\\u0022\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"finish_reason\":null,\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\": \\u0022ls -la\\u0022}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"finish_reason\":\"tool_calls\",\"index\":0,\"delta\":{}}]}\n\n",
        "data: [DONE]\n\n",
    );

    #[tokio::test]
    async fn tool_name_arriving_after_first_fragment_is_accepted() {
        let chunks = run_ok(LATE_TOOL_NAME_STREAM).await;
        let (message, _) = final_message(&chunks);
        assert_eq!(
            message.content,
            vec![ContentBlock::tool_use(
                "call_123",
                "bash",
                json!({"command": "ls -la"})
            )]
        );
    }

    #[tokio::test]
    async fn empty_arguments_become_empty_object() {
        let raw = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"type\":\"function\",\"function\":{\"name\":\"list\",\"arguments\":\"\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let chunks = run_ok(raw).await;
        let (message, _) = final_message(&chunks);
        assert_eq!(
            message.content,
            vec![ContentBlock::tool_use("c1", "list", json!({}))]
        );
    }

    /// Обрезанные аргументы чинятся json_repair, а не роняют ход.
    #[tokio::test]
    async fn truncated_tool_arguments_go_through_json_repair() {
        let raw = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"type\":\"function\",\"function\":{\"name\":\"write\",\"arguments\":\"{'path': 'a.txt', 'content': 'hello\"}}]},\"finish_reason\":\"length\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let chunks = run_ok(raw).await;
        let (message, _) = final_message(&chunks);
        assert_eq!(
            message.content,
            vec![ContentBlock::tool_use(
                "c1",
                "write",
                json!({"path": "a.txt", "content": "hello"})
            )]
        );
    }

    #[test]
    fn unparsable_arguments_are_marked_not_dropped() {
        let value = parse_arguments("совсем не json", "write");
        assert_eq!(value[UNPARSED_MARKER_KEY], json!(true));
        assert_eq!(value[RAW_ARGUMENTS_KEY], json!("совсем не json"));
    }

    #[test]
    fn double_encoded_arguments_are_unwrapped() {
        assert_eq!(
            parse_arguments("\"{\\\"path\\\":\\\"a\\\"}\"", "read"),
            json!({"path": "a"})
        );
        // Обычная строка строкой и остаётся.
        assert_eq!(
            parse_arguments("\"просто строка\"", "read"),
            json!("просто строка")
        );
    }

    #[test]
    fn arguments_key_order_is_preserved() {
        let value = parse_arguments(r#"{"zebra":1,"alpha":2}"#, "write");
        assert_eq!(
            serde_json::to_string(&value).unwrap(),
            r#"{"zebra":1,"alpha":2}"#
        );
    }

    #[tokio::test]
    async fn broken_chunk_json_surfaces_as_parse_error() {
        let out = run_sse("data: {не json}\n\n").await;
        assert!(matches!(out[0], Err(ProviderError::Parse(_))), "{out:?}");
        assert_eq!(out.len(), 1, "после ошибки стрим закрывается");
    }

    /// llama-server умеет отдать ошибку прямо в SSE-кадре.
    #[tokio::test]
    async fn error_frame_mid_stream_is_mapped() {
        let raw = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"нач\"},\"finish_reason\":null}]}\n\n",
            "data: {\"error\":{\"code\":400,\"message\":\"request (14429 tokens) exceeds the available context size (8192 tokens), try increasing it\",\"type\":\"exceed_context_size_error\"}}\n\n",
        );
        let out = run_sse(raw).await;
        assert_eq!(
            out[0].as_ref().unwrap(),
            &StreamChunk::TextDelta("нач".into())
        );
        assert!(matches!(out[1], Err(ProviderError::ContextLengthExceeded)));
    }

    // ───────────────────────── маппинг ошибок ─────────────────────────

    /// Тела 400 у разных серверов; формулировки сверены с их исходниками.
    #[test]
    fn context_overflow_bodies_map_to_dedicated_error() {
        let bodies = [
            // OpenAI: код ошибки + текст.
            r#"{"error":{"message":"This model's maximum context length is 16385 tokens. However, your messages resulted in 16648 tokens. Please reduce the length of the messages.","type":"invalid_request_error","param":"messages","code":"context_length_exceeded"}}"#,
            // vllm 0.11: свой конверт и свой `type`.
            r#"{"error":{"message":"This model's maximum context length is 8192 tokens. However, your request has 9000 input tokens. Please reduce the length of the input messages.","type":"BadRequestError","code":400,"param":null}}"#,
            // vllm, вторая формулировка.
            r#"{"error":{"message":"The decoder prompt (length 9000) is longer than the maximum model length of 8192.","type":"BadRequestError","code":400,"param":null}}"#,
            // llama.cpp server (tools/server/server-context.cpp).
            r#"{"error":{"code":400,"message":"request (14429 tokens) exceeds the available context size (8192 tokens), try increasing it","type":"exceed_context_size_error"}}"#,
            // ollama через OpenAI-совместимый слой.
            r#"{"error":{"message":"the prompt is longer than the context length currently available to the model; shorten the prompt, adjust the context length in settings, or use a model with a longer context length","type":"invalid_request_error","param":null,"code":null}}"#,
            // ollama, нативный конверт.
            r#"{"error":"the input length exceeds the context length"}"#,
            // Другой регистр не должен мешать.
            "Error: Context Length Exceeded",
        ];
        for body in bodies {
            assert!(
                matches!(
                    map_error_body(400, body),
                    ProviderError::ContextLengthExceeded
                ),
                "не распознано: {body}"
            );
        }
    }

    #[test]
    fn other_errors_keep_status_and_body() {
        let bodies = [
            r#"{"error":{"message":"Incorrect API key provided","type":"invalid_request_error","code":"invalid_api_key"}}"#,
            r#"{"error":{"message":"Rate limit reached for gpt-4o-mini in organization org-1 on tokens per min (TPM): Limit 200000","type":"tokens","code":"rate_limit_exceeded"}}"#,
            r#"{"error":{"message":"model 'qwen3' not found","type":"api_error","param":null,"code":null}}"#,
        ];
        for body in bodies {
            match map_error_body(401, body) {
                ProviderError::Http { status, body: got } => {
                    assert_eq!(status, 401);
                    assert_eq!(got, body);
                }
                other => panic!("ожидалась Http для {body}, получена {other:?}"),
            }
        }
    }

    #[test]
    fn huge_error_bodies_are_truncated_on_char_boundary() {
        let body = "я".repeat(4096);
        match map_error_body(500, &body) {
            ProviderError::Http { body, .. } => {
                assert!(body.len() < 4096 + 64);
                assert!(body.contains("обрезано"));
            }
            other => panic!("ожидалась Http, получена {other:?}"),
        }
    }

    #[test]
    fn json_response_is_distinguished_from_event_stream() {
        assert!(is_json_response(Some("application/json")));
        assert!(is_json_response(Some("Application/JSON; charset=utf-8")));
        assert!(!is_json_response(Some("text/event-stream")));
        assert!(!is_json_response(Some("text/event-stream; charset=utf-8")));
        // Нестандартный или отсутствующий заголовок трактуем как стрим.
        assert!(!is_json_response(None));
        assert!(!is_json_response(Some("text/plain")));
    }

    #[test]
    fn retryable_statuses_are_transient_only() {
        for status in [408, 409, 425, 429, 500, 502, 503, 504, 529] {
            assert!(is_retryable_status(status), "{status} должен ретраиться");
        }
        for status in [400, 401, 403, 404, 422] {
            assert!(!is_retryable_status(status), "{status} ретраить нельзя");
        }
    }

    #[test]
    fn backoff_grows_exponentially_and_respects_retry_after() {
        assert_eq!(retry_delay(1, None), Duration::from_millis(250));
        assert_eq!(retry_delay(2, None), Duration::from_millis(500));
        assert_eq!(retry_delay(3, None), Duration::from_millis(1000));
        assert_eq!(retry_delay(1, Some("7")), Duration::from_secs(7));
        assert_eq!(retry_delay(1, Some(" 3 ")), Duration::from_secs(3));
        // Мусор в заголовке не ломает бэкофф.
        assert_eq!(
            retry_delay(1, Some("Wed, 21 Oct 2015 07:28:00 GMT")),
            Duration::from_millis(250)
        );
        // Потолок соблюдается.
        assert_eq!(retry_delay(1, Some("600")), BACKOFF_CAP);
        assert_eq!(retry_delay(20, None), BACKOFF_CAP);
    }
}
