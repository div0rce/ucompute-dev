//! Audit trail and redacted diagnostics (F-06-10).
//!
//! `audit.jsonl` (0600) under `COMMON_HOME/logs/` records every off-device action: pairing,
//! peer connect/disconnect/reject, revocation, remote requests, sessions. Never prompt text,
//! token strings or key material: `record` refuses fields whose keys could carry them and any
//! string longer than 2 KiB. Every string of the diagnostics bundle passes through `redact`, the
//! single place that knows the secret patterns, and a test scans the result.
//!
//! Retention: at 8 MiB `audit.jsonl` rotates to `audit.1.jsonl` (then `.2`, `.3`; the oldest is
//! dropped), so the trail is bounded at about 32 MiB: retained, not append-only forever. Every
//! rotation opens the fresh file with an `audit_rotated` line (`file`, `rotated_bytes`,
//! `dropped_bytes`), so a drop is always visible. `SECURITY_KINDS` (pairing, revocation, settings)
//! are also copied to `audit.security.jsonl`, rotated on its own, so a paired device flooding
//! refused requests rotates away request history only; `tail` adds those copies back once
//! `audit.jsonl` no longer holds them. `tail` reads backwards across the files.

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

/// A trail rotates once an append leaves its live file at least this large.
pub const AUDIT_ROTATE_BYTES: u64 = 8 * 1024 * 1024;
/// Rotated files kept per trail: `audit.1.jsonl` (newest) to `audit.3.jsonl` (oldest).
pub const AUDIT_ROTATED_FILES: usize = 3;
/// Kinds also copied to `audit.security.jsonl`. Only kinds a remote device cannot produce at
/// will: owner actions and revocations (bounded by the pinned set). Kinds a peer can trigger
/// repeatedly (`peer_rejected`, `remote_request`, `peer_connected`, `pairing_failed`, ...) are not,
/// or a flood would rotate that file away too.
pub const SECURITY_KINDS: &[&str] = &[
    "pairing_started",
    "pairing_succeeded",
    "pairing_locked",
    "peer_revoked",
    "peer_removed",
    "privacy_changed",
    "network_changed",
    "group_updated",
];
/// `tail` reads each file backwards in blocks of this size.
const TAIL_BLOCK: u64 = 64 * 1024;

pub struct Audit {
    /// `audit.jsonl`: every event.
    pub path: PathBuf,
    /// `audit.security.jsonl`: a copy of the `SECURITY_KINDS` events, rotated on its own.
    pub security_path: PathBuf,
    // ponytail: one process-wide lock serialises appends; O_APPEND keeps lines whole across processes.
    lock: Mutex<()>,
    /// `AUDIT_ROTATE_BYTES`; tests lower it.
    pub(crate) rotate_at: u64,
}

