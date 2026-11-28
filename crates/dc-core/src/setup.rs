//! First-launch setup wizard types that are not part of the account request.

use serde::{Deserialize, Serialize};

/// `POST /api/setup/check`: checks the one-time setup code and reports the
/// connections the server was started with. Nothing is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupCheckRequest {
    pub setup_code: String,
}

/// One connection as the wizard shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionCheck {
    /// Whether the connection works now.
    pub ok: bool,
    /// What is connected, for example the practice records connector's name.
    pub label: String,
    /// A sentence for staff explaining the result.
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupCheckResponse {
    /// The practice management system connector.
    pub practice_records: ConnectionCheck,
    /// The text model provider used to write presentations.
    pub ai_provider: ConnectionCheck,
}
