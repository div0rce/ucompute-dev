//! Audit trail and redacted diagnostics (F-06-10).
//!
//! `audit.jsonl` (0600) under `COMMON_HOME/logs/` records every off-device action: pairing,
//! peer connect/disconnect/reject, revocation, remote requests, sessions. Never prompt text,
//! token strings or key material: `record` refuses fields whose keys could carry them and any
//! string longer than 2 KiB. Every string of the diagnostics bundle passes through `redact`, the
//! single place that knows the secret patterns, and a test scans the result.

use common_core::{CommonError, Paths, Result};
use serde::{Deserialize, Serialize};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditEvent {
    pub ts_utc: String,
    /// e.g. `pairing_succeeded`, `peer_rejected`, `remote_request`, `peer_revoked`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_id: Option<String>,
    /// Structured, redaction-safe fields (sizes, ids, reasons, durations). Never prompt text.
    #[serde(default)]
    pub fields: serde_json::Value,
}

/// Field keys that can only carry prompt text or key material; `record` refuses them.
pub const FORBIDDEN_FIELD_KEYS: &[&str] = &["prompt", "content", "messages", "api_key", "private_key", "pkcs8"];
/// Longest string value an audit field may hold; prompts and keys are longer, ids and reasons are not.
pub const MAX_FIELD_STRING: usize = 2048;

pub struct Audit {
    pub path: PathBuf,
    // ponytail: one process-wide lock serialises appends; O_APPEND keeps lines whole across processes.
    lock: Mutex<()>,
}

impl Audit {
    pub fn open(paths: &Paths) -> Result<Audit> {
        std::fs::create_dir_all(paths.logs())?;
        Ok(Audit {
            path: paths.logs().join("audit.jsonl"),
            lock: Mutex::new(()),
        })
    }
    /// Append one JSON line (0600 file). Rejects events whose fields contain keys named like
    /// `prompt`, `content`, `messages`, `api_key`, `private_key`, `pkcs8` (at any depth) or any
    /// string value longer than 2 KiB, with `InvalidRequest`.
    pub fn record(&self, kind: &str, peer_id: Option<&str>, fields: serde_json::Value) -> Result<()> {
        if kind.trim().is_empty() || kind.len() > 64 {
            return Err(CommonError::invalid("audit kind must be 1..=64 characters"));
        }
        if peer_id.is_some_and(|p| p.len() > 128) {
            return Err(CommonError::invalid("audit peer_id too long"));
        }
        check_fields(&fields)?;
        let event = AuditEvent {
            ts_utc: utc_now(),
            kind: kind.to_string(),
            peer_id: peer_id.map(str::to_string),
            fields,
        };
        let mut line = serde_json::to_string(&event)?;
        line.push('\n');
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut opts = std::fs::OpenOptions::new();
        opts.append(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&self.path)?;
        tighten_mode(&self.path)?;
        f.write_all(line.as_bytes())?;
        Ok(())
    }
    /// The last `n` events, oldest first. Lines that do not parse (a torn write) are skipped.
    pub fn tail(&self, n: usize) -> Result<Vec<AuditEvent>> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(e.into()),
        };
        let mut events: Vec<AuditEvent> = text
            .lines()
            .rev()
            .filter_map(|l| serde_json::from_str(l).ok())
            .take(n)
            .collect();
        events.reverse();
        Ok(events)
    }
}