impl Audit {
    pub fn open(paths: &Paths) -> Result<Audit> {
        std::fs::create_dir_all(paths.logs())?;
        Ok(Audit {
            path: paths.logs().join("audit.jsonl"),
            security_path: paths.logs().join("audit.security.jsonl"),
            lock: Mutex::new(()),
            rotate_at: AUDIT_ROTATE_BYTES,
        })
    }
    /// Append one JSON line (0600 file), plus its copy for `SECURITY_KINDS`. Rejects events whose
    /// fields contain keys named like `prompt`, `content`, `messages`, `api_key`, `private_key`,
    /// `pkcs8` (at any depth) or any string value longer than 2 KiB, with `InvalidRequest`.
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
        self.append(&self.path, &line)?;
        if SECURITY_KINDS.contains(&kind) {
            self.append(&self.security_path, &line)?;
        }
        Ok(())
    }

    fn append(&self, path: &Path, line: &str) -> Result<()> {
        let mut f = open_append(path)?;
        f.write_all(line.as_bytes())?;
        if f.metadata()?.len() >= self.rotate_at {
            drop(f);
            // The line is written; a failed rotation only lets the file grow, so it is not an error.
            if let Err(e) = self.rotate(path) {
                tracing::warn!(error = %e.message, "audit rotation failed; the live file keeps growing");
            }
        }
        Ok(())
    }

    /// `X.2` -> `X.3` (old `.3` dropped), `X.1` -> `X.2`, `X.jsonl` -> `X.1`, then a fresh 0600
    /// `X.jsonl` that opens with an `audit_rotated` line naming the bytes rotated and dropped.
    // ponytail: appends stay lock-free O_APPEND writes on a file opened per record; only rotation
    // takes the trail's `X.jsonl.lock` flock and re-checks the size under it, so two processes (daemon +
    // CLI) crossing the threshold together rotate once. A writer that opened `audit.jsonl` just before
    // a rotation writes into the renamed inode, i.e. `audit.1.jsonl`: never lost, though lines at the
    // seam may be slightly out of order. Ceiling: that line is dropped only if its writer stalls
    // between open and write through three more rotations (24 MiB); writers other than `Audit` are
    // not coordinated (advisory lock). Upgrade: hold the flock across every append.
    fn rotate(&self, path: &Path) -> Result<()> {
        let lock = open_append(&path.with_extension("jsonl.lock"))?;
        lock.lock()?;
        let rotated_bytes = match std::fs::metadata(path) {
            Ok(m) if m.len() >= self.rotate_at => m.len(),
            Ok(_) => return Ok(()), // another handle or process rotated first
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        let dropped_bytes = std::fs::metadata(rotated(path, AUDIT_ROTATED_FILES)).map_or(0, |m| m.len());
        for i in (1..AUDIT_ROTATED_FILES).rev() {
            match std::fs::rename(rotated(path, i), rotated(path, i + 1)) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
        }
        std::fs::rename(path, rotated(path, 1))?;
        let marker = AuditEvent {
            ts_utc: utc_now(),
            kind: "audit_rotated".into(),
            peer_id: None,
            fields: serde_json::json!({
                "file": path.file_name().and_then(|n| n.to_str()).unwrap_or_default(),
                "rotated_bytes": rotated_bytes,
                "dropped_bytes": dropped_bytes,
            }),
        };
        let mut line = serde_json::to_string(&marker)?;
        line.push('\n');
        open_append(path)?.write_all(line.as_bytes())?;
        Ok(())
    }

    /// The last `n` events, oldest first, continuing into `audit.1.jsonl`.. when the live file
    /// holds fewer. When all of `audit.jsonl` holds fewer than `n`, the `audit.security.jsonl`
    /// copies older than its oldest line (rotated away) come first. Files are read backwards in
    /// blocks, so the cost follows `n`, not the file size. Lines that do not parse are skipped.
    // ponytail: a rotation racing this read can repeat or skip lines at the file seam; tail is a
    // snapshot for people and the bundle. Take the rotation flock here if an exact window matters.
    // ponytail: "rotated away" is decided by `ts_utc` (one-second resolution) and, within the
    // oldest second, by equality with a kept line; a wall clock stepped backwards can show a copy
    // twice. A sequence number per line if that ever matters.
    pub fn tail(&self, n: usize) -> Result<Vec<AuditEvent>> {
        let mut out = tail_trail(&self.path, n)?; // newest first
        if out.len() < n {
            let oldest = out.last().map(|e| e.ts_utc.clone());
            let copies_kept = out.iter().filter(|e| SECURITY_KINDS.contains(&e.kind.as_str())).count();
            let rotated_away: Vec<AuditEvent> =
                tail_trail(&self.security_path, (n - out.len()).saturating_add(copies_kept))?
                    .into_iter()
                    .filter(|e| {
                        oldest
                            .as_ref()
                            .is_none_or(|o| e.ts_utc < *o || (e.ts_utc == *o && !out.contains(e)))
                    })
                    .collect();
            let room = n - out.len();
            out.extend(rotated_away.into_iter().take(room));
        }
        out.reverse();
        Ok(out)
    }
}

/// `X.N.jsonl` for `X.jsonl` (1 = newest).
fn rotated(path: &Path, n: usize) -> PathBuf {
    path.with_extension(format!("{n}.jsonl"))
}

/// The last `n` events of one trail (`path`, then `.1.jsonl`..), newest first.
fn tail_trail(path: &Path, n: usize) -> Result<Vec<AuditEvent>> {
    let mut out = Vec::new();
    for i in 0..=AUDIT_ROTATED_FILES {
        if out.len() >= n {
            break;
        }
        let file = if i == 0 { path.to_path_buf() } else { rotated(path, i) };
        tail_file(&file, n, &mut out)?;
    }
    Ok(out)
}

