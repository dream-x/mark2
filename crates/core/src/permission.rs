//! Политика доступа к тулам: [`PermissionPolicy`] и её решение
//! [`PermissionCheck`].
//!
//! Гейт стоит между «модель попросила тул» и «тул выполняется», и у него ровно
//! три исхода: выполнять, спросить пользователя (событие
//! [`crate::Event::ApprovalRequired`], ответ [`crate::Op::Approve`]) или
//! отказать. Отказ — не ошибка ядра: модель
//! получает его обычным tool result с `is_error = true` и текстом, объясняющим,
//! что делать дальше. Тот же принцип, что у валидации аргументов в
//! [`crate::tools`]: ошибки — это данные для модели.
//!
//! Разделение тулов примитивное и намеренно консервативное: мутирует всё, что
//! может изменить диск или систему (`bash`, `write`, `edit`), а `read`, `grep`
//! (и любой незнакомый реестру тул чтения) свободны. `bash` считается мутирующим, даже
//! когда модель зовёт его ради `ls`: разобрать шелл-строку надёжно нельзя, а
//! ошибаться безопаснее в сторону вопроса.

use serde::{Deserialize, Serialize};

/// Тулы, способные изменить состояние машины. Список фиксирован: новый
/// мутирующий тул обязан быть добавлен сюда явно.
pub const MUTATING_TOOLS: [&str; 3] = ["bash", "edit", "write"];

/// Мутирует ли тул с таким именем.
pub fn is_mutating(tool_name: &str) -> bool {
    MUTATING_TOOLS.contains(&tool_name)
}

/// Политика доступа: чем ядро отвечает на запрос тула.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionPolicy {
    /// Ничего не спрашивать — всё разрешено (`--yolo`, CI, песочница).
    Yolo,
    /// Значение по умолчанию: чтение свободно, мутирующие тулы требуют
    /// подтверждения пользователя.
    #[default]
    AskWrites,
    /// Только чтение: мутирующие тулы отклоняются, ничего не спрашивая.
    ReadOnly,
}

impl PermissionPolicy {
    /// Имя политики в конфиге и логах.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Yolo => "yolo",
            Self::AskWrites => "ask_writes",
            Self::ReadOnly => "read_only",
        }
    }

    /// Решение по конкретному тулу.
    pub fn check(&self, tool_name: &str) -> PermissionCheck {
        if !is_mutating(tool_name) {
            return PermissionCheck::Allowed;
        }
        match self {
            Self::Yolo => PermissionCheck::Allowed,
            Self::AskWrites => PermissionCheck::NeedsApproval,
            Self::ReadOnly => PermissionCheck::Denied {
                reason: read_only_denial(tool_name),
            },
        }
    }
}

/// Исход проверки политики.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionCheck {
    /// Выполнять немедленно.
    Allowed,
    /// Спросить пользователя и ждать [`crate::Op::Approve`].
    NeedsApproval,
    /// Отказать; `reason` уходит модели как tool result с `is_error = true`.
    Denied {
        /// Объяснение для модели: что запрещено и что делать вместо этого.
        reason: String,
    },
}

impl PermissionCheck {
    /// Можно ли выполнять без вопросов.
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }

    /// Нужен ли аппрув пользователя.
    pub fn needs_approval(&self) -> bool {
        matches!(self, Self::NeedsApproval)
    }

    /// Текст отказа, если политика запретила вызов.
    pub fn denial_reason(&self) -> Option<&str> {
        match self {
            Self::Denied { reason } => Some(reason),
            _ => None,
        }
    }
}

/// Текст отказа для [`PermissionPolicy::ReadOnly`].
///
/// Пишется для модели, а не для человека: сказано, что именно запрещено, что
/// повторять бессмысленно и чем заняться вместо этого.
pub fn read_only_denial(tool_name: &str) -> String {
    format!(
        "Permission denied: `{tool_name}` is blocked because this session is read-only. \
         Only read-only tools such as `read` and `grep` are available; calling `{tool_name}` \
         again will fail the same way. Inspect what you need and describe the change in your answer instead."
    )
}

/// Текст отказа, когда пользователь ответил `deny` на [`crate::Op::Approve`].
pub fn user_denial(tool_name: &str) -> String {
    format!(
        "Denied by user: the user rejected this `{tool_name}` call. Do not retry it. \
         Continue without it or ask the user what to do instead."
    )
}