fn check_fields(v: &serde_json::Value) -> Result<()> {
    match v {
        serde_json::Value::String(s) if s.len() > MAX_FIELD_STRING => Err(CommonError::invalid(format!(
            "audit field string exceeds {MAX_FIELD_STRING} bytes; audit entries carry ids and sizes, never content"
        ))),
        serde_json::Value::Array(items) => items.iter().try_for_each(check_fields),
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                if FORBIDDEN_FIELD_KEYS.contains(&k.to_ascii_lowercase().as_str()) {
                    return Err(CommonError::invalid(format!("audit fields must not contain key `{k}`")));
                }
                check_fields(v)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Force 0600 on an existing file (older builds, copies, umask).
fn tighten_mode(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::metadata(path)?.permissions().mode() & 0o777 != 0o600 {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    let _ = path;
    Ok(())
}

/// Current time as `YYYY-MM-DDTHH:MM:SSZ` from `std::time` (no chrono).
pub fn utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_utc(secs)
}

/// Civil date from Unix seconds (Howard Hinnant's `civil_from_days`).
fn format_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

// ---- redaction ----

/// JSON keys whose string values are replaced by `<redacted>` wherever they appear in text.
pub const REDACTED_JSON_KEYS: &[&str] = &[
    "prompt",
    "content",
    "input",
    "api_key",
    "authorization",
    "private_key",
    "pkcs8",
];
/// Hex runs at least this long are masked (32-byte seeds and keys are 64 hex, PKCS#8 DER is longer).
pub const HEX_MASK_MIN: usize = 64;

/// Redact secrets from free text: `Bearer` tokens, `sk-` style API keys, hex runs of 64+ chars
/// (keys; also SHA-256 digests, see `redact_allowing`), PEM blocks and the string values of
/// `REDACTED_JSON_KEYS` (also when the JSON is itself escaped inside a JSON string, in which case
/// redaction may run to the end of the enclosing string: over-redaction is the safe direction).
pub fn redact(text: &str) -> String {
    redact_allowing(text, &[])
}

/// `redact`, but 64+ hex runs listed in `allow` (known peer ids, model digests) are kept.
pub fn redact_allowing(text: &str, allow: &[&str]) -> String {
    let mut out = redact_pem(text);
    out = redact_prefixed(
        &out,
        "Bearer ",
        |c| c.is_ascii_alphanumeric() || "-._~+/=".contains(c),
        1,
        "Bearer <redacted>",
    );
    out = redact_prefixed(
        &out,
        "sk-",
        |c| c.is_ascii_alphanumeric() || c == '-' || c == '_',
        8,
        "sk-<redacted>",
    );
    out = redact_hex_runs(&out, allow);
    for key in REDACTED_JSON_KEYS {
        out = redact_json_string_value(&out, key);
    }
    out
}

/// Replace `<prefix><token>` where token is a run of `is_token` chars of at least `min_len`.
fn redact_prefixed(text: &str, prefix: &str, is_token: fn(char) -> bool, min_len: usize, replacement: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(prefix) {
        // The prefix must start a token: "task-123" must not match "sk-".
        let starts_token = rest[..i].chars().next_back().is_none_or(|c| !c.is_ascii_alphanumeric());
        let after = &rest[i + prefix.len()..];
        let token_len = after
            .chars()
            .take_while(|c| is_token(*c))
            .map(char::len_utf8)
            .sum::<usize>();
        if starts_token && token_len >= min_len {
            out.push_str(&rest[..i]);
            out.push_str(replacement);
            rest = &after[token_len..];
        } else {
            out.push_str(&rest[..i + prefix.len()]);
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

fn redact_hex_runs(text: &str, allow: &[&str]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(|c: char| c.is_ascii_hexdigit()) {
        out.push_str(&rest[..i]);
        let run_len = rest[i..].chars().take_while(char::is_ascii_hexdigit).count();
        let run = &rest[i..i + run_len];
        if run_len >= HEX_MASK_MIN && !allow.iter().any(|a| a.eq_ignore_ascii_case(run)) {
            out.push_str("<redacted-hex>");
        } else {
            out.push_str(run);
        }
        rest = &rest[i + run_len..];
    }
    out.push_str(rest);
    out
}

fn redact_pem(text: &str) -> String {
    const BEGIN: &str = "-----BEGIN";
    const END: &str = "-----END";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(BEGIN) {
        out.push_str(&rest[..i]);
        out.push_str("<redacted-pem>");
        let tail = &rest[i + BEGIN.len()..];
        rest = match tail.find(END) {
            // Skip to the end of the END marker line (`-----END X-----`).
            Some(e) => {
                let after_end = &tail[e + END.len()..];
                match after_end.find("-----") {
                    Some(d) => &after_end[d + 5..],
                    None => "",
                }
            }
            None => "",
        };
    }
    out.push_str(rest);
    out
}

/// Replace the string value of `"key": "..."` (quotes optionally backslash-escaped) with `<redacted>`.
fn redact_json_string_value(text: &str, key: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(key) {
        let after = &rest[i + key.len()..];
        // The key must be quoted (`"key"` or `\"key\"`): a bare word or `prompt_tokens` is not it.
        let close = if after.starts_with('"') {
            1
        } else if after.starts_with("\\\"") {
            2
        } else {
            0
        };
        let opening = (close > 0 && rest[..i].ends_with('"'))
            .then(|| json_string_opening(&after[close..]))
            .flatten();
        match opening {
            Some(open) => {
                out.push_str(&rest[..i + key.len() + close + open]);
                out.push_str("<redacted>");
                rest = skip_json_string_body(&after[close + open..]);
            }
            None => {
                out.push_str(&rest[..i + key.len()]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Redact a JSON document in place: every string (keys included) passes through `redact`, and any
/// value under a `REDACTED_JSON_KEYS` key becomes `"<redacted>"`. Structure is never broken.
pub fn redact_value(v: &mut serde_json::Value) {
    use serde_json::Value;
    match v {
        Value::String(s) => *s = redact(s),
        Value::Array(items) => items.iter_mut().for_each(redact_value),
        Value::Object(map) => {
            let mut next = serde_json::Map::with_capacity(map.len());
            for (k, mut val) in std::mem::take(map) {
                if REDACTED_JSON_KEYS.contains(&k.to_ascii_lowercase().as_str()) {
                    val = Value::String("<redacted>".into());
                } else {
                    redact_value(&mut val);
                }
                next.insert(redact(&k), val);
            }
            *map = next;
        }
        _ => {}
    }
}

/// Byte offset just past the opening quote of a string value, given text starting after the key:
/// optional whitespace, `:`, optional whitespace, `"` (or `\"`). None when the value is not a string.
fn json_string_opening(after_key: &str) -> Option<usize> {
    let bytes = after_key.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if bytes.get(i) != Some(&b':') {
        return None;
    }
    i += 1;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if bytes.get(i) == Some(&b'\\') {
        i += 1;
    }
    (bytes.get(i) == Some(&b'"')).then_some(i + 1)
}

/// Given text starting inside a JSON string body, return the text from its closing quote onward.
fn skip_json_string_body(body: &str) -> &str {
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return &body[i..],
            _ => i += 1,
        }
    }
    ""
}

/// Redacted diagnostics bundle (JSON): version, hardware, audit tail (200), the last 100 lines of
/// every `COMMON_HOME/logs/*.log`, and the caller's `extra` (status, config without keys). The
/// finished document goes through `redact_value`, so every string in every section is covered,
/// 64-hex peer ids in the audit tail included (they look like keys; the short label survives).
pub fn bundle(paths: &Paths, extra: serde_json::Value) -> Result<serde_json::Value> {
    let hardware = match common_hardware::probe() {
        Ok(r) => serde_json::to_value(r)?,
        Err(e) => serde_json::json!({ "error": e.message }),
    };
    let audit_tail = Audit::open(paths)?.tail(200)?;
    let mut logs = serde_json::Map::new();
    if let Ok(rd) = std::fs::read_dir(paths.logs()) {
        let mut files: Vec<PathBuf> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "log") && p.is_file())
            .collect();
        files.sort();
        for p in files {
            // ponytail: reads the whole file; seek-from-end tail when engine logs grow past tens of MB.
            let text = String::from_utf8_lossy(&std::fs::read(&p)?).into_owned();
            let lines: Vec<&str> = text.lines().collect();
            // Redact the tail as one text so multi-line secrets (PEM blocks) are seen whole.
            let tail = redact(&lines[lines.len().saturating_sub(100)..].join("\n"));
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("log").to_string();
            logs.insert(name, serde_json::json!(tail.lines().collect::<Vec<_>>()));
        }
    }
    let mut doc = serde_json::json!({
        "schema_version": 1,
        "generated_at_utc": utc_now(),
        "version": common_core::VERSION,
        "hardware": hardware,
        "audit_tail": audit_tail,
        "logs": logs,
        "extra": extra,
    });
    redact_value(&mut doc);
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use common_core::ErrorCode;
    use serde_json::json;

    fn paths() -> (tempfile::TempDir, Paths) {
        let t = tempfile::tempdir().unwrap();
        let p = Paths::at(t.path());
        (t, p)
    }

    #[test]
    fn audit_record_and_tail() {
        let (_t, p) = paths();
        let a = Audit::open(&p).unwrap();
        assert!(a.tail(5).unwrap().is_empty(), "missing file is empty, not an error");
        for i in 0..5 {
            a.record(
                "peer_connected",
                Some("ab".repeat(32).as_str()),
                json!({ "n": i, "reason": "ok" }),
            )
            .unwrap();
        }
        a.record("pairing_started", None, serde_json::Value::Null).unwrap();
        let tail = a.tail(3).unwrap();
        assert_eq!(tail.len(), 3);
        assert_eq!(tail[0].fields["n"], 3);
        assert_eq!(tail[2].kind, "pairing_started");
        assert_eq!(tail[2].peer_id, None);
        assert!(tail[0].ts_utc.ends_with('Z'));
        assert_eq!(a.tail(100).unwrap().len(), 6);
        // A torn last line is skipped.
        std::fs::OpenOptions::new()
            .append(true)
            .open(&a.path)
            .unwrap()
            .write_all(b"{\"ts_utc\":\"2026")
            .unwrap();
        assert_eq!(a.tail(100).unwrap().len(), 6);
        assert_eq!(std::fs::read_to_string(&a.path).unwrap().lines().count(), 7);
    }

    #[cfg(unix)]
    #[test]
    fn audit_file_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let (_t, p) = paths();
        let a = Audit::open(&p).unwrap();
        a.record("x", None, json!({})).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&a.path), 0o600);
        std::fs::set_permissions(&a.path, std::fs::Permissions::from_mode(0o644)).unwrap();
        a.record("y", None, json!({})).unwrap();
        assert_eq!(mode(&a.path), 0o600, "tightened on the next append");
    }

    #[test]
    fn audit_rejects_content_bearing_fields() {
        let (_t, p) = paths();
        let a = Audit::open(&p).unwrap();
        let bad = [
            json!({ "prompt": "Once upon a time" }),
            json!({ "Content": "x" }),
            json!({ "messages": [] }),
            json!({ "api_key": "sk-1" }),
            json!({ "private_key": "x" }),
            json!({ "pkcs8": "x" }),
            json!({ "nested": { "deep": [ { "prompt": "x" } ] } }),
            json!({ "long": "a".repeat(MAX_FIELD_STRING + 1) }),
            json!([{ "content": "x" }]),
        ];
        for f in bad {
            let e = a.record("remote_request", None, f.clone()).unwrap_err();
            assert_eq!(e.code, ErrorCode::InvalidRequest, "{f}");
        }
        assert_eq!(
            a.record("", None, json!({})).unwrap_err().code,
            ErrorCode::InvalidRequest
        );
        assert_eq!(
            a.record("k", Some("x".repeat(129).as_str()), json!({}))
                .unwrap_err()
                .code,
            ErrorCode::InvalidRequest
        );
        assert!(!a.path.exists(), "nothing was written");
        a.record(
            "remote_request",
            None,
            json!({ "prompt_tokens": 12, "bytes": 4096, "ok": "a".repeat(MAX_FIELD_STRING) }),
        )
        .unwrap();
        assert_eq!(a.tail(1).unwrap().len(), 1);
    }

    #[test]
    fn redaction_rules() {
        // Bearer tokens.
        assert_eq!(
            redact("Authorization: Bearer eyJhbGciOi.payload_x-1 next"),
            "Authorization: Bearer <redacted> next"
        );
        // sk- keys, at token boundaries only.
        assert_eq!(redact("key=sk-proj-ABCdef123456 done"), "key=sk-<redacted> done");
        assert_eq!(
            redact("task-123 sk-ab"),
            "task-123 sk-ab",
            "short or embedded runs are kept"
        );
        // Hex runs >= 64 are masked, shorter are not; the allow-list keeps named digests.
        let key = "ab".repeat(32);
        let der = "0123456789abcdef".repeat(6);
        assert_eq!(
            redact(&format!("seed {key} der {der} short deadbeef")),
            "seed <redacted-hex> der <redacted-hex> short deadbeef"
        );
        assert_eq!(
            redact_allowing(&format!("model sha256-{key}"), &[key.as_str()]),
            format!("model sha256-{key}")
        );
        let upper = key.to_uppercase();
        assert_eq!(
            redact_allowing(&format!("model sha256-{key}"), &[upper.as_str()]),
            format!("model sha256-{key}")
        );
        // PEM blocks, including unterminated ones.
        let pem = "-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIGx\n-----END PRIVATE KEY-----\nafter";
        assert_eq!(redact(pem), "<redacted-pem>\nafter");
        assert_eq!(redact("x -----BEGIN PRIVATE KEY-----\nMC4C"), "x <redacted-pem>");
        // JSON string values of sensitive keys, with escapes inside the value.
        assert_eq!(
            redact(r#"{"prompt": "say \"hi\"", "max_tokens": 8}"#),
            r#"{"prompt": "<redacted>", "max_tokens": 8}"#
        );
        assert_eq!(
            redact(r#"{"messages":[{"role":"user","content":"secret"}]}"#),
            r#"{"messages":[{"role":"user","content":"<redacted>"}]}"#
        );
        // Escaped JSON inside a JSON string is redacted too, and the result still parses.
        let nested = serde_json::to_string(&json!({ "line": r#"{"prompt":"secret"}"#, "n": 1 })).unwrap();
        let red = redact(&nested);
        assert!(!red.contains("secret"), "{red}");
        assert_eq!(serde_json::from_str::<serde_json::Value>(&red).unwrap()["n"], 1);
        // Structured redaction: forbidden keys lose their value whatever its type; other strings are scanned.
        let mut v = json!({ "Prompt": [1, 2], "config": { "api_key": "hunter2", "note": format!("token sk-{}", "a".repeat(20)) } });
        redact_value(&mut v);
        assert_eq!(
            v,
            json!({ "Prompt": "<redacted>", "config": { "api_key": "<redacted>", "note": "token sk-<redacted>" } })
        );
        // Non-string values and bare words are left alone.
        assert_eq!(
            redact(r#"{"prompt": [1, 2], "prompt_tokens": 3}"#),
            r#"{"prompt": [1, 2], "prompt_tokens": 3}"#
        );
        assert_eq!(redact("the prompt was long"), "the prompt was long");
        assert_eq!(redact(""), "");
        // Idempotent.
        let once = redact(pem);
        assert_eq!(redact(&once), once);
    }

    /// Returns every secret that survives in `text`.
    fn secret_scan_of_bundle<'a>(text: &str, secrets: &[&'a str]) -> Vec<&'a str> {
        secrets.iter().copied().filter(|s| text.contains(s)).collect()
    }

    #[test]
    fn diagnostics_bundle_redacted() {
        let (_t, p) = paths();
        let api_key = "sk-live-ZZtopSECRET1234567890";
        let bearer = "eyJhbGciOiJIUzI1NiJ9.SECRETPAYLOAD.sig";
        let prompt = "Once upon a time, a little rabbit told me the launch codes";
        let seed_hex = "deadbeef".repeat(8);
        let pem_body = "MC4CAQAwBQYDK2VwBCIEIKEYMATERIALKEYMATERIALKEYMATERIAL";
        std::fs::create_dir_all(p.logs()).unwrap();
        std::fs::write(
            p.logs().join("llama-server.log"),
            format!(
                "info: request {{\"prompt\": \"{prompt}\", \"n_predict\": 32}}\n\
                 info: header Authorization: Bearer {bearer}\n\
                 info: api key {api_key}\n\
                 info: key {seed_hex}\n\
                 -----BEGIN PRIVATE KEY-----\n{pem_body}\n-----END PRIVATE KEY-----\n\
                 info: harmless line\n"
            ),
        )
        .unwrap();
        std::fs::write(p.logs().join("notes.txt"), format!("not a log: {api_key}")).unwrap();
        let audit = Audit::open(&p).unwrap();
        audit
            .record(
                "remote_request",
                Some("ab".repeat(32).as_str()),
                json!({ "bytes": 10, "request_id": "r1" }),
            )
            .unwrap();
        audit
            .record(
                "peer_rejected",
                Some("cd".repeat(32).as_str()),
                json!({ "reason": "not paired" }),
            )
            .unwrap();
        let extra = json!({ "config": { "api_key": api_key, "host": "127.0.0.1" }, "status": "idle" });

        let b = bundle(&p, extra).unwrap();
        let text = serde_json::to_string(&b).unwrap();
        let leaked = secret_scan_of_bundle(&text, &[api_key, bearer, prompt, &seed_hex, pem_body, "launch codes"]);
        assert!(leaked.is_empty(), "secrets survived in the bundle: {leaked:?}\n{text}");

        // Shape: what a support engineer still gets.
        assert_eq!(b["schema_version"], 1);
        assert_eq!(b["version"], common_core::VERSION);
        assert!(b["hardware"]["cpu"].is_object() || b["hardware"]["error"].is_string());
        assert_eq!(b["audit_tail"].as_array().unwrap().len(), 2);
        assert_eq!(b["audit_tail"][0]["fields"]["request_id"], "r1");
        let lines = b["logs"]["llama-server.log"].as_array().unwrap();
        assert_eq!(
            lines.len(),
            6,
            "the three PEM lines collapse into one <redacted-pem>: {lines:?}"
        );
        assert_eq!(lines[4], "<redacted-pem>");
        assert!(lines[5].as_str().unwrap().contains("harmless line"));
        assert!(lines[0].as_str().unwrap().contains("\"n_predict\": 32"), "{}", lines[0]);
        assert!(b["logs"].get("notes.txt").is_none(), "only *.log files are collected");
        assert_eq!(b["extra"]["config"]["host"], "127.0.0.1");
        assert_eq!(b["extra"]["config"]["api_key"], "<redacted>");
        assert_eq!(b["extra"]["status"], "idle");
    }

    #[test]
    fn bundle_log_tail_is_last_100_lines() {
        let (_t, p) = paths();
        std::fs::create_dir_all(p.logs()).unwrap();
        let body: String = (0..250).map(|i| format!("line {i}\n")).collect();
        std::fs::write(p.logs().join("engine.log"), body).unwrap();
        let b = bundle(&p, serde_json::Value::Null).unwrap();
        let lines = b["logs"]["engine.log"].as_array().unwrap();
        assert_eq!(lines.len(), 100);
        assert_eq!(lines[0], "line 150");
        assert_eq!(lines[99], "line 249");
        assert!(b["audit_tail"].as_array().unwrap().is_empty());
    }

    #[test]
    fn format_utc_known_values() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc(1_700_000_000), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn types_are_send_sync() {
        fn check<T: Send + Sync>() {}
        check::<Audit>();
    }
}
