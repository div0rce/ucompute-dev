//! Typed errors with stable codes (F-08-03).
//!
//! Every error that can reach a user has a stable machine code, an HTTP status and a plain-language
//! message. `tests/error_codes_snapshot.rs` pins the code list so a rename is a deliberate change.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Stable error codes. Add at the end; never rename or reuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Request was malformed or violated a bound (bad JSON, prompt too long, tokens <= 0).
    InvalidRequest,
    /// The named model is not in the catalog.
    ModelNotFound,
    /// The model is known but its weights are not downloaded yet.
    ModelNotDownloaded,
    /// The GGUF file is truncated, corrupt or not a GGUF file.
    ModelFileInvalid,
    /// SHA-256 of a file does not match the manifest/catalog.
    IntegrityMismatch,
    /// The model would not fit in available memory (weights + KV cache + buffers).
    InsufficientMemory,
    /// Requested context exceeds the model's trained context or planned n_ctx.
    ContextTooLong,
    /// The pinned backend binaries are missing or failed verification.
    BackendUnavailable,
    /// The backend process failed to start or exited.
    BackendFailed,
    /// The backend was asked for GPU execution and could not provide it; see message for fallback.
    GpuUnavailable,
    /// Operation exceeded its timeout (load, first token, inter-token).
    Timeout,
    /// The request was cancelled by the client (disconnect or Ctrl-C).
    Cancelled,
    /// Server is at capacity; retry later (HTTP 429/503).
    Overloaded,
    /// Download failed (network, HTTP status, size mismatch).
    DownloadFailed,
    /// Feature exists in the registry but is not implemented yet.
    NotImplemented,
    /// Refused by configuration or policy (e.g. non-loopback bind without an API key).
    PolicyDenied,
    /// Authentication failed (bad or missing API key when one is required).
    Unauthorized,
    /// A local filesystem or OS error.
    Io,
    /// Catch-all for unexpected internal failures. Message is still user-safe.
    Internal,
}

