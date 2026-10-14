//! Snapshot of stable error codes (F-08-03). Changing this list is an API change.
use common_core::ErrorCode;

#[test]
fn error_codes_are_stable() {
    let codes: Vec<&str> = ErrorCode::ALL.iter().map(|c| c.as_str()).collect();
    assert_eq!(
        codes,
        vec![
            "invalid_request", "model_not_found", "model_not_downloaded", "model_file_invalid",
            "integrity_mismatch", "insufficient_memory", "context_too_long", "backend_unavailable",
            "backend_failed", "gpu_unavailable", "timeout", "cancelled", "overloaded",
            "download_failed", "not_implemented", "policy_denied", "unauthorized", "io", "internal",
        ]
    );
}
