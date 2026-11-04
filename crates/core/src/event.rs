//! SQ/EQ-протокол ядра: [`Op`] внутрь, [`Event`] наружу.
//!
//! Это единственная абстракция, через которую внешний мир говорит с агентом:
//! CLI — лишь первый потребитель, TUI/ACP/сервер придут к тому же каналу.
//! Оба enum'а — плоские serde-tagged (`{"type": "...", ...}`), чтобы их можно
//! было гонять по JSON-линии без дополнительной обёртки.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::message::{Message, Usage};

/// Событие, которое ядро стримит наружу.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// Начался ход агента.
    TurnStarted {
        /// Идентификатор хода — им же помечаются маркеры в JSONL-сессии.
        turn_id: String,
    },
    /// Кусок текста ответа модели.
    TextDelta {
        /// Текст дельты.
        text: String,
    },
    /// Кусок рассуждений модели.
    ThinkingDelta {
        /// Текст дельты.
        text: String,
    },
    /// Аргументы тул-колла приведены к типам схемы тула (`"60"` → `60`, см.
    /// [`crate::tools::ToolRegistry::coerce_input`]) — до истории, сессии и
    /// исполнения; без приведения вызов отбила бы валидация. Приходит до
    /// [`Event::MessageComplete`] с этим вызовом, по событию на вызов: по ним
    /// потребитель считает, насколько приведение востребовано.
    ArgumentsCoerced {
        /// Идентификатор тул-колла.
        id: String,
        /// Имя тула.
        name: String,
        /// Приведённые свойства верхнего уровня, в порядке аргументов.
        fields: Vec<String>,
    },
    /// Тул начал выполняться (аргументы уже собраны и провалидированы).
    ToolCallStarted {
        /// Идентификатор тул-колла.
        id: String,
        /// Имя тула.
        name: String,
        /// Аргументы тула.
        input: Value,
    },
    /// Промежуточный вывод тула (например поток stdout у `bash`).
    ToolCallOutput {
        /// Идентификатор тул-колла.
        id: String,
        /// Кусок вывода.
        chunk: String,
    },
    /// Тул завершился.
    ToolCallFinished {
        /// Идентификатор тул-колла.
        id: String,
        /// Итоговый результат (или текст ошибки).
        content: String,
        /// Завершился ли тул ошибкой.
        is_error: bool,
    },
    /// Предохранитель agent loop заметил непродуктивный паттерн и подсказал
    /// модели сменить тактику — или сенсор вернул ей сигнал об ошибке
    /// (post-edit проверка не прошла). Текст для модели уже лежит в tool
    /// result вызова `id` (парный [`Event::ToolCallFinished`] пришёл раньше);
    /// здесь — сигнал для человека и логов. Сам по себе ход не останавливает:
    /// жёсткий предел `loop_detection` завершает ход отдельным [`Event::Error`].
    Nudge {
        /// Тул-колл, к результату которого относится подсказка.
        id: String,
        /// Какой предохранитель сработал.
        guard: Guard,
        /// Короткое описание для человека.
        message: String,
    },
    /// Требуется решение пользователя по политике доступа; ход ждёт
    /// [`Op::Approve`] с тем же `id`.
    ApprovalRequired {
        /// Идентификатор тул-колла, ожидающего решения.
        id: String,
        /// Имя тула.
        name: String,
        /// Аргументы тула.
        input: Value,
    },
    /// Сообщение собрано целиком и добавлено в историю.
    MessageComplete {
        /// Готовое сообщение в нейтральном формате.
        message: Message,
    },
    /// Расход токенов за запрос к провайдеру.
    Usage {
        /// Счётчики.
        usage: Usage,
    },
    /// История сжата компакцией.
    Compacted {
        /// Оценка занятости контекста до компакции.
        tokens_before: u64,
        /// Оценка занятости контекста после компакции.
        tokens_after: u64,
    },
    /// Ход завершён.
    TurnFinished {
        /// Причина завершения.
        reason: StopReason,
    },
    /// Ошибка, не привязанная к конкретному тулу.
    Error {
        /// Текст ошибки.
        message: String,
    },
    /// Предупреждение для человека: ход не сломан, но что-то не так, как
    /// ждали (например, память агента не влезла в кап и в промпт ушла не
    /// целиком). Модели не адресовано и на ход не влияет.
    Warning {
        /// Текст предупреждения.
        message: String,
    },
}

/// Причина завершения хода.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Модель ответила без тул-коллов — ход закончен нормально.
    Completed,
    /// Ход прерван [`Op::Interrupt`] (или отменой снаружи).
    Cancelled,
    /// Сработал предохранитель `max_turns`.
    MaxTurns,
    /// Ход прерван ошибкой; подробности — в [`Event::Error`].
    Error,
}

