//! `mark2-core` — UI-agnostic ядро agent harness.
//!
//! Здесь живут нейтральные типы сообщений, SQ/EQ-протокол (`Op` внутрь /
//! `Event` наружу), trait `Provider`, тулы, agent loop и запись сессий.
//! Пока — заглушка каркаса (Task 1 плана).

/// Версия ядра.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