/// Push events from the end of `path` onto `out` (newest first) until it holds `n`; a missing file
/// adds nothing. Returns the bytes read.
fn tail_file(path: &Path, n: usize, out: &mut Vec<AuditEvent>) -> Result<u64> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e.into()),
    };
    let mut pos = f.metadata()?.len();
    let mut read = 0;
    // The text before the first newline of the block read last: a line that starts further back.
    let mut carry = Vec::new();
    while pos > 0 && out.len() < n {
        let step = pos.min(TAIL_BLOCK);
        pos -= step;
        let mut buf = vec![0; step as usize];
        f.seek(SeekFrom::Start(pos))?;
        f.read_exact(&mut buf)?;
        read += step;
        buf.extend_from_slice(&carry);
        let complete_from = if pos == 0 {
            0
        } else {
            match buf.iter().position(|&b| b == b'\n') {
                Some(i) => i + 1,
                None => {
                    carry = buf; // one line longer than a block
                    continue;
                }
            }
        };
        for line in buf[complete_from..].rsplit(|&b| b == b'\n') {
            if out.len() >= n {
                break;
            }
            if let Ok(e) = serde_json::from_slice(line) {
                out.push(e);
            }
        }
        buf.truncate(complete_from.saturating_sub(1));
        carry = buf;
    }
    Ok(read)
}

/// Open for append, creating it 0600.
fn open_append(path: &Path) -> Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let f = opts.open(path)?;
    tighten_mode(&f)?;
    Ok(f)
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

/// Force 0600 on an existing file (older builds, copies, umask). By handle: the path may be
/// mid-rotation.
fn tighten_mode(f: &std::fs::File) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if f.metadata()?.permissions().mode() & 0o777 != 0o600 {
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
    }
    let _ = f;
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

// ---- job registry (desktop UI 2.0, E3) ----

/// The requests this daemon runs, sends to a paired device or serves for one, in memory only (lost
/// at restart): the active ones and the last [`jobs::MAX_RECENT`] finished. Ids, counts, codes and
/// durations only: no field of [`jobs::Job`] can hold prompt or completion text.
pub mod jobs {
    use super::format_utc;
    use serde::Serialize;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    /// Finished jobs kept.
    pub const MAX_RECENT: usize = 256;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
    #[serde(rename_all = "snake_case")]
    pub enum JobState {
        /// Waiting for an engine slot (local admission); for a request to a paired device, which
        /// takes no slot here, reaching that device until the request is sent (then `Running`).
        Queued,
        /// In the model step (a load, or finding the loaded model).
        Loading,
        /// Generating, or sent to a paired device.
        Running,
        Done,
        Error,
        Cancelled,
    }

    /// One request. Every key is always serialized; `null` means not known (yet).
    #[derive(Debug, Clone, Serialize)]
    pub struct Job {
        /// The response id (`chatcmpl-…`, `cmpl-…`), a registry id, or the peer's `request_id`.
        pub id: String,
        /// `X-Common-Client-Ref` of the request (an id, never text).
        pub client_ref: Option<String>,
        /// `chat`, `completion`, `embeddings` or `served_for_peer`.
        pub kind: String,
        pub model_id: String,
        /// `local` or `peer:<hex>`: where it runs.
        pub target: String,
        /// `local` or `peer:<hex>`: who asked.
        pub origin: String,
        pub state: JobState,
        pub created_at_utc: String,
        pub started_at_utc: Option<String>,
        pub first_token_at_utc: Option<String>,
        pub finished_at_utc: Option<String>,
        pub queue_ms: Option<u64>,
        pub load_ms: Option<u64>,
        pub ttft_ms: Option<u64>,
        pub prompt_ms: Option<f64>,
        pub decode_ms: Option<f64>,
        pub duration_ms: Option<u64>,
        pub prompt_tokens: Option<u32>,
        /// Counted per streamed piece while active; the engine's final count once done.
        pub completion_tokens: Option<u32>,
        pub decode_tokens_per_second: Option<f64>,
        pub finish_reason: Option<String>,
        pub error_code: Option<String>,
        /// Requests to or from a paired device: joins the `remote_request` audit entries.
        pub request_id: Option<String>,
        pub device_actual: Option<String>,
    }

    /// What the engine reported at the end of a generation.
    #[derive(Debug, Clone, Default)]
    pub struct Finished {
        pub finish_reason: Option<String>,
        pub prompt_tokens: Option<u32>,
        pub completion_tokens: Option<u32>,
        pub prompt_ms: Option<f64>,
        pub decode_ms: Option<f64>,
        pub decode_tokens_per_second: Option<f64>,
    }

    /// `YYYY-MM-DDTHH:MM:SS.mmmZ`.
    pub fn utc_ms(t: SystemTime) -> String {
        let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
        let s = format_utc(d.as_secs());
        format!("{}.{:03}Z", s.trim_end_matches('Z'), d.subsec_millis())
    }

    fn ms(since: Instant) -> u64 {
        since.elapsed().as_millis() as u64
    }

    struct Entry {
        key: u64,
        job: Job,
        created: Instant,
        started: Option<Instant>,
    }

    #[derive(Default)]
    struct Inner {
        next: u64,
        active: Vec<Entry>,
        recent: VecDeque<Job>,
    }

