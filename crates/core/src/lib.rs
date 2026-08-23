//! `mark2-core` — UI-agnostic ядро agent harness.
//!
//! Здесь живут нейтральные типы сообщений ([`message`]), SQ/EQ-протокол
//! ([`event`]: `Op` внутрь / `Event` наружу), trait `Provider`, тулы, agent
//! loop, запись сессий и компакция контекста.
//!
//! Модули объявлены заранее — по одному на таск плана
//! `docs/plans/20260824-rust-agent-harness.md`, чтобы параллельные волны
//! реализации не пересекались в общих файлах.

pub mod agent;
pub mod compact;
pub mod config;
pub mod event;
pub mod message;
pub mod permission;
pub mod prompt;
pub mod provider;
pub mod session;
pub mod tools;

pub use agent::{Agent, AgentConfig, AgentHandle};
pub use event::{ApprovalDecision, Event, Op, StopReason};
pub use message::{ContentBlock, Message, MessageMetadata, Role, Usage};
pub use permission::{PermissionCheck, PermissionPolicy};

/// Версия ядра.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
