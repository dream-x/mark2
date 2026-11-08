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
//! Главный инвариант: **новый тул по умолчанию заперт, свободу надо заслужить
//! явной записью в [`READ_ONLY_TOOLS`]**. Без гейта проходят только тулы из
//! этого allowlist (`read`, `grep`); всё остальное в `ask_writes` спрашивает
//! пользователя, а в `read_only` получает отказ. Тул, добавленный в реестр и
//! забытый здесь, не получает свободу молча — он упирается в гейт.
//!
//! Гейтованные тулы, известные политике, разложены по двум спискам — ради
//! точных формулировок отказа и разметки промпта, а не ради самого гейта.
//! [`MUTATING_TOOLS`] — всё, что может изменить диск или систему (`bash`,
//! `write`, `edit`). `bash` считается мутирующим, даже когда модель зовёт его
//! ради `ls`: разобрать шелл-строку надёжно нельзя, а ошибаться безопаснее в
//! сторону вопроса. [`NETWORK_TOOLS`] (`web_fetch`, `web_search`) диск не
//! меняют, но гейтятся так же, как `bash`: запрос уходит наружу, и URL с
//! данными из проекта — готовый канал утечки для промпт-инъекции.
//!
//! Незнакомец — тул ни из одного списка — гейтуется наравне с ними, а отказ в
//! `read_only` прямо называет его неклассифицированным
//! ([`unclassified_denial`]): модель должна понять, что дело не в аргументах и
//! повтор не поможет.

use serde::{Deserialize, Serialize};

/// Тулы, которым политика доверяет без вопросов: только читают внутри cwd и
/// ни диск, ни сеть не трогают. Это allowlist: всё, чего здесь нет, стоит за
/// гейтом, и новый тул становится свободным только явной записью сюда.
pub const READ_ONLY_TOOLS: [&str; 2] = ["read", "grep"];

/// Свободен ли тул с таким именем от гейта: стоит в [`READ_ONLY_TOOLS`].
pub fn is_read_only(tool_name: &str) -> bool {
    READ_ONLY_TOOLS.contains(&tool_name)
}

/// Тулы, способные изменить состояние машины. Гейт держится не на этом
/// списке (незнакомец заперт и так), а точный текст отказа и классификация —
/// на нём: новый мутирующий тул добавляется сюда явно.
pub const MUTATING_TOOLS: [&str; 3] = ["bash", "edit", "write"];

/// Мутирует ли тул с таким именем.
pub fn is_mutating(tool_name: &str) -> bool {
    MUTATING_TOOLS.contains(&tool_name)
}

/// Тулы, уводящие запрос в сеть. Гейтятся как мутирующие; новый сетевой тул
/// добавляется сюда явно — от этого зависят текст отказа и место его
/// определения в промпте ([`crate::prompt`]).
pub const NETWORK_TOOLS: [&str; 2] = ["web_fetch", "web_search"];

/// Ходит ли тул с таким именем в сеть.
pub fn is_network(tool_name: &str) -> bool {
    NETWORK_TOOLS.contains(&tool_name)
}

/// Классифицирован ли тул явно — стоит хотя бы в одном из списков политики.
/// Неклассифицированный тул заперт и в `read_only` получает отказ
/// [`unclassified_denial`].
pub fn is_classified(tool_name: &str) -> bool {
    is_read_only(tool_name) || is_mutating(tool_name) || is_network(tool_name)
}

/// Стоит ли тул за гейтом политики: всё, что не в [`READ_ONLY_TOOLS`].
/// Явные гейтованные списки старше allowlist: окажись имя по ошибке и там, и
/// там — гейт остаётся.
pub fn is_gated(tool_name: &str) -> bool {
    !is_read_only(tool_name) || is_mutating(tool_name) || is_network(tool_name)
}