    /// The registry; one per daemon, shared with peer serving.
    #[derive(Default)]
    pub struct Jobs(Mutex<Inner>);

    impl Jobs {
        pub fn new() -> Arc<Jobs> {
            Arc::default()
        }

        fn lock(&self) -> MutexGuard<'_, Inner> {
            self.0.lock().unwrap_or_else(PoisonError::into_inner)
        }

        /// Register a request as `queued`. The handle moves it on and finishes it, at the latest
        /// when dropped (`cancelled`: the request went away without an outcome).
        pub fn start(
            self: &Arc<Self>,
            id: String,
            kind: &str,
            model_id: &str,
            target: String,
            origin: String,
            client_ref: Option<String>,
        ) -> JobHandle {
            let mut job = Job {
                id,
                client_ref,
                kind: kind.into(),
                // The request's own string, checked only later: bounded like a catalog id.
                model_id: model_id.chars().take(200).collect(),
                target,
                origin,
                state: JobState::Queued,
                created_at_utc: utc_ms(SystemTime::now()),
                started_at_utc: None,
                first_token_at_utc: None,
                finished_at_utc: None,
                queue_ms: None,
                load_ms: None,
                ttft_ms: None,
                prompt_ms: None,
                decode_ms: None,
                duration_ms: None,
                prompt_tokens: None,
                completion_tokens: None,
                decode_tokens_per_second: None,
                finish_reason: None,
                error_code: None,
                request_id: None,
                device_actual: None,
            };
            let mut g = self.lock();
            // A ref is the caller's own id for one request: a second request with a ref the
            // registry already holds (any local client can read refs from /api/jobs) gets none, so
            // its row never passes for the first caller's (the desktop matches rows and Stop by ref).
            let taken = job.client_ref.as_ref().is_some_and(|r| {
                g.active.iter().any(|e| e.job.client_ref.as_ref() == Some(r))
                    || g.recent.iter().any(|j| j.client_ref.as_ref() == Some(r))
            });
            if taken {
                job.client_ref = None;
            }
            g.next += 1;
            let key = g.next;
            g.active.push(Entry {
                key,
                job,
                created: Instant::now(),
                started: None,
            });
            JobHandle {
                jobs: self.clone(),
                key,
            }
        }

