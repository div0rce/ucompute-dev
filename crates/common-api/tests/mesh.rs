//! Common Mesh through the daemon API (F-02-01/02/05, F-06-02/05/07/10, F-10-05/10 backends).
//!
//! **SINGLE-HOST**: tests that run two or three servers run them all in this process on this one Mac
//! (`127.0.0.1`, separate temp `COMMON_HOME`s, distinct identities, ephemeral ports). Nothing here
//! satisfies a two-machine criterion. The remote-inference test runs the real pinned llama-server
//! on the serving side (skips loudly without the fixtures; `COMMON_REQUIRE_FIXTURES=1` fails
//! instead). Nothing is mocked.

mod common;

use common::*;
use common_api::mesh::{Settings, DISCLOSURE};
use common_api::{ApiConfig, AppState};
use common_artifacts::Store;
use common_core::Paths;
use common_hardware::HardwareReport;
use common_identity::{DeviceIdentity, PeerId, PeerRecord};
use common_network::NetworkConfig;
use common_runtime::manager::ModelManager;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Peer endpoint on loopback, ephemeral port, no mDNS; still off until `PUT /api/network`.
fn cfg() -> ApiConfig {
    ApiConfig {
        network: NetworkConfig {
            enabled: false,
            listen_port: 0,
            bind_addr: "127.0.0.1".into(),
            mdns: false,
            ..NetworkConfig::default()
        },
        ..ApiConfig::default()
    }
}

async fn empty_server() -> (Server, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let srv = Server::start(app(cfg(), dir.path().to_path_buf(), HardwareReport::default())).await;
    (srv, dir)
}

async fn get(srv: &Server, path: &str) -> Value {
    let r = client().get(srv.url(path)).send().await.unwrap();
    assert_eq!(r.status(), 200, "GET {path}: {}", r.text().await.unwrap());
    r.json().await.unwrap()
}

async fn send(srv: &Server, method: reqwest::Method, path: &str, body: Value) -> (u16, Value) {
    let r = client()
        .request(method, srv.url(path))
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    let text = r.text().await.unwrap();
    let v = if text.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).unwrap()
    };
    (status, v)
}

async fn put(srv: &Server, path: &str, body: Value) -> (u16, Value) {
    send(srv, reqwest::Method::PUT, path, body).await
}

async fn post(srv: &Server, path: &str, body: Value) -> (u16, Value) {
    send(srv, reqwest::Method::POST, path, body).await
}

fn audit_kinds(v: &Value) -> Vec<String> {
    v["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap().to_string())
        .collect()
}

/// Kinds that can only come from an open or attempted peer connection.
const NETWORK_KINDS: &[&str] = &["peer_connected", "peer_rejected", "peer_disconnected", "remote_request"];

/// No request left this device: no `remote_request` entry and no open connection. (With
/// networking on, the sync loop may still dial pinned devices for control; that is not a request.)
async fn assert_no_remote_request(srv: &Server) {
    let node = srv.state.net.lock().await.node.clone();
    assert_eq!(
        node.map_or(0, |n| n.connections().len()),
        0,
        "no peer connection may exist"
    );
    let kinds = audit_kinds(&get(srv, "/api/audit?n=1000").await);
    assert!(!kinds.iter().any(|k| k == "remote_request"), "{kinds:?}");
}

async fn assert_no_network_activity(srv: &Server) {
    let node = srv.state.net.lock().await.node.clone();
    assert_eq!(
        node.map_or(0, |n| n.connections().len()),
        0,
        "no peer connection may exist"
    );
    let kinds = audit_kinds(&get(srv, "/api/audit?n=1000").await);
    assert!(
        !kinds.iter().any(|k| NETWORK_KINDS.contains(&k.as_str())),
        "audit shows network activity: {kinds:?}"
    );
}

async fn enable_network(srv: &Server, serve_peers: bool) -> Value {
    let (status, v) = put(
        srv,
        "/api/network",
        json!({ "enabled": true, "serve_peers": serve_peers }),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["enabled"], true);
    assert_eq!(v["serve_peers"], serve_peers);
    assert!(v["listen_addr"].as_str().unwrap().starts_with("127.0.0.1:"), "{v}");
    assert_eq!(v["mdns"], false);
    v
}

/// `joiner` joins `initiator`'s pairing window with the code it shows; both end up pinned.
async fn pair(initiator: &Server, joiner: &Server, i_name: &str, j_name: &str) -> (PeerId, PeerId) {
    let (status, start) = post(initiator, "/api/pair/start", json!({ "name": i_name })).await;
    assert_eq!(status, 200, "{start}");
    let code = start["code"].as_str().unwrap();
    assert!(code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit()), "{start}");
    let port = start["port"].as_u64().unwrap();
    let me = initiator.state.identity.id;
    assert!(start["qr_payload"]
        .as_str()
        .unwrap()
        .starts_with(&format!("common-pair:v1:{}:127.0.0.1:{port}:", me.to_hex())));
    assert_eq!(start["pair_addr"], format!("127.0.0.1:{port}"));
    assert!(start["expires_at"].as_u64().unwrap() > 0);
    assert_eq!(get(initiator, "/api/pair/status").await["active"], true);
    let (status, v) = post(initiator, "/api/pair/start", json!({})).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (409, Some("overloaded")),
        "one window at a time"
    );

    let (status, rec) = post(
        joiner,
        "/api/pair/join",
        json!({ "addr": format!("127.0.0.1:{port}"), "code": code, "name": j_name }),
    )
    .await;
    assert_eq!(status, 200, "{rec}");
    let pinned: PeerRecord = serde_json::from_value(rec).unwrap();
    assert_eq!((pinned.id, pinned.name.as_str()), (me, i_name));
    let status = wait_for("initiator pairing result", || async {
        let v = get(initiator, "/api/pair/status").await;
        (v["active"] == false).then_some(v)
    })
    .await;
    let other: PeerRecord = serde_json::from_value(status["result"]["peer"].clone()).unwrap();
    assert_eq!((other.id, other.name.as_str()), (joiner.state.identity.id, j_name));
    (me, other.id)
}