/// Политика доступа: чем ядро отвечает на запрос тула.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionPolicy {
    /// Ничего не спрашивать — всё разрешено (`--yolo`, CI, песочница).
    Yolo,
    /// Значение по умолчанию: тулы из [`READ_ONLY_TOOLS`] свободны, всё
    /// остальное — мутирующие, сетевые и неклассифицированные — требует
    /// подтверждения пользователя.
    #[default]
    AskWrites,
    /// Только чтение: всё, что не в [`READ_ONLY_TOOLS`], отклоняется, ничего
    /// не спрашивая.
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
        if !is_gated(tool_name) {
            return PermissionCheck::Allowed;
        }
        match self {
            Self::Yolo => PermissionCheck::Allowed,
            Self::AskWrites => PermissionCheck::NeedsApproval,
            Self::ReadOnly => PermissionCheck::Denied {
                reason: if is_classified(tool_name) {
                    read_only_denial(tool_name)
                } else {
                    unclassified_denial(tool_name)
                },
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

/// Текст отказа [`PermissionPolicy::ReadOnly`] для тула вне всех списков.
///
/// Модель должна понять три вещи: тул не классифицирован и потому заперт
/// (дело не в аргументах), повтор не поможет, а свободны только тулы из
/// [`READ_ONLY_TOOLS`] — их список подставляется из константы.
pub fn unclassified_denial(tool_name: &str) -> String {
    let allowed = READ_ONLY_TOOLS
        .iter()
        .map(|tool| format!("`{tool}`"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Permission denied: `{tool_name}` is an unclassified tool, and unclassified tools are \
         locked in this read-only session. Only tools explicitly marked read-only ({allowed}) are \
         available; calling `{tool_name}` again will fail the same way, whatever the arguments. \
         Inspect what you need with them and describe the rest in your answer instead."
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

    /// Имена, которых нет ни в одном списке: будущий тул, галлюцинация модели
    /// и известное имя в другом регистре — сравнение точное, `Read` не `read`.
    const UNCLASSIFIED: [&str; 3] = ["some_future_search_tool", "list_dir", "Read"];

    /// Реестр со всеми тулами, какие умеет собрать ядро: обе стратегии
    /// `edit`, `web_fetch` и `web_search`.
    fn full_registries() -> Vec<crate::tools::ToolRegistry> {
        use crate::tools::{EditStrategy, ToolRegistry};

        EditStrategy::ALL
            .into_iter()
            .map(|strategy| {
                ToolRegistry::with_defaults(strategy)
                    .with_web_search(Some("http://localhost:8888"))
                    .unwrap()
            })
            .collect()
    }

    #[test]
    fn default_policy_asks_before_writes() {
        assert_eq!(PermissionPolicy::default(), PermissionPolicy::AskWrites);
    }

    /// Allowlist — ровно `read` и `grep`: расширять его — осознанное решение,
    /// а не побочный эффект.
    #[test]
    fn read_only_allowlist_is_read_and_grep() {
        assert_eq!(READ_ONLY_TOOLS, ["read", "grep"]);
        for tool in READ_ONLY_TOOLS {
            assert!(is_read_only(tool), "{tool}");
            assert!(is_classified(tool), "{tool}");
            assert!(!is_gated(tool), "{tool}");
        }
    }

    #[test]
    fn mutating_set_is_bash_edit_write() {
        assert_eq!(MUTATING_TOOLS, ["bash", "edit", "write"]);
        for tool in MUTATING_TOOLS {
            assert!(is_mutating(tool), "{tool}");
        }
        for tool in READ_ONLY_TOOLS.iter().chain(UNCLASSIFIED.iter()) {
            assert!(!is_mutating(tool), "{tool}");
        }
    }

    #[test]
    fn network_tools_are_gated_but_not_mutating() {
        assert_eq!(NETWORK_TOOLS, ["web_fetch", "web_search"]);
        for tool in NETWORK_TOOLS {
            assert!(is_network(tool), "{tool}");
            assert!(is_gated(tool), "{tool}");
            assert!(!is_mutating(tool), "{tool}");
        }
        for tool in MUTATING_TOOLS {
            assert!(is_gated(tool) && !is_network(tool), "{tool}");
        }
        for tool in READ_ONLY_TOOLS {
            assert!(!is_gated(tool), "{tool}");
        }
    }

    /// Ни одно имя не стоит в двух списках сразу: тул либо свободен, либо
    /// гейтован по понятной причине.
    #[test]
    fn classification_lists_are_disjoint() {
        let all: Vec<&str> = READ_ONLY_TOOLS
            .iter()
            .chain(MUTATING_TOOLS.iter())
            .chain(NETWORK_TOOLS.iter())
            .copied()
            .collect();
        for (index, tool) in all.iter().enumerate() {
            assert!(!all[index + 1..].contains(tool), "`{tool}` в двух списках");
        }
    }

    /// Инвариант модуля: незнакомец заперт, хотя ни в одном гейтованном
    /// списке его нет.
    #[test]
    fn unclassified_tools_are_gated_by_default() {
        for tool in UNCLASSIFIED {
            assert!(!is_classified(tool), "{tool}");
            assert!(!is_read_only(tool), "{tool}");
            assert!(is_gated(tool), "{tool}");
        }
    }

    /// Сеть — как `bash`: yolo пускает, ask_writes спрашивает, read_only
    /// отказывает текстом для модели.
    #[test]
    fn network_tools_follow_the_bash_rules() {
        for tool in NETWORK_TOOLS {
            assert_eq!(PermissionPolicy::Yolo.check(tool), PermissionCheck::Allowed);
            assert!(
                PermissionPolicy::AskWrites.check(tool).needs_approval(),
                "{tool}"
            );
            let check = PermissionPolicy::ReadOnly.check(tool);
            let reason = check.denial_reason().unwrap_or_else(|| panic!("{tool}"));
            assert!(reason.starts_with("Permission denied:"), "{reason}");
            assert!(reason.contains(tool), "{reason}");
        }
    }

    #[test]
    fn yolo_allows_everything() {
        for tool in MUTATING_TOOLS
            .iter()
            .chain(NETWORK_TOOLS.iter())
            .chain(READ_ONLY_TOOLS.iter())
            .chain(UNCLASSIFIED.iter())
        {
            assert_eq!(
                PermissionPolicy::Yolo.check(tool),
                PermissionCheck::Allowed,
                "{tool}"
            );
        }
    }

    /// `ask_writes` пускает без вопроса только allowlist; мутирующие, сетевые
    /// и незнакомые тулы — через подтверждение.
    #[test]
    fn ask_writes_frees_only_the_read_only_allowlist() {
        for tool in MUTATING_TOOLS
            .iter()
            .chain(NETWORK_TOOLS.iter())
            .chain(UNCLASSIFIED.iter())
        {
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
            // Известный тул — точная формулировка, не «неклассифицированный».
            assert!(!reason.contains("unclassified"), "{reason}");
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

    /// Незнакомец в `read_only` отклоняется без вопроса, и отказ объясняет
    /// модели причину: тул не классифицирован, потому заперт; повтор не
    /// поможет; свободны `read` и `grep`.
    #[test]
    fn read_only_denies_unclassified_tools_as_unclassified() {
        for tool in UNCLASSIFIED {
            let check = PermissionPolicy::ReadOnly.check(tool);
            assert!(!check.needs_approval(), "{tool} не должен спрашивать");
            let reason = check.denial_reason().unwrap_or_else(|| panic!("{tool}"));
            assert_eq!(reason, unclassified_denial(tool));
            assert!(reason.starts_with("Permission denied:"), "{reason}");
            assert!(reason.contains(&format!("`{tool}`")), "{reason}");
            assert!(reason.contains("unclassified tool"), "{reason}");
            assert!(reason.contains("locked"), "{reason}");
            assert!(reason.contains("again"), "{reason}");
            for allowed in READ_ONLY_TOOLS {
                assert!(reason.contains(&format!("`{allowed}`")), "{reason}");
            }
        }
    }

    /// Из полного реестра (с `web_search`, при любой стратегии `edit`) в
    /// read-only сессии остаются ровно `grep` и `read` — те же, что в
    /// allowlist, — а `ask_writes` свободно пускает только их же: поиск
    /// доступен ревьюверу наравне с чтением, остальное заперто.
    #[test]
    fn read_only_keeps_exactly_grep_and_read_from_the_default_registry() {
        for registry in full_registries() {
            for policy in [PermissionPolicy::ReadOnly, PermissionPolicy::AskWrites] {
                let allowed: Vec<&str> = registry
                    .names()
                    .into_iter()
                    .filter(|name| policy.check(name).is_allowed())
                    .collect();
                assert_eq!(allowed, ["grep", "read"], "{}", policy.as_str());
            }
        }
    }

    /// Каждый тул, который умеет собрать ядро, классифицирован явно — стоит
    /// хотя бы в одном списке. Новый тул в реестре без классификации валит
    /// этот тест, а не проходит незамеченным с отказом «unclassified tool».
    #[test]
    fn every_default_registry_tool_is_classified() {
        for registry in full_registries() {
            let names = registry.names();
            // Сетевые тулы в проверке есть: иначе тест молча сузился бы.
            for network in NETWORK_TOOLS {
                assert!(names.contains(&network), "{network} нет в {names:?}");
            }
            for name in names {
                assert!(is_classified(name), "тул `{name}` не классифицирован");
            }
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
