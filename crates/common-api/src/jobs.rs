//! `GET /api/jobs` (desktop UI 2.0, E3): the requests this daemon runs, sends to a paired device or
//! serves for one, from the moment the handler starts (before admission) to the end, then the last
//! 256 finished. In memory only; ids, counts, codes and durations, never prompt or completion text.
//! So a daemon that dies mid-request leaves no `running` row behind: its restart lists no job of the
//! old process (F-08-04, `tests/jobs.rs`); the caller saw its stream end.

use crate::AppState;
use axum::extract::State;
use axum::http::HeaderMap;
use common_observability::jobs::JobHandle;
use common_runtime::GenerateStream;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn jobs_get(State(st): State<Arc<AppState>>) -> axum::Json<Value> {
    let (active, recent) = st.jobs.list();
    axum::Json(json!({ "active": active, "recent": recent }))
}

/// `X-Common-Client-Ref`: a caller's own id for a request (1..=64 of `[A-Za-z0-9-]`, else ignored).
pub(crate) fn client_ref(headers: &HeaderMap) -> Option<String> {
    let v = headers.get("x-common-client-ref")?.to_str().ok()?;
    let ok = (1..=64).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    ok.then(|| v.to_string())
}

/// `gen` with `job` following it: each piece, the engine's numbers at Done, an error. Dropping the
/// stream unfinished (client gone, Stop) ends the job `cancelled`.
pub(crate) fn track(gen: GenerateStream, job: JobHandle) -> GenerateStream {
    gen.inspect(move |ev| common_network::remote::job_event(&job, ev))
        .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_ref_is_an_id_or_nothing() {
        let with = |v: &str| {
            let mut h = HeaderMap::new();
            h.insert("x-common-client-ref", v.parse().unwrap());
            client_ref(&h)
        };
        let uuid = "0b7c6f5e-2a7d-4e1b-9c3f-1d2e3f4a5b6c";
        assert_eq!(with(uuid).as_deref(), Some(uuid));
        for bad in ["", "a b", "a/b", "<script>", &"x".repeat(65), "naïve"] {
            assert_eq!(with(bad), None, "{bad:?}");
        }
        assert_eq!(client_ref(&HeaderMap::new()), None);
    }
}
