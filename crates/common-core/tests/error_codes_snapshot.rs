//! Snapshot of stable error codes (F-08-03). Changing this list is an API change.
//! Pins (code, HTTP status, OpenAI `error.type`) so a status remap also fails here.
use common_core::ErrorCode;

#[test]
fn error_codes_are_stable() {
    let actual: Vec<(&str, u16, &str)> = ErrorCode::ALL
        .iter()
        .map(|c| (c.as_str(), c.http_status(), c.openai_type()))
        .collect();
    assert_eq!(
        actual,
        vec![
            ("invalid_request", 400, "invalid_request_error"),
            ("model_not_found", 404, "not_found_error"),
            ("model_not_downloaded", 404, "not_found_error"),
            ("model_file_invalid", 422, "invalid_request_error"),
            ("integrity_mismatch", 422, "invalid_request_error"),
            ("insufficient_memory", 422, "invalid_request_error"),
            ("context_too_long", 400, "invalid_request_error"),
            ("backend_unavailable", 503, "server_error"),
            ("backend_failed", 500, "server_error"),
            ("gpu_unavailable", 503, "server_error"),
            ("timeout", 504, "server_error"),
            ("cancelled", 499, "server_error"),
            ("overloaded", 429, "rate_limit_error"),
            ("download_failed", 500, "server_error"),
            ("not_implemented", 501, "server_error"),
            ("policy_denied", 403, "permission_error"),
            ("unauthorized", 401, "authentication_error"),
            ("io", 500, "server_error"),
            ("internal", 500, "server_error"),
            // M3 (ADR-0003), appended
            ("peer_unauthorized", 403, "permission_error"),
            ("peer_revoked", 403, "permission_error"),
            ("pairing_locked", 429, "rate_limit_error"),
            ("pairing_failed", 401, "authentication_error"),
            ("peer_lost", 502, "server_error"),
            ("protocol_incompatible", 426, "invalid_request_error"),
            ("frame_too_large", 413, "invalid_request_error"),
            // UI 2.0 E5, appended
            ("peer_held", 403, "permission_error"),
        ]
    );
}