async fn wait_for<T, F, Fut>(what: &str, mut f: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let t0 = Instant::now();
    loop {
        if let Some(v) = f().await {
            return v;
        }
        assert!(t0.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn defaults_are_local_only_and_a_peer_target_is_refused_without_any_network() {
    let (srv, dir) = empty_server().await;
    let privacy = get(&srv, "/api/privacy").await;
    assert_eq!(privacy["mode"], "local_only");
    assert_eq!(privacy["disclosure"], DISCLOSURE);
    let net = get(&srv, "/api/network").await;
    assert_eq!(
        (net["enabled"].as_bool(), net["serve_peers"].as_bool()),
        (Some(false), Some(false))
    );
    assert!(net["listen_addr"].is_null());
    let me = srv.state.identity.id;
    assert_eq!(net["peer_id"], me.to_hex());
    assert_eq!(net["short_label"], me.short_label());
    assert_eq!(
        Settings::load(&Paths::at(dir.path())).unwrap(),
        None,
        "nothing persisted yet"
    );

    let stranger = DeviceIdentity::generate().unwrap().id;
    let body = chat_body(MODEL_ID, 4, json!({ "target": format!("peer:{}", stranger.to_hex()) }));
    let (status, v) = post(&srv, "/v1/chat/completions", body).await;
    assert_eq!(status, 403, "{v}");
    assert_eq!(v["error"]["code"], "policy_denied");
    assert!(v["error"]["message"].as_str().unwrap().contains("local"), "{v}");
    // The header form, and the short-label form of an unknown device.
    let r = client()
        .post(srv.url("/v1/completions"))
        .header("x-common-target", format!("peer:{}", stranger.to_hex()))
        .json(&json!({ "model": MODEL_ID, "prompt": "x", "max_tokens": 1 }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    assert_eq!(r.json::<Value>().await.unwrap()["error"]["code"], "policy_denied");
    let (status, v) = post(
        &srv,
        "/v1/chat/completions",
        chat_body(MODEL_ID, 4, json!({ "target": "peer:1a2b-3c4d-5e6f" })),
    )
    .await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("peer_unauthorized")),
        "{v}"
    );
    for bad in ["bogus", "peer:", "peer:zz", "remote"] {
        let (status, v) = post(
            &srv,
            "/v1/chat/completions",
            chat_body(MODEL_ID, 4, json!({ "target": bad })),
        )
        .await;
        assert_eq!(
            (status, v["error"]["code"].as_str()),
            (400, Some("invalid_request")),
            "{bad}: {v}"
        );
    }
    let r = client()
        .post(srv.url("/v1/embeddings"))
        .header("x-common-target", format!("peer:{}", stranger.to_hex()))
        .json(&json!({ "model": MODEL_ID, "input": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400, "embeddings never run remotely");

    assert!(
        srv.state.net.lock().await.node.is_none(),
        "no Node exists while networking is off"
    );
    assert_no_network_activity(&srv).await;
    assert!(get(&srv, "/api/peers").await["peers"].as_array().unwrap().is_empty());
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn peer_target_is_denied_before_any_connection_when_network_is_enabled() {
    let (srv, dir) = empty_server().await;
    enable_network(&srv, false).await;
    assert!(srv.state.net.lock().await.node.is_some());
    let paths = Paths::at(dir.path());
    assert!(Settings::load(&paths).unwrap().unwrap().network_enabled, "persisted");

    let stranger = DeviceIdentity::generate().unwrap().id;
    let target = json!({ "target": format!("peer:{}", stranger.to_hex()) });
    let (status, v) = post(&srv, "/v1/chat/completions", chat_body(MODEL_ID, 4, target.clone())).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("policy_denied")),
        "still local_only: {v}"
    );

    // The disclosure must be acknowledged before leaving local_only.
    let (status, v) = put(&srv, "/api/privacy", json!({ "mode": "trusted_group" })).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("policy_denied")),
        "{v}"
    );
    assert_eq!(v["error"]["message"], DISCLOSURE);
    assert!(v["error"]["hint"].as_str().unwrap().contains("acknowledged"));
    assert_eq!(get(&srv, "/api/privacy").await["mode"], "local_only");
    let (status, v) = put(
        &srv,
        "/api/privacy",
        json!({ "mode": "swarm_allowed", "acknowledged": true }),
    )
    .await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("policy_denied")),
        "swarm is gated: {v}"
    );
    let (status, v) = put(
        &srv,
        "/api/privacy",
        json!({ "mode": "trusted_group", "acknowledged": true }),
    )
    .await;
    assert_eq!((status, v["mode"].as_str()), (200, Some("trusted_group")), "{v}");
    assert_eq!(
        Settings::load(&paths).unwrap().unwrap().privacy,
        common_policy::PrivacyMode::TrustedGroup
    );

    // trusted_group, but the device is not paired: refused before any connect.
    let (status, v) = post(&srv, "/v1/chat/completions", chat_body(MODEL_ID, 4, target.clone())).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("peer_unauthorized")),
        "{v}"
    );
    // Paired (pinned by hand, with an address nothing listens on) but in no group: still refused
    // by the policy, before the request path dials anything (a dial would end in peer_lost after
    // the handshake timeout, not in this 403).
    srv.state
        .peers
        .pin(PeerRecord {
            id: stranger,
            name: "stranger".into(),
            paired_at_utc: common_identity::utc_now(),
            last_seen_utc: None,
            last_addr: Some("127.0.0.1:1".into()),
        })
        .unwrap();
    let (status, v) = post(&srv, "/v1/chat/completions", chat_body(MODEL_ID, 4, target.clone())).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("peer_unauthorized")),
        "{v}"
    );
    let (status, v) = put(
        &srv,
        "/api/groups",
        json!({ "name": "home", "members": [stranger.to_hex()], "policy": { "allow_pooled_inference": true } }),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["groups"][0]["name"], "home");
    let (status, v) = post(&srv, "/v1/chat/completions", chat_body(MODEL_ID, 4, target)).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("peer_unauthorized")),
        "wrong kind: {v}"
    );
    assert_no_remote_request(&srv).await;
    let kinds = audit_kinds(&get(&srv, "/api/audit").await);
    for k in ["network_changed", "privacy_changed", "group_updated"] {
        assert!(kinds.contains(&k.to_string()), "{kinds:?}");
    }

    let (status, v) = put(&srv, "/api/network", json!({ "enabled": false })).await;
    assert_eq!((status, v["enabled"].as_bool()), (200, Some(false)), "{v}");
    assert!(srv.state.net.lock().await.node.is_none());
    assert!(!Settings::load(&paths).unwrap().unwrap().network_enabled);
    let (status, v) = post(&srv, "/api/pair/start", json!({})).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("policy_denied")),
        "no pairing while off"
    );
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pairing_pins_both_sides_and_revoke_closes_the_connection_single_host() {
    let (a, _da) = empty_server().await;
    let (b, _db) = empty_server().await;
    enable_network(&a, true).await;
    enable_network(&b, true).await;

    // A wrong code pairs nothing and leaves the window open (up to 5 failures).
    let (status, start) = post(&b, "/api/pair/start", json!({ "name": "B" })).await;
    assert_eq!(status, 200, "{start}");
    let addr = format!("127.0.0.1:{}", start["port"]);
    let wrong = if start["code"] == "000000" { "111111" } else { "000000" };
    let (status, v) = post(
        &a,
        "/api/pair/join",
        json!({ "addr": addr, "code": wrong, "name": "A" }),
    )
    .await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (401, Some("pairing_failed")),
        "{v}"
    );
    assert!(get(&a, "/api/peers").await["peers"].as_array().unwrap().is_empty());
    assert!(get(&b, "/api/peers").await["peers"].as_array().unwrap().is_empty());
    let (status, v) = post(&a, "/api/pair/join", json!({ "code": "123456" })).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (400, Some("invalid_request")),
        "addr required: {v}"
    );
    assert_eq!(get(&b, "/api/pair/status").await["active"], true);

    // The right code, on the same window.
    let (status, v) = post(
        &a,
        "/api/pair/join",
        json!({ "addr": addr, "code": start["code"], "name": "A" }),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let (ida, idb) = (a.state.identity.id, b.state.identity.id);
    assert_eq!(v["id"], idb.to_hex());
    let status = wait_for("B pairing result", || async {
        let v = get(&b, "/api/pair/status").await;
        (v["active"] == false).then_some(v)
    })
    .await;
    assert_eq!(status["result"]["peer"]["id"], ida.to_hex());
    let peers_a = get(&a, "/api/peers").await;
    assert_eq!(peers_a["peers"][0]["id"], idb.to_hex());
    assert_eq!(peers_a["peers"][0]["short_label"], idb.short_label());
    assert_eq!(peers_a["peers"][0]["name"], "B");
    assert_eq!(get(&b, "/api/peers").await["peers"][0]["id"], ida.to_hex());

    // An authenticated workload connection A -> B, visible on both sides.
    let na = a.state.node().await.unwrap();
    let conn = a.state.peer_connection(&na, idb).await.unwrap();
    assert_eq!(conn.peer_id, idb);
    let nb = b.state.node().await.unwrap();
    wait_for("B sees A connected", || async {
        (nb.connections().iter().any(|c| c.peer_id == ida)).then_some(())
    })
    .await;
    assert_eq!(get(&a, "/api/peers").await["peers"][0]["connected"], true);
    // One per direction at most: each side's sync loop may have dialled the other first.
    let n = get(&a, "/api/network").await["connections"].as_u64().unwrap();
    assert!((1..=2).contains(&n), "{n}");
    // A dialled B, so B's address and last contact are on record.
    let rec = a.state.peers.get(&idb).unwrap().unwrap();
    assert!(rec.last_seen_utc.is_some(), "{rec:?}");

    // Revoke by short label: unpinned, signed revocation recorded, connection closed everywhere.
    let (status, rev) = post(&a, &format!("/api/peers/{}/revoke", idb.short_label()), json!({})).await;
    assert_eq!(status, 200, "{rev}");
    assert_eq!(rev["peer_id"], idb.to_hex());
    assert_eq!(rev["revoked_by"], ida.to_hex());
    wait_for("connection closed on both sides", || async {
        (conn.conn.close_reason().is_some() && nb.connections().is_empty() && na.connections().is_empty()).then_some(())
    })
    .await;
    let peers_a = get(&a, "/api/peers").await;
    assert!(peers_a["peers"].as_array().unwrap().is_empty());
    assert_eq!(peers_a["revocations"][0]["peer_id"], idb.to_hex());
    // B is still pinned to A, but A's verifier refuses the revoked key inside the handshake.
    let e = b.state.peer_connection(&nb, ida).await.unwrap_err();
    assert_eq!(e.code, common_core::ErrorCode::PeerRevoked, "{e}");
    let (status, v) = post(
        &a,
        "/v1/chat/completions",
        chat_body(MODEL_ID, 4, json!({ "target": format!("peer:{}", idb.to_hex()) })),
    )
    .await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("policy_denied")),
        "local_only on A: {v}"
    );
    let kinds = audit_kinds(&get(&a, "/api/audit").await);
    for k in [
        "pairing_succeeded",
        "peer_connected",
        "peer_revoked",
        "peer_disconnected",
    ] {
        assert!(kinds.contains(&k.to_string()), "A audit {kinds:?}");
    }
    let kinds = audit_kinds(&get(&b, "/api/audit").await);
    for k in [
        "pairing_started",
        "pairing_failed",
        "pairing_succeeded",
        "peer_connected",
    ] {
        assert!(kinds.contains(&k.to_string()), "B audit {kinds:?}");
    }
    // Unpair without revoking.
    let (status, _) = send(
        &b,
        reqwest::Method::DELETE,
        &format!("/api/peers/{}", ida.to_hex()),
        Value::Null,
    )
    .await;
    assert_eq!(status, 204);
    assert!(get(&b, "/api/peers").await["peers"].as_array().unwrap().is_empty());
    a.stop().await;
    b.stop().await;
}