impl ErrorCode {
    /// Every code, in declaration order. Used by the snapshot test and by the desktop
    /// plain-language mapping test (F-10-06).
    pub const ALL: &'static [ErrorCode] = &[
        ErrorCode::InvalidRequest,
        ErrorCode::ModelNotFound,
        ErrorCode::ModelNotDownloaded,
        ErrorCode::ModelFileInvalid,
        ErrorCode::IntegrityMismatch,
        ErrorCode::InsufficientMemory,
        ErrorCode::ContextTooLong,
        ErrorCode::BackendUnavailable,
        ErrorCode::BackendFailed,
        ErrorCode::GpuUnavailable,
        ErrorCode::Timeout,
        ErrorCode::Cancelled,
        ErrorCode::Overloaded,
        ErrorCode::DownloadFailed,
        ErrorCode::NotImplemented,
        ErrorCode::PolicyDenied,
        ErrorCode::Unauthorized,
        ErrorCode::Io,
        ErrorCode::Internal,
    ];

    /// Stable snake_case string form, identical to the serde representation.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::InvalidRequest => "invalid_request",
            ErrorCode::ModelNotFound => "model_not_found",
            ErrorCode::ModelNotDownloaded => "model_not_downloaded",
            ErrorCode::ModelFileInvalid => "model_file_invalid",
            ErrorCode::IntegrityMismatch => "integrity_mismatch",
            ErrorCode::InsufficientMemory => "insufficient_memory",
            ErrorCode::ContextTooLong => "context_too_long",
            ErrorCode::BackendUnavailable => "backend_unavailable",
            ErrorCode::BackendFailed => "backend_failed",
            ErrorCode::GpuUnavailable => "gpu_unavailable",
            ErrorCode::Timeout => "timeout",
            ErrorCode::Cancelled => "cancelled",
            ErrorCode::Overloaded => "overloaded",
            ErrorCode::DownloadFailed => "download_failed",
            ErrorCode::NotImplemented => "not_implemented",
            ErrorCode::PolicyDenied => "policy_denied",
            ErrorCode::Unauthorized => "unauthorized",
            ErrorCode::Io => "io",
            ErrorCode::Internal => "internal",
        }
    }

    /// HTTP status the API uses for this code.
    pub fn http_status(self) -> u16 {
        match self {
            ErrorCode::InvalidRequest | ErrorCode::ContextTooLong => 400,
            ErrorCode::Unauthorized => 401,
            ErrorCode::PolicyDenied => 403,
            ErrorCode::ModelNotFound | ErrorCode::ModelNotDownloaded => 404,
            ErrorCode::Cancelled => 499,
            ErrorCode::InsufficientMemory | ErrorCode::ModelFileInvalid | ErrorCode::IntegrityMismatch => 422,
            ErrorCode::Overloaded => 429,
            ErrorCode::NotImplemented => 501,
            ErrorCode::BackendUnavailable | ErrorCode::GpuUnavailable => 503,
            ErrorCode::Timeout => 504,
            ErrorCode::BackendFailed | ErrorCode::DownloadFailed | ErrorCode::Io | ErrorCode::Internal => 500,
        }
    }

    /// OpenAI-style `error.type` string.
    pub fn openai_type(self) -> &'static str {
        match self.http_status() {
            400 | 422 => "invalid_request_error",
            401 => "authentication_error",
            403 => "permission_error",
            404 => "not_found_error",
            429 => "rate_limit_error",
            _ => "server_error",
        }
    }

    /// Plain-language explanation and suggested next step for people, not developers (F-10-06).
    pub fn user_message(self) -> (&'static str, &'static str) {
        match self {
            ErrorCode::InvalidRequest => ("The request was not understood.", "Check the prompt and settings, then try again."),
            ErrorCode::ModelNotFound => ("That model is not in Common's catalog.", "Pick a model from the library or import a GGUF file."),
            ErrorCode::ModelNotDownloaded => ("This model has not been downloaded yet.", "Download it from the model library, then run it."),
            ErrorCode::ModelFileInvalid => ("This model file is damaged or is not a GGUF file.", "Delete it and download the model again."),
            ErrorCode::IntegrityMismatch => ("The downloaded file does not match its published checksum.", "Common deleted the file. Download it again; if this repeats, the source may be compromised."),
            ErrorCode::InsufficientMemory => ("This model needs more memory than your computer has available.", "Choose a smaller model or quantization, lower the context length, or close other apps."),
            ErrorCode::ContextTooLong => ("The prompt is longer than this model can read at once.", "Shorten the prompt or choose a model with a larger context window."),
            ErrorCode::BackendUnavailable => ("The inference engine is not installed or failed verification.", "Run Common's setup to download the pinned engine."),
            ErrorCode::BackendFailed => ("The inference engine stopped unexpectedly.", "Try again. If it keeps failing, open Diagnostics and share the log."),
            ErrorCode::GpuUnavailable => ("Your GPU could not be used for this model.", "Common can run it on the CPU instead, which will be slower."),
            ErrorCode::Timeout => ("The model took too long to respond.", "Try a shorter prompt or a smaller model."),
            ErrorCode::Cancelled => ("The request was stopped.", "Nothing further is needed."),
            ErrorCode::Overloaded => ("Common is busy with other requests.", "Wait a moment and try again."),
            ErrorCode::DownloadFailed => ("The download did not complete.", "Check your internet connection and try again; downloads resume where they stopped."),
            ErrorCode::NotImplemented => ("This feature is not available yet.", "Check the roadmap in the documentation."),
            ErrorCode::PolicyDenied => ("Your settings do not allow this action.", "Review Privacy and Network settings."),
            ErrorCode::Unauthorized => ("This computer is not authorized to use the API.", "Provide the API key configured on the server."),
            ErrorCode::Io => ("Common could not read or write a file.", "Check disk space and permissions."),
            ErrorCode::Internal => ("Something went wrong inside Common.", "Try again. If it keeps failing, open Diagnostics and share the log."),
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The error type every Common crate returns to callers.
#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct CommonError {
    pub code: ErrorCode,
    /// Developer-facing detail. Must never contain prompt text, API keys or private keys.
    pub message: String,
    /// Optional short hint (e.g. the command that fixes it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl CommonError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), hint: None }
    }
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
    pub fn invalid(msg: impl Into<String>) -> Self { Self::new(ErrorCode::InvalidRequest, msg) }
    pub fn internal(msg: impl Into<String>) -> Self { Self::new(ErrorCode::Internal, msg) }
    pub fn io(msg: impl Into<String>) -> Self { Self::new(ErrorCode::Io, msg) }
    pub fn http_status(&self) -> u16 { self.code.http_status() }

    /// OpenAI-compatible JSON body: `{"error":{"message","type","code","param":null}}`.
    pub fn to_openai_json(&self) -> serde_json::Value {
        serde_json::json!({
            "error": {
                "message": self.message,
                "type": self.code.openai_type(),
                "code": self.code.as_str(),
                "param": serde_json::Value::Null,
                "hint": self.hint,
            }
        })
    }
}

impl From<std::io::Error> for CommonError {
    fn from(e: std::io::Error) -> Self {
        CommonError::new(ErrorCode::Io, e.to_string())
    }
}

impl From<serde_json::Error> for CommonError {
    fn from(e: serde_json::Error) -> Self {
        CommonError::new(ErrorCode::InvalidRequest, format!("invalid JSON: {e}"))
    }
}

pub type Result<T> = std::result::Result<T, CommonError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_form_matches_as_str() {
        for code in ErrorCode::ALL {
            let s = serde_json::to_string(code).unwrap();
            assert_eq!(s, format!("\"{}\"", code.as_str()));
        }
    }

    #[test]
    fn every_code_has_status_type_and_user_message() {
        for code in ErrorCode::ALL {
            assert!((400..600).contains(&code.http_status()), "{code}");
            assert!(!code.openai_type().is_empty());
            let (msg, next) = code.user_message();
            assert!(!msg.is_empty() && !next.is_empty(), "{code}");
        }
    }

    #[test]
    fn openai_json_shape() {
        let e = CommonError::new(ErrorCode::ModelNotFound, "no such model").with_hint("common models list");
        let v = e.to_openai_json();
        assert_eq!(v["error"]["code"], "model_not_found");
        assert_eq!(v["error"]["type"], "not_found_error");
        assert!(v["error"]["param"].is_null());
    }
}