/// Предохранитель agent loop, пославший [`Event::Nudge`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Guard {
    /// Детектор зацикливания: N-й подряд идентичный тул-колл не исполнен.
    LoopDetection,
    /// Сенсор write-churn: N-я подряд запись в один файл без единого `bash`
    /// между записями. Запись исполнена, к результату приложена подсказка.
    WriteChurn,
    /// Post-edit сенсор ([`crate::post_edit`]): команда проверки после
    /// успешной записи не прошла (ненулевой exit, сигнал, таймаут). Запись
    /// исполнена, к результату приложен вывод проверки; `message` — `exit N`.
    PostEdit,
}

impl Guard {
    /// Wire-имя — то же, что ключ настройки в `[agent]` (у post-edit сенсора
    /// — общий префикс его ключей `post_edit_*`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LoopDetection => "loop_detection",
            Self::WriteChurn => "write_churn",
            Self::PostEdit => "post_edit",
        }
    }
}

/// Команда, которую внешний мир отправляет ядру.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Op {
    /// Новый ввод пользователя — запускает ход.
    UserInput {
        /// Текст ввода.
        text: String,
    },
    /// Прервать текущий ход.
    Interrupt,
    /// Ответ на [`Event::ApprovalRequired`].
    Approve {
        /// Идентификатор тул-колла из события.
        id: String,
        /// Решение пользователя.
        decision: ApprovalDecision,
    },
    /// Сжать историю вручную, не дожидаясь порога.
    Compact,
}

impl Op {
    /// Ввод пользователя из текста.
    pub fn user_input(text: impl Into<String>) -> Self {
        Self::UserInput { text: text.into() }
    }

    /// Разрешить тул-колл.
    pub fn allow(id: impl Into<String>) -> Self {
        Self::Approve {
            id: id.into(),
            decision: ApprovalDecision::Allow,
        }
    }

    /// Запретить тул-колл.
    pub fn deny(id: impl Into<String>) -> Self {
        Self::Approve {
            id: id.into(),
            decision: ApprovalDecision::Deny,
        }
    }
}

/// Решение пользователя по запросу доступа.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    /// Выполнить тул.
    Allow,
    /// Не выполнять; модели вернётся tool result с отказом.
    Deny,
}

impl ApprovalDecision {
    /// Разрешено ли выполнение.
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ContentBlock;
    use serde_json::json;

    fn all_events() -> Vec<Event> {
        vec![
            Event::TurnStarted {
                turn_id: "turn-1".into(),
            },
            Event::TextDelta {
                text: "привет".into(),
            },
            Event::ThinkingDelta {
                text: "думаю".into(),
            },
            Event::ArgumentsCoerced {
                id: "call_1".into(),
                name: "bash".into(),
                fields: vec!["timeout_seconds".into()],
            },
            Event::ToolCallStarted {
                id: "call_1".into(),
                name: "bash".into(),
                input: json!({"command": "ls", "timeout_ms": 5000}),
            },
            Event::ToolCallOutput {
                id: "call_1".into(),
                chunk: "src\n".into(),
            },
            Event::ToolCallFinished {
                id: "call_1".into(),
                content: "src\n".into(),
                is_error: false,
            },
            Event::Nudge {
                id: "call_1".into(),
                guard: Guard::WriteChurn,
                message: "3-я запись подряд в a.py без запуска".into(),
            },
            Event::ApprovalRequired {
                id: "call_2".into(),
                name: "write".into(),
                input: json!({"path": "/tmp/a.txt", "content": "hi"}),
            },
            Event::MessageComplete {
                message: Message::new(
                    crate::message::Role::Assistant,
                    vec![
                        ContentBlock::text("готово"),
                        ContentBlock::tool_use("call_3", "read", json!({"path": "a"})),
                    ],
                ),
            },
            Event::Usage {
                usage: Usage::new(100, 20),
            },
            Event::Compacted {
                tokens_before: 12_000,
                tokens_after: 3_000,
            },
            Event::TurnFinished {
                reason: StopReason::Completed,
            },
            Event::Error {
                message: "boom".into(),
            },
            Event::Warning {
                message: "память обрезана".into(),
            },
        ]
    }

    fn all_ops() -> Vec<Op> {
        vec![
            Op::user_input("сделай X"),
            Op::Interrupt,
            Op::allow("call_1"),
            Op::deny("call_2"),
            Op::Compact,
        ]
    }

    #[test]
    fn every_event_variant_round_trips() {
        for event in all_events() {
            let json = serde_json::to_string(&event).unwrap();
            let back: Event = serde_json::from_str(&json).unwrap();
            assert_eq!(event, back, "round-trip failed for {json}");
        }
    }

    #[test]
    fn every_op_variant_round_trips() {
        for op in all_ops() {
            let json = serde_json::to_string(&op).unwrap();
            let back: Op = serde_json::from_str(&json).unwrap();
            assert_eq!(op, back, "round-trip failed for {json}");
        }
    }