/// Текст результата для тулов, до которых ход не дошёл из-за прерывания.
///
/// Нужен, чтобы у каждого `tool_use` в истории был парный `tool_result`: без
/// пары строгие OpenAI-совместимые сервера отвергают следующий запрос.
pub fn interrupted_result() -> String {
    "Interrupted by the user before this tool ran.".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const READ_ONLY_TOOLS: [&str; 3] = ["read", "grep", "some_future_search_tool"];

    #[test]
    fn default_policy_asks_before_writes() {
        assert_eq!(PermissionPolicy::default(), PermissionPolicy::AskWrites);
    }

    #[test]
    fn mutating_set_is_bash_edit_write() {
        assert_eq!(MUTATING_TOOLS, ["bash", "edit", "write"]);
        for tool in MUTATING_TOOLS {
            assert!(is_mutating(tool), "{tool}");
        }
        for tool in READ_ONLY_TOOLS {
            assert!(!is_mutating(tool), "{tool}");
        }
    }

    #[test]
    fn yolo_allows_everything() {
        for tool in MUTATING_TOOLS.iter().chain(READ_ONLY_TOOLS.iter()) {
            assert_eq!(
                PermissionPolicy::Yolo.check(tool),
                PermissionCheck::Allowed,
                "{tool}"
            );
        }
    }

    #[test]
    fn ask_writes_gates_only_mutating_tools() {
        for tool in MUTATING_TOOLS {
            assert!(
                PermissionPolicy::AskWrites.check(tool).needs_approval(),
                "{tool}"
            );
        }
        for tool in READ_ONLY_TOOLS {
            assert!(
                PermissionPolicy::AskWrites.check(tool).is_allowed(),
                "{tool}"
            );
        }
    }

    #[test]
    fn read_only_denies_mutating_tools_with_a_message_for_the_model() {
        for tool in MUTATING_TOOLS {
            let check = PermissionPolicy::ReadOnly.check(tool);
            assert!(!check.needs_approval(), "{tool} не должен спрашивать");
            let reason = check.denial_reason().unwrap_or_else(|| panic!("{tool}"));
            assert!(reason.starts_with("Permission denied:"), "{reason}");
            assert!(reason.contains(tool), "{reason}");
            assert!(reason.contains("read-only"), "{reason}");
            // Модель должна понять, что повтор бессмыслен.
            assert!(reason.contains("again"), "{reason}");
            // …и чем искать вместо `bash` с grep.
            assert!(reason.contains("`grep`"), "{reason}");
        }
        for tool in READ_ONLY_TOOLS {
            assert!(
                PermissionPolicy::ReadOnly.check(tool).is_allowed(),
                "{tool}"
            );
        }
    }

    /// Из реестра по умолчанию в read-only сессии остаются ровно `grep` и
    /// `read`: новый тул обязан быть классифицирован осознанно, а поиск —
    /// доступен ревьюверу наравне с чтением.
    #[test]
    fn read_only_keeps_exactly_grep_and_read_from_the_default_registry() {
        use crate::tools::{EditStrategy, ToolRegistry};

        let registry = ToolRegistry::with_defaults(EditStrategy::SearchReplace);
        let allowed: Vec<&str> = registry
            .names()
            .into_iter()
            .filter(|name| PermissionPolicy::ReadOnly.check(name).is_allowed())
            .collect();
        assert_eq!(allowed, ["grep", "read"]);
        for name in registry.names() {
            assert!(
                is_mutating(name) || PermissionPolicy::AskWrites.check(name).is_allowed(),
                "{name}"
            );
        }
    }

    #[test]
    fn denial_texts_name_the_tool_and_next_step() {
        let denied = user_denial("write");
        assert!(denied.starts_with("Denied by user:"), "{denied}");
        assert!(denied.contains("`write`"), "{denied}");
        assert!(denied.contains("Do not retry"), "{denied}");
        assert!(interrupted_result().contains("Interrupted"));
    }

    #[test]
    fn policy_wire_names_are_stable() {
        for (policy, expected) in [
            (PermissionPolicy::Yolo, "yolo"),
            (PermissionPolicy::AskWrites, "ask_writes"),
            (PermissionPolicy::ReadOnly, "read_only"),
        ] {
            assert_eq!(
                serde_json::to_value(policy).unwrap(),
                serde_json::json!(expected)
            );
            assert_eq!(policy.as_str(), expected);
            let back: PermissionPolicy =
                serde_json::from_value(serde_json::json!(expected)).unwrap();
            assert_eq!(policy, back);
        }
    }
}
