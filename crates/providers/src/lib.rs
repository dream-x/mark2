//! `mark2-providers` — реализации trait `Provider` из [`mark2_core`].
//!
//! Первая цель — `openai_compat`: один код на llama-server, ollama,
//! LM Studio, vllm и OpenRouter. Пока — заглушка каркаса (Task 1 плана).

/// Ядро, против которого собраны провайдеры.
pub use mark2_core;

/// Версия крейта провайдеров.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