        /// Active jobs (oldest first) and finished ones (newest first).
        pub fn list(&self) -> (Vec<Job>, Vec<Job>) {
            let g = self.lock();
            (
                g.active.iter().map(|e| e.job.clone()).collect(),
                g.recent.iter().cloned().collect(),
            )
        }
    }

    /// Updates one job; see [`Jobs::start`].
    pub struct JobHandle {
        jobs: Arc<Jobs>,
        key: u64,
    }

    impl JobHandle {
        fn with(&self, f: impl FnOnce(&mut Entry)) {
            let mut g = self.jobs.lock();
            if let Some(e) = g.active.iter_mut().find(|e| e.key == self.key) {
                f(e);
            }
        }

        fn begin(e: &mut Entry) {
            if e.started.is_none() {
                e.started = Some(Instant::now());
                e.job.started_at_utc = Some(utc_ms(SystemTime::now()));
                e.job.queue_ms = Some(ms(e.created));
            }
        }

        /// Admitted: `loading` (the model step comes next).
        pub fn started(&self) {
            self.with(|e| {
                Self::begin(e);
                e.job.state = JobState::Loading;
            });
        }

        /// The model is ready: `running`; `load_ms` is the time since admission.
        pub fn loaded(&self, device_actual: &str) {
            self.with(|e| {
                Self::begin(e);
                e.job.load_ms = e.started.map(ms);
                e.job.device_actual = Some(device_actual.into());
                e.job.state = JobState::Running;
            });
        }

        /// Sent to a paired device: `running` there (no local slot or load).
        pub fn sent(&self, request_id: &str) {
            self.with(|e| {
                Self::begin(e);
                e.job.request_id = Some(request_id.into());
                e.job.state = JobState::Running;
            });
        }

        pub fn set_request_id(&self, request_id: &str) {
            self.with(|e| e.job.request_id = Some(request_id.into()));
        }

        /// One more streamed piece (a token or a tool-call delta).
        pub fn token(&self) {
            self.with(|e| {
                if e.job.first_token_at_utc.is_none() {
                    e.job.first_token_at_utc = Some(utc_ms(SystemTime::now()));
                    e.job.ttft_ms = Some(ms(e.created));
                }
                e.job.completion_tokens = Some(e.job.completion_tokens.unwrap_or(0) + 1);
            });
        }

        /// Finished normally; the engine's numbers replace the live count.
        pub fn done(&self, f: Finished) {
            self.finish(JobState::Done, |j| {
                j.finish_reason = f.finish_reason;
                j.prompt_tokens = f.prompt_tokens;
                j.completion_tokens = f.completion_tokens.or(j.completion_tokens);
                j.prompt_ms = f.prompt_ms;
                j.decode_ms = f.decode_ms;
                j.decode_tokens_per_second = f.decode_tokens_per_second;
            });
        }

        /// Ended with an error code; `cancelled` and `client_gone` end as `cancelled`.
        pub fn fail(&self, code: &str) {
            let state = match code {
                "cancelled" | "client_gone" => JobState::Cancelled,
                _ => JobState::Error,
            };
            self.finish(state, |j| j.error_code = Some(code.into()));
        }

        /// Moves the job to `recent` (once; later calls do nothing).
        fn finish(&self, state: JobState, f: impl FnOnce(&mut Job)) {
            let mut g = self.jobs.lock();
            let Some(i) = g.active.iter().position(|e| e.key == self.key) else {
                return;
            };
            let e = g.active.remove(i);
            let mut job = e.job;
            f(&mut job);
            job.state = state;
            job.finished_at_utc = Some(utc_ms(SystemTime::now()));
            job.duration_ms = Some(ms(e.created));
            g.recent.push_front(job);
            g.recent.truncate(MAX_RECENT);
        }
    }

    impl Drop for JobHandle {
        fn drop(&mut self) {
            self.fail("cancelled");
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn start(jobs: &Arc<Jobs>, id: &str) -> JobHandle {
            jobs.start(
                id.into(),
                "chat",
                "m",
                "local".into(),
                "local".into(),
                Some("ref-1".into()),
            )
        }

        #[test]
        fn a_job_moves_from_active_to_recent_once() {
            let jobs = Jobs::new();
            let h = start(&jobs, "chatcmpl-1");
            let (active, recent) = jobs.list();
            assert_eq!((active.len(), recent.len()), (1, 0));
            assert_eq!(active[0].state, JobState::Queued);
            assert_eq!(active[0].client_ref.as_deref(), Some("ref-1"));
            h.started();
            assert_eq!(jobs.list().0[0].state, JobState::Loading);
            h.loaded("cpu");
            h.token();
            h.token();
            let a = &jobs.list().0[0];
            assert_eq!(
                (a.state, a.completion_tokens, a.device_actual.as_deref()),
                (JobState::Running, Some(2), Some("cpu"))
            );
            assert!(
                a.ttft_ms.is_some() && a.first_token_at_utc.is_some() && a.queue_ms.is_some() && a.load_ms.is_some()
            );
            h.done(Finished {
                finish_reason: Some("length".into()),
                completion_tokens: Some(3),
                ..Default::default()
            });
            h.fail("internal");
            drop(h);
            let (active, recent) = jobs.list();
            assert!(active.is_empty());
            assert_eq!(recent.len(), 1, "finished once");
            assert_eq!(
                (recent[0].state, recent[0].completion_tokens),
                (JobState::Done, Some(3))
            );
            assert!(recent[0].error_code.is_none() && recent[0].duration_ms.is_some());
        }

        #[test]
        fn a_client_ref_already_held_is_not_given_to_another_request() {
            let jobs = Jobs::new();
            let first = start(&jobs, "a");
            let copy = start(&jobs, "b");
            assert_eq!(jobs.list().0[1].client_ref, None, "a ref of an active row");
            drop(first);
            drop(copy);
            let _late = start(&jobs, "c");
            assert_eq!(jobs.list().0[0].client_ref, None, "a ref of a finished row too");
            let _other = jobs.start(
                "d".into(),
                "chat",
                "m",
                "local".into(),
                "local".into(),
                Some("ref-2".into()),
            );
            assert_eq!(jobs.list().0[1].client_ref.as_deref(), Some("ref-2"));
        }

        #[test]
        fn dropped_and_failed_jobs_say_how_they_ended() {
            let jobs = Jobs::new();
            drop(start(&jobs, "a"));
            start(&jobs, "b").fail("overloaded");
            start(&jobs, "c").fail("client_gone");
            let recent = jobs.list().1;
            let got: Vec<(&str, JobState, Option<&str>)> = recent
                .iter()
                .map(|j| (j.id.as_str(), j.state, j.error_code.as_deref()))
                .collect();
            assert_eq!(
                got,
                [
                    ("c", JobState::Cancelled, Some("client_gone")),
                    ("b", JobState::Error, Some("overloaded")),
                    ("a", JobState::Cancelled, Some("cancelled")),
                ],
                "newest first"
            );
        }

        #[test]
        fn recent_is_bounded_and_every_key_is_serialized() {
            let jobs = Jobs::new();
            for i in 0..MAX_RECENT + 10 {
                drop(start(&jobs, &i.to_string()));
            }
            let recent = jobs.list().1;
            assert_eq!(recent.len(), MAX_RECENT);
            assert_eq!(recent[0].id, (MAX_RECENT + 9).to_string());
            let v = serde_json::to_value(&recent[0]).unwrap();
            assert_eq!(v.as_object().unwrap().len(), 24, "{v}");
            assert!(v["prompt_ms"].is_null() && v["state"] == "cancelled");
        }

        #[test]
        fn a_huge_model_field_is_not_kept_whole() {
            let jobs = Jobs::new();
            let _h = jobs.start(
                "x".into(),
                "chat",
                &"m".repeat(100_000),
                "local".into(),
                "local".into(),
                None,
            );
            assert_eq!(jobs.list().0[0].model_id.len(), 200);
        }

        #[test]
        fn timestamps_have_milliseconds() {
            let t = UNIX_EPOCH + std::time::Duration::from_millis(1_700_000_000_042);
            assert_eq!(utc_ms(t), "2023-11-14T22:13:20.042Z");
        }
    }
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

    /// The `i` of every event but the rotation markers.
    fn ids(events: &[AuditEvent]) -> Vec<u64> {
        events
            .iter()
            .filter(|e| e.kind != "audit_rotated")
            .map(|e| e.fields["i"].as_u64().unwrap())
            .collect()
    }

    fn markers(events: &[AuditEvent]) -> Vec<&AuditEvent> {
        events.iter().filter(|e| e.kind == "audit_rotated").collect()
    }

    #[test]
    fn rotation_keeps_three_files_and_0600() {
        let (_t, p) = paths();
        let mut a = Audit::open(&p).unwrap();
        a.rotate_at = 1024;
        let mut i = 0;
        while !rotated(&a.path, 1).exists() {
            a.record("tick", None, json!({ "i": i })).unwrap();
            i += 1;
        }
        // The fresh audit.jsonl holds only the marker: what rotated, nothing dropped yet.
        let fresh = a.tail(1).unwrap();
        assert_eq!(std::fs::read_to_string(&a.path).unwrap().lines().count(), 1);
        assert_eq!(fresh[0].kind, "audit_rotated");
        assert_eq!(fresh[0].fields["file"], "audit.jsonl");
        assert_eq!(
            fresh[0].fields["rotated_bytes"].as_u64(),
            Some(std::fs::metadata(rotated(&a.path, 1)).unwrap().len())
        );
        assert_eq!(fresh[0].fields["dropped_bytes"], 0);
        while i < 200 {
            a.record("tick", None, json!({ "i": i })).unwrap();
            i += 1;
        }
        for n in 1..=3 {
            let len = std::fs::metadata(rotated(&a.path, n)).unwrap().len();
            assert!((1024..1024 + 128).contains(&len), "audit.{n}.jsonl is {len} bytes");
        }
        let mut names: Vec<String> = std::fs::read_dir(p.logs())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "audit.1.jsonl",
                "audit.2.jsonl",
                "audit.3.jsonl",
                "audit.jsonl",
                "audit.jsonl.lock"
            ]
        );
        #[cfg(unix)]
        for name in &names {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(p.logs().join(name)).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{name}");
        }
        // What is kept is contiguous and ends at the newest line; the oldest were dropped, and the
        // markers that open each kept file say so.
        let all = a.tail(usize::MAX).unwrap();
        let kept = ids(&all);
        assert_eq!(*kept.last().unwrap(), 199);
        assert!(kept[0] > 0, "oldest lines dropped: {kept:?}");
        assert!(kept.windows(2).all(|w| w[1] == w[0] + 1), "{kept:?}");
        let marks = markers(&all);
        assert_eq!(marks.len(), 4, "one marker opens each kept file");
        assert_eq!(
            all[0].kind, "audit_rotated",
            "the oldest kept file starts with its marker"
        );
        assert!(
            marks
                .iter()
                .all(|m| m.fields["dropped_bytes"].as_u64().unwrap() >= 1024),
            "{marks:?}"
        );
    }

    #[test]
    fn tail_reads_across_rotation() {
        let (_t, p) = paths();
        let mut a = Audit::open(&p).unwrap();
        a.rotate_at = 512;
        for i in 0..30 {
            a.record("tick", None, json!({ "i": i })).unwrap();
        }
        assert!(rotated(&a.path, 2).exists());
        let live = std::fs::read_to_string(&a.path).unwrap().lines().count();
        assert!(live < 20, "live file holds {live} lines");
        let tail = a.tail(20).unwrap();
        assert_eq!(tail.len(), 20);
        let ticks = ids(&tail);
        assert_eq!(*ticks.last().unwrap(), 29);
        assert!(ticks.windows(2).all(|w| w[1] == w[0] + 1), "{ticks:?}");
        assert!(!markers(&tail).is_empty(), "the window spans a rotation");
        assert!(a.tail(0).unwrap().is_empty());
        // Right after a rotation the live file holds only its marker; tail continues into .1.
        let mut i = 30;
        loop {
            a.record("tick", None, json!({ "i": i })).unwrap();
            i += 1;
            if std::fs::read_to_string(&a.path).unwrap().lines().count() == 1 {
                break;
            }
        }
        let tail = a.tail(3).unwrap();
        assert_eq!(ids(&tail), [i - 2, i - 1]);
        assert_eq!(tail[2].kind, "audit_rotated");
        // A torn last line in the live file is skipped.
        std::fs::OpenOptions::new()
            .append(true)
            .open(&a.path)
            .unwrap()
            .write_all(b"{\"ts_utc\":\"2026")
            .unwrap();
        assert_eq!(ids(&a.tail(3).unwrap()), [i - 2, i - 1]);
    }

    /// F-06-10 retention: a paired device that floods refused requests rotates away request history
    /// only. Pairing, privacy and revocation lines keep their copy, and audit.jsonl says what it
    /// dropped.
    #[test]
    fn security_trail_survives_a_request_flood() {
        let (_t, p) = paths();
        let mut a = Audit::open(&p).unwrap();
        a.rotate_at = 4096;
        let peer = "ab".repeat(32);
        let security = ["pairing_succeeded", "privacy_changed", "peer_revoked"];
        for kind in security {
            a.record(kind, Some(&peer), json!({ "source": "local" })).unwrap();
        }
        for i in 0..5000 {
            a.record(
                "peer_rejected",
                Some(&peer),
                json!({ "reason": "policy_denied", "request_id": format!("r{i}") }),
            )
            .unwrap();
        }
        let in_file: Vec<String> = std::fs::read_to_string(&a.security_path)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str::<AuditEvent>(l).unwrap().kind)
            .collect();
        assert_eq!(in_file, security, "the copy holds only its kinds, all of them");
        assert!(!rotated(&a.security_path, 1).exists(), "the flood never rotated it");
        let in_requests = (0..=AUDIT_ROTATED_FILES)
            .map(|i| if i == 0 { a.path.clone() } else { rotated(&a.path, i) })
            .filter_map(|f| std::fs::read_to_string(f).ok())
            .any(|t| t.contains("pairing_succeeded"));
        assert!(!in_requests, "the flood rotated them out of audit.jsonl");

        let all = a.tail(usize::MAX).unwrap();
        let kinds: Vec<&str> = all.iter().take(3).map(|e| e.kind.as_str()).collect();
        assert_eq!(kinds, security, "tail still shows them, oldest first");
        assert_eq!(all.iter().filter(|e| e.kind == "peer_revoked").count(), 1);
        let first_kept = all.iter().find(|e| e.kind == "peer_rejected").unwrap();
        assert_ne!(first_kept.fields["request_id"], "r0", "the request trail did rotate");
        assert_eq!(all.last().unwrap().fields["request_id"], "r4999");
        let marks = markers(&all);
        assert!(
            marks
                .iter()
                .any(|m| m.fields["file"] == "audit.jsonl" && m.fields["dropped_bytes"].as_u64() > Some(0)),
            "the drop is recorded: {marks:?}"
        );
    }

    /// `tail` answers from `audit.jsonl` alone while it holds `n`; past its oldest line it adds the
    /// security copies rotated away, older first, and never one it still holds.
    #[test]
    fn tail_adds_back_security_copies_rotated_away() {
        let (_t, p) = paths();
        let a = Audit::open(&p).unwrap();
        let write = |path: &Path, lines: &[(u64, &str, u64)]| {
            let text: String = lines
                .iter()
                .map(|(sec, kind, i)| {
                    let e = AuditEvent {
                        ts_utc: format_utc(1_700_000_000 + sec),
                        kind: kind.to_string(),
                        peer_id: None,
                        fields: json!({ "i": i }),
                    };
                    serde_json::to_string(&e).unwrap() + "\n"
                })
                .collect();
            std::fs::write(path, text).unwrap();
        };
        // audit.jsonl kept from second 5 on; pairing (3) and privacy (5, before the kept revocation)
        // were rotated away.
        write(&a.path, &[(5, "peer_revoked", 5), (6, "tick", 6)]);
        write(
            &a.security_path,
            &[
                (3, "pairing_succeeded", 3),
                (5, "privacy_changed", 4),
                (5, "peer_revoked", 5),
            ],
        );
        assert_eq!(ids(&a.tail(10).unwrap()), [3, 4, 5, 6]);
        assert_eq!(ids(&a.tail(3).unwrap()), [4, 5, 6]);
        assert_eq!(ids(&a.tail(2).unwrap()), [5, 6], "audit.jsonl alone answers");
        std::fs::remove_file(&a.path).unwrap();
        assert_eq!(ids(&a.tail(10).unwrap()), [3, 4, 5], "copies alone");
        // Nothing rotated away: each security event shows once, in written order.
        let (_t, p) = paths();
        let a = Audit::open(&p).unwrap();
        a.record("tick", None, json!({ "i": 0 })).unwrap();
        a.record("pairing_started", None, json!({ "i": 1 })).unwrap();
        a.record("tick", None, json!({ "i": 2 })).unwrap();
        assert_eq!(ids(&a.tail(usize::MAX).unwrap()), [0, 1, 2]);
        assert_eq!(std::fs::read_to_string(&a.security_path).unwrap().lines().count(), 1);
    }

    #[test]
    fn tail_is_bounded_on_a_large_file() {
        let (_t, p) = paths();
        let a = Audit::open(&p).unwrap();
        // ~20 MiB written directly: `record` would rotate at 8 MiB.
        let mut w = std::io::BufWriter::new(std::fs::File::create(&a.path).unwrap());
        let (mut n, mut len) = (0u64, 0usize);
        while len < 20 << 20 {
            let line =
                format!("{{\"ts_utc\":\"2026-01-01T00:00:00Z\",\"kind\":\"filler\",\"fields\":{{\"i\":{n}}}}}\n");
            w.write_all(line.as_bytes()).unwrap();
            len += line.len();
            n += 1;
        }
        w.flush().unwrap();
        drop(w);

        let t0 = std::time::Instant::now();
        let tail = a.tail(5).unwrap();
        let took = t0.elapsed();
        eprintln!("tail(5) of a {len}-byte audit file: {took:?}");
        assert_eq!(ids(&tail), (n - 5..n).collect::<Vec<_>>());
        assert!(took < std::time::Duration::from_secs(1), "{took:?}");
        let read = tail_file(&a.path, 5, &mut Vec::new()).unwrap();
        assert!(read <= TAIL_BLOCK, "tail(5) read {read} of {len} bytes");
        // Lines across block seams are stitched whole: 3000 events span several blocks.
        assert_eq!(ids(&a.tail(3000).unwrap()), (n - 3000..n).collect::<Vec<_>>());
    }

    /// Two handles with separate in-process locks stand in for two processes (daemon + CLI): the
    /// rotation flock is per open file, so they exclude each other exactly as processes would.
    #[test]
    fn concurrent_appends_survive_rotation() {
        let (_t, p) = paths();
        let mut a = Audit::open(&p).unwrap();
        let mut b = Audit::open(&p).unwrap();
        // ~74-byte lines, 2000 of them: three rotations, below the four files kept, so none is dropped.
        a.rotate_at = 48 * 1024;
        b.rotate_at = 48 * 1024;
        const PER: u64 = 1000;
        std::thread::scope(|s| {
            for (h, w) in [(&a, "a"), (&b, "b")] {
                s.spawn(move || {
                    for i in 0..PER {
                        h.record("tick", None, json!({ "w": w, "i": i })).unwrap();
                    }
                });
            }
        });
        assert!(rotated(&a.path, 2).exists(), "rotated at least twice");
        let mut seen = std::collections::HashSet::new();
        for i in 0..=AUDIT_ROTATED_FILES {
            let path = if i == 0 { a.path.clone() } else { rotated(&a.path, i) };
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            for line in text.lines() {
                let e: AuditEvent = serde_json::from_str(line).expect("whole lines only");
                if e.kind == "audit_rotated" {
                    assert_eq!(e.fields["dropped_bytes"], 0, "nothing dropped: {line}");
                    continue;
                }
                let key = (
                    e.fields["w"].as_str().unwrap().to_string(),
                    e.fields["i"].as_u64().unwrap(),
                );
                assert!(seen.insert(key), "line written twice: {line}");
            }
        }
        assert_eq!(seen.len() as u64, 2 * PER, "every line accounted for");
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