    #[test]
    fn event_tags_are_snake_case_and_complete() {
        let tags: Vec<String> = all_events()
            .iter()
            .map(|e| {
                serde_json::to_value(e).unwrap()["type"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(
            tags,
            [
                "turn_started",
                "text_delta",
                "thinking_delta",
                "arguments_coerced",
                "tool_call_started",
                "tool_call_output",
                "tool_call_finished",
                "nudge",
                "approval_required",
                "message_complete",
                "usage",
                "compacted",
                "turn_finished",
                "error",
                "warning",
            ]
        );
    }

    #[test]
    fn op_tags_are_snake_case_and_unit_variants_carry_only_tag() {
        assert_eq!(
            serde_json::to_value(Op::Interrupt).unwrap(),
            json!({"type": "interrupt"})
        );
        assert_eq!(
            serde_json::to_value(Op::Compact).unwrap(),
            json!({"type": "compact"})
        );
        assert_eq!(
            serde_json::to_value(Op::user_input("hi")).unwrap(),
            json!({"type": "user_input", "text": "hi"})
        );
        assert_eq!(
            serde_json::to_value(Op::deny("c1")).unwrap(),
            json!({"type": "approve", "id": "c1", "decision": "deny"})
        );
    }

    #[test]
    fn stop_reason_wire_names_are_stable() {
        for (reason, expected) in [
            (StopReason::Completed, "completed"),
            (StopReason::Cancelled, "cancelled"),
            (StopReason::MaxTurns, "max_turns"),
            (StopReason::Error, "error"),
        ] {
            assert_eq!(serde_json::to_value(reason).unwrap(), json!(expected));
            let back: StopReason = serde_json::from_value(json!(expected)).unwrap();
            assert_eq!(reason, back);
        }
    }

    /// Имя предохранителя — и в JSON, и в маркере `[agent] <guard>` в stderr.
    #[test]
    fn guard_wire_names_are_stable() {
        for (guard, expected) in [
            (Guard::LoopDetection, "loop_detection"),
            (Guard::WriteChurn, "write_churn"),
            (Guard::PostEdit, "post_edit"),
        ] {
            assert_eq!(serde_json::to_value(guard).unwrap(), json!(expected));
            assert_eq!(guard.as_str(), expected);
            let back: Guard = serde_json::from_value(json!(expected)).unwrap();
            assert_eq!(guard, back);
        }
        assert_eq!(
            serde_json::to_value(Event::Nudge {
                id: "c1".into(),
                guard: Guard::LoopDetection,
                message: "m".into(),
            })
            .unwrap(),
            json!({"type": "nudge", "id": "c1", "guard": "loop_detection", "message": "m"})
        );
    }

    #[test]
    fn approval_decision_wire_names_and_helper() {
        assert_eq!(
            serde_json::to_value(ApprovalDecision::Allow).unwrap(),
            json!("allow")
        );
        assert_eq!(
            serde_json::to_value(ApprovalDecision::Deny).unwrap(),
            json!("deny")
        );
        assert!(ApprovalDecision::Allow.is_allowed());
        assert!(!ApprovalDecision::Deny.is_allowed());
    }

    #[test]
    fn nested_message_event_serializes_exactly() {
        let event = Event::MessageComplete {
            message: Message::user("привет").with_agent_visible(false),
        };
        assert_eq!(
            serde_json::to_string(&event).unwrap(),
            r#"{"type":"message_complete","message":{"role":"user","content":[{"type":"text","text":"привет"}],"metadata":{"agent_visible":false}}}"#
        );
    }

    /// Одинаковый вход → байт-в-байт одинаковый JSON, включая порядок ключей
    /// в произвольных `input` тулов.
    #[test]
    fn serialization_is_byte_stable() {
        let first: Vec<String> = all_events()
            .iter()
            .map(|e| serde_json::to_string(e).unwrap())
            .collect();
        for _ in 0..16 {
            let again: Vec<String> = all_events()
                .iter()
                .map(|e| serde_json::to_string(e).unwrap())
                .collect();
            assert_eq!(again, first);
        }

        let ops_first: Vec<String> = all_ops()
            .iter()
            .map(|o| serde_json::to_string(o).unwrap())
            .collect();
        for _ in 0..16 {
            let again: Vec<String> = all_ops()
                .iter()
                .map(|o| serde_json::to_string(o).unwrap())
                .collect();
            assert_eq!(again, ops_first);
        }
    }

    #[test]
    fn tool_input_key_order_survives_round_trip() {
        let raw = r#"{"type":"tool_call_started","id":"c1","name":"write","input":{"zebra":1,"alpha":2}}"#;
        let event: Event = serde_json::from_str(raw).unwrap();
        assert_eq!(serde_json::to_string(&event).unwrap(), raw);
    }

    #[test]
    fn events_are_line_delimited_json_friendly() {
        let line: String = all_events()
            .iter()
            .map(|e| serde_json::to_string(e).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        let parsed: Vec<Event> = line
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(parsed, all_events());
    }
}
