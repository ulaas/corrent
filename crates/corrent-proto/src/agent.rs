//! AI agents (MCP): changes they propose, which a person approves or rejects.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    pub id: i64,
    pub created_at: i64,
    /// The MCP tool that proposed it (`propose_details`, …).
    pub tool: String,
    /// What it would do, for people (in the server's language).
    pub summary: String,
    /// `pending`, `applied`, `failed` or `rejected`.
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<i64>,
    /// What went wrong (failed), or notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}
