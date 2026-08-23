//! TODO(task 3): trait `Provider`, `ProviderRequest`, `MessageStream`, `StreamChunk`.
//!
//! `ToolDef` определён заранее как общий контракт между провайдером (task 3)
//! и реестром тулов (task 4) — не менять сигнатуру без синхронизации обоих.

use serde::{Deserialize, Serialize};

/// Определение тула, передаваемое провайдеру: имя, описание и JSON Schema входа.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}
