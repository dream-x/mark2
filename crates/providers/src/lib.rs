//! `mark2-providers` — реализации trait `Provider` из [`mark2_core`].
//!
//! Первая цель — [`openai_compat`]: один код на llama-server, ollama,
//! LM Studio, vllm и OpenRouter. Модули объявлены заранее (Task 3 плана),
//! чтобы параллельные волны реализации не пересекались в общих файлах.

pub mod json_repair;
pub mod openai_compat;
pub mod sse;

/// Ядро, против которого собраны провайдеры.
pub use mark2_core;

/// Версия крейта провайдеров.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
