//! API errors: a stable machine code that clients translate, plus English text for logs.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Stable error codes. Clients show their own (translated) text for a code and never compare
/// the English `message`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadRequest,
    Unauthorized,
    Forbidden,
    NotFound,
    Conflict,
    TooManyRequests,
    PayloadTooLarge,
    /// The request came from outside the LAN while remote access is off.
    RemoteAccessDisabled,
    InvalidCredentials,
    AccountLocked,
    PairingCodeInvalid,
    EngineUnavailable,
    /// Waiting for something (magnet metadata, the engine) took too long.
    Timeout,
    /// The server's disk is (nearly) full.
    NoSpace,
    Internal,
}

impl ErrorCode {
    pub fn http_status(self) -> u16 {
        match self {
            ErrorCode::BadRequest => 400,
            ErrorCode::Unauthorized | ErrorCode::InvalidCredentials | ErrorCode::PairingCodeInvalid => 401,
            ErrorCode::Forbidden | ErrorCode::RemoteAccessDisabled => 403,
            ErrorCode::NotFound => 404,
            ErrorCode::Conflict => 409,
            ErrorCode::PayloadTooLarge => 413,
            ErrorCode::TooManyRequests | ErrorCode::AccountLocked => 429,
            ErrorCode::EngineUnavailable => 503,
            ErrorCode::Timeout => 504,
            ErrorCode::NoSpace => 507,
            ErrorCode::Internal => 500,
        }
    }

    /// Translated text for this code (Rust clients).
    pub fn text(self) -> &'static str {
        match self {
            ErrorCode::BadRequest => t!("The request was not valid."),
            ErrorCode::Unauthorized => t!("Please sign in again."),
            ErrorCode::Forbidden => t!("You don't have permission to do that."),
            ErrorCode::NotFound => t!("Not found."),
            ErrorCode::Conflict => t!("That already exists."),
            ErrorCode::TooManyRequests => t!("Too many attempts. Wait a moment and try again."),
            ErrorCode::PayloadTooLarge => t!("That is too large to upload."),
            ErrorCode::RemoteAccessDisabled => t!("Remote access is turned off on this server."),
            ErrorCode::InvalidCredentials => t!("Wrong user name or password."),
            ErrorCode::AccountLocked => t!("Too many failed sign-ins. The account is locked for a while."),
            ErrorCode::PairingCodeInvalid => t!("The pairing code is wrong or has expired."),
            ErrorCode::EngineUnavailable => t!("The torrent engine isn't running."),
            ErrorCode::Timeout => t!("That took too long. Try again later."),
            ErrorCode::NoSpace => t!("The server is running out of disk space."),
            ErrorCode::Internal => t!("Something went wrong on the server."),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: ErrorCode,
    /// English, for logs and developers.
    pub message: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
}

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> ApiError {
        ApiError { code, message: message.into(), params: BTreeMap::new() }
    }

    pub fn with(mut self, key: &str, value: impl ToString) -> ApiError {
        self.params.insert(key.to_string(), value.to_string());
        self
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for ApiError {}