/// **SINGLE-HOST**: A (nothing downloaded, never loads) runs the golden prompt on B (real engine,
/// fixture model) through `target`, over the authenticated transport, with the response saying so.
#[tokio::test(flavor = "multi_thread")]
async fn remote_completion_returns_golden_text_with_target_echoed_single_host() {
    let Some(home) = fixture_home() else {
        return;
    };
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = Server::start(app(cfg(), da.path().to_path_buf(), HardwareReport::default())).await;
    // B's identity, peers, audit and settings live in its temp dir; its models and engine are the
    // real COMMON_HOME (never written to).
    let real = Paths::at(&home);
    let backend = common_runtime::backend_by_name("llama.cpp", engine_dir(&real), real.clone()).unwrap();
    let manager = ModelManager::new(
        real.clone(),
        Store::new(real),
        backend,
        common_hardware::probe().unwrap(),
    );
    let b = Server::start(app_with(cfg(), Paths::at(db.path()), Arc::new(manager))).await;
    enable_network(&a, false).await;
    enable_network(&b, true).await;
    let (idb, ida) = pair(&b, &a, "B", "A").await;
    for (srv, member) in [(&a, idb), (&b, ida)] {
        let (status, v) = put(
            srv,
            "/api/groups",
            json!({ "name": "home", "members": [member.to_hex()], "policy": { "allow_remote_inference": true } }),
        )
        .await;
        assert_eq!(status, 200, "{v}");
    }
    let (status, _) = put(
        &a,
        "/api/privacy",
        json!({ "mode": "trusted_group", "acknowledged": true }),
    )
    .await;
    assert_eq!(status, 200);
    let target = format!("peer:{}", idb.to_hex());

    let body = json!({ "model": MODEL_ID, "prompt": PROMPT, "max_tokens": 32, "temperature": 0, "seed": 42, "target": format!("peer:{}", idb.short_label()) });
    let t0 = Instant::now();
    let (status, v) = post(&a, "/v1/completions", body.clone()).await;
    eprintln!("SINGLE-HOST remote completion: {:?}", t0.elapsed());
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["choices"][0]["text"], GOLDEN);
    assert_eq!(v["choices"][0]["finish_reason"], "length");
    assert_eq!(v["target"], target);
    assert_eq!(
        v["usage"],
        json!({"prompt_tokens": 10, "completion_tokens": 32, "total_tokens": 42})
    );
    assert!(v["timings"]["time_to_first_token_ms"].as_f64().unwrap() > 0.0, "{v}");
    assert!(a.state.manager.loaded.lock().await.is_none(), "A never loaded a model");

    // Streamed, with the header form: the same text, every chunk labelled.
    let r = client()
        .post(a.url("/v1/completions"))
        .header("x-common-target", &target)
        .json(&json!({ "model": MODEL_ID, "prompt": PROMPT, "max_tokens": 32, "temperature": 0, "seed": 42, "stream": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/event-stream"));
    let data = sse_data(&r.text().await.unwrap());
    assert_eq!(data.last().map(String::as_str), Some("[DONE]"));
    let mut text = String::new();
    for d in &data[..data.len() - 1] {
        let v: Value = serde_json::from_str(d).unwrap();
        assert_eq!(v["target"], target, "{v}");
        text.push_str(v["choices"][0]["text"].as_str().unwrap_or(""));
    }
    assert_eq!(text, GOLDEN);

    // Chat on a peer: the peer's engine prompts the template-less model, so only the envelope is
    // asserted (the plain-prompt fallback needs the GGUF header, which A does not have).
    let (status, v) = post(
        &a,
        "/v1/chat/completions",
        chat_body(MODEL_ID, 8, json!({ "target": target })),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["target"], target);
    assert!(
        v["choices"][0]["message"]["content"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "{v}"
    );

    let metrics = client()
        .get(a.url("/metrics"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(
        metric(
            &metrics,
            &format!("common_remote_requests_total{{peer=\"{}\"}}", idb.short_label())
        ),
        Some(3.0),
        "{metrics}"
    );
    // Tokens B decoded are not A's generation: A's own counters stay at zero.
    assert_eq!(metric(&metrics, "common_tokens_generated_total"), Some(0.0));
    for (srv, role) in [(&a, "client"), (&b, "server")] {
        let audit = get(srv, "/api/audit").await;
        let done = audit["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["kind"] == "remote_request" && e["fields"]["outcome"] == "done")
            .count();
        assert_eq!(done, 3, "{role}: {audit}");
    }
    for dir in [da.path(), db.path()] {
        let text = std::fs::read_to_string(dir.join("logs/audit.jsonl")).unwrap();
        assert!(
            !text.contains(PROMPT) && !text.contains(GOLDEN) && !text.contains("Benny"),
            "{text}"
        );
    }

    // F-06-10: a streaming client that goes away mid-answer still leaves A's entry ("cancelled").
    let mut r = client()
        .post(a.url("/v1/completions"))
        .json(&json!({ "model": MODEL_ID, "prompt": PROMPT, "max_tokens": 200, "stream": true, "target": target }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.chunk().await.unwrap().is_some(), "first chunk");
    drop(r);
    let client_entries = |v: &Value, phase: &str| -> Vec<Value> {
        v["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["kind"] == "remote_request" && e["fields"]["phase"] == phase)
            .map(|e| e["fields"].clone())
            .collect()
    };
    let finished = wait_for("A audits the abandoned request", || async {
        let v = get(&a, "/api/audit?n=1000").await;
        let f = client_entries(&v, "finished");
        (f.len() == 4).then_some(f)
    })
    .await;
    assert_eq!(finished[3]["outcome"], "cancelled", "{finished:?}");
    let sent = client_entries(&get(&a, "/api/audit?n=1000").await, "sent");
    assert_eq!(sent.len(), 4, "every request was audited before it left: {sent:?}");

    // Back to local_only: the same request is refused before anything leaves, nothing new on B.
    let b_requests = || async {
        audit_kinds(&get(&b, "/api/audit?n=1000").await)
            .iter()
            .filter(|k| *k == "remote_request")
            .count()
    };
    let before = b_requests().await;
    let (status, _) = put(&a, "/api/privacy", json!({ "mode": "local_only" })).await;
    assert_eq!(status, 200);
    let (status, v) = post(&a, "/v1/completions", body).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("policy_denied")),
        "{v}"
    );
    assert_eq!(b_requests().await, before);
    a.stop().await;
    b.stop().await;
}

/// `common privacy local-only` writes config.json from another process while the daemon runs: the
/// next request is evaluated as local_only, and a later desktop toggle does not write the old
/// mode back. An unparsable file fails closed.
#[tokio::test(flavor = "multi_thread")]
async fn privacy_changed_on_disk_applies_at_once_and_is_never_written_back() {
    let (srv, dir) = empty_server().await;
    let paths = Paths::at(dir.path());
    enable_network(&srv, false).await;
    let (status, _) = put(
        &srv,
        "/api/privacy",
        json!({ "mode": "trusted_group", "acknowledged": true }),
    )
    .await;
    assert_eq!(status, 200);
    let stranger = DeviceIdentity::generate().unwrap().id;
    let target = json!({ "target": format!("peer:{}", stranger.to_hex()) });
    let (_, v) = post(&srv, "/v1/chat/completions", chat_body(MODEL_ID, 4, target.clone())).await;
    assert_eq!(
        v["error"]["code"], "peer_unauthorized",
        "trusted_group is in effect: {v}"
    );

    // What the CLI does: read the file, change the mode, save it.
    let on_disk = Settings::load(&paths).unwrap().unwrap();
    Settings {
        privacy: common_policy::PrivacyMode::LocalOnly,
        ..on_disk
    }
    .save(&paths)
    .unwrap();
    assert_eq!(get(&srv, "/api/privacy").await["mode"], "local_only");
    let (status, v) = post(&srv, "/v1/chat/completions", chat_body(MODEL_ID, 4, target.clone())).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("policy_denied")),
        "local_only from disk: {v}"
    );

    // The desktop's network toggle is a read-modify-write: the CLI's choice survives it.
    let (status, v) = put(&srv, "/api/network", json!({ "serve_peers": true })).await;
    assert_eq!((status, v["serve_peers"].as_bool()), (200, Some(true)), "{v}");
    let s = Settings::load(&paths).unwrap().unwrap();
    assert_eq!(
        (s.privacy, s.network_enabled, s.serve_peers),
        (common_policy::PrivacyMode::LocalOnly, true, true)
    );

    // A file that no longer parses: local_only until it is fixed.
    let (status, _) = put(
        &srv,
        "/api/privacy",
        json!({ "mode": "trusted_group", "acknowledged": true }),
    )
    .await;
    assert_eq!(status, 200);
    std::fs::write(Settings::path(&paths), "{nope").unwrap();
    assert_eq!(get(&srv, "/api/privacy").await["mode"], "local_only");
    let (_, v) = post(&srv, "/v1/chat/completions", chat_body(MODEL_ID, 4, target)).await;
    assert_eq!(v["error"]["code"], "policy_denied", "{v}");
    srv.stop().await;
}

/// The pairing window's socket closes with networking: nothing listens, nothing can be pinned,
/// and the next window opens without a stale 409.
#[tokio::test(flavor = "multi_thread")]
async fn turning_networking_off_closes_an_open_pairing_window() {
    let (srv, _dir) = empty_server().await;
    enable_network(&srv, false).await;
    let (status, start) = post(&srv, "/api/pair/start", json!({})).await;
    assert_eq!(status, 200, "{start}");
    let port = start["port"].as_u64().unwrap() as u16;
    assert!(
        std::net::UdpSocket::bind(("127.0.0.1", port)).is_err(),
        "the pairing endpoint holds its port while the window is open"
    );
    let (status, v) = put(&srv, "/api/network", json!({ "enabled": false })).await;
    assert_eq!((status, v["enabled"].as_bool()), (200, Some(false)), "{v}");
    let status = get(&srv, "/api/pair/status").await;
    assert_eq!(status["active"], false, "{status}");
    assert_eq!(status["result"]["error"]["code"], "cancelled", "{status}");
    wait_for("pairing port released", || async {
        std::net::UdpSocket::bind(("127.0.0.1", port)).ok().map(|_| ())
    })
    .await;
    let events = get(&srv, "/api/audit").await;
    assert!(
        events["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "pairing_failed" && e["fields"]["reason"] == "cancelled"),
        "{events}"
    );
    enable_network(&srv, false).await;
    let (status, v) = post(&srv, "/api/pair/start", json!({})).await;
    assert_eq!(status, 200, "no stale window: {v}");
    srv.stop().await;
}

/// The desktop's paste-the-QR path (no address typed) pairs; a QR naming another device pins
/// nothing in the end and keeps an earlier revocation of the device that answered.
#[tokio::test(flavor = "multi_thread")]
async fn qr_join_pairs_and_a_qr_for_another_device_is_rolled_back_single_host() {
    let (a, _da) = empty_server().await;
    let (b, _db) = empty_server().await;
    enable_network(&a, false).await;
    enable_network(&b, false).await;
    let (ida, idb) = (a.state.identity.id, b.state.identity.id);
    let (status, _) = post(&a, &format!("/api/peers/{}/revoke", idb.to_hex()), json!({})).await;
    assert_eq!(status, 200);

    let (status, start) = post(&b, "/api/pair/start", json!({ "name": "B" })).await;
    assert_eq!(status, 200, "{start}");
    let stranger = DeviceIdentity::generate().unwrap().id;
    let forged = start["qr_payload"]
        .as_str()
        .unwrap()
        .replace(&idb.to_hex(), &stranger.to_hex());
    let (status, v) = post(&a, "/api/pair/join", json!({ "code": forged, "name": "A" })).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (401, Some("pairing_failed")),
        "{v}"
    );
    assert!(!a.state.peers.is_pinned(&idb), "B is not pinned on A");
    assert!(a.state.peers.is_revoked(&idb), "B stays revoked on A");
    let kinds = get(&a, "/api/audit").await;
    assert!(
        kinds["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "pairing_failed" && e["fields"]["reason"] == "qr_peer_mismatch"),
        "{kinds}"
    );

    // The genuine QR alone, on a new window: both pinned (a re-pair lifts A's revocation of B).
    wait_for("first window closed", || async {
        (get(&b, "/api/pair/status").await["active"] == false).then_some(())
    })
    .await;
    let (status, start) = post(&b, "/api/pair/start", json!({ "name": "B" })).await;
    assert_eq!(status, 200, "{start}");
    let (status, rec) = post(
        &a,
        "/api/pair/join",
        json!({ "code": start["qr_payload"], "name": "A" }),
    )
    .await;
    assert_eq!(
        (status, rec["id"].as_str()),
        (200, Some(idb.to_hex().as_str())),
        "{rec}"
    );
    assert!(a.state.peers.is_pinned(&idb) && !a.state.peers.is_revoked(&idb));
    wait_for("B pins A", || async { b.state.peers.is_pinned(&ida).then_some(()) }).await;
    a.stop().await;
    b.stop().await;
}

/// F-06-05 across a group: A, B and C paired and in one group; B revokes A. C learns it over the
/// B-C connection the sync loop keeps, closes C-A within 5 s and refuses A; the revocation
/// survives C's restart; and after C re-pairs A, B's replay of the old revocation does not undo it.
#[tokio::test(flavor = "multi_thread")]
async fn revocation_reaches_a_third_device_and_a_re_pair_survives_replay_single_host() {
    let (a, _da) = empty_server().await;
    let (b, _db) = empty_server().await;
    let dc = tempfile::tempdir().unwrap();
    let start_c = || Server::start(app(cfg(), dc.path().to_path_buf(), HardwareReport::default()));
    let c = start_c().await;
    for srv in [&a, &b, &c] {
        enable_network(srv, true).await;
    }
    let (ida, idb, idc) = (a.state.identity.id, b.state.identity.id, c.state.identity.id);
    pair(&a, &b, "A", "B").await;
    pair(&a, &c, "A", "C").await;
    pair(&b, &c, "B", "C").await;
    let members = json!([ida.to_hex(), idb.to_hex(), idc.to_hex()]);
    for srv in [&a, &b, &c] {
        let (status, v) = put(
            srv,
            "/api/groups",
            json!({ "name": "home", "members": members, "policy": {} }),
        )
        .await;
        assert_eq!(status, 200, "{v}");
    }
    let nc = c.state.node().await.unwrap();
    wait_for("C connected to A and B by its sync loop", || async {
        let ids: Vec<PeerId> = nc.connections().iter().map(|x| x.peer_id).collect();
        (ids.contains(&ida) && ids.contains(&idb)).then_some(())
    })
    .await;

    let t0 = Instant::now();
    let (status, rev) = post(&b, &format!("/api/peers/{}/revoke", ida.to_hex()), json!({})).await;
    assert_eq!(status, 200, "{rev}");
    wait_for("C applies B's revocation and drops A", || async {
        let applied = c.state.peers.is_revoked(&ida) && !c.state.peers.is_pinned(&ida);
        (applied && !nc.connections().iter().any(|x| x.peer_id == ida)).then_some(())
    })
    .await;
    let took = t0.elapsed();
    eprintln!("SINGLE-HOST revocation B->C applied and C-A closed in {took:?}");
    assert!(took < Duration::from_secs(5), "{took:?}");
    let na = a.state.node().await.unwrap();
    wait_for("A sees C-A closed", || async {
        (!na.connections().iter().any(|x| x.peer_id == idc)).then_some(())
    })
    .await;
    let e = a.state.peer_connection(&na, idc).await.unwrap_err();
    assert_eq!(e.code, common_core::ErrorCode::PeerRevoked, "{e}");
    let events = get(&c, "/api/audit").await;
    assert!(
        events["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "peer_revoked" && e["fields"]["source"] == "peer"),
        "{events}"
    );

    // Persisted: a restarted C still refuses A.
    c.stop().await;
    let c = start_c().await;
    let peers_c = get(&c, "/api/peers").await;
    assert!(
        peers_c["revocations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["peer_id"] == ida.to_hex()),
        "{peers_c}"
    );
    assert!(!c.state.peers.is_pinned(&ida));

    // C pairs A again, then reconnects to B, which replays its old revocation on connect.
    pair(&c, &a, "C", "A").await;
    assert!(c.state.peers.is_pinned(&ida) && !c.state.peers.is_revoked(&ida));
    for enabled in [false, true] {
        let (status, v) = put(&c, "/api/network", json!({ "enabled": enabled })).await;
        assert_eq!(status, 200, "{v}");
    }
    let nc = c.state.node().await.unwrap();
    wait_for("C reconnected to B", || async {
        nc.connections().iter().any(|x| x.peer_id == idb).then_some(())
    })
    .await;
    // The control loop sends revocations first thing on a new connection; give it a moment.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(
        c.state.peers.is_pinned(&ida) && !c.state.peers.is_revoked(&ida),
        "the replayed revocation must not undo C's re-pair"
    );
    for srv in [a, b, c] {
        srv.stop().await;
    }
}

#[test]
fn settings_type_is_reexported_for_the_cli() {
    // The CLI and the desktop toggle share this file; the default must stay the safe one.
    let s = Settings::default();
    assert!(!s.network_enabled && !s.serve_peers);
    assert_eq!(s.privacy, common_policy::PrivacyMode::LocalOnly);
    let _ = AppState::load_options;
}
