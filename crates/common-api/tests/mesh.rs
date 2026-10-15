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

/// The tests that run a real engine take turns: they measure latency and the 10k run is long, so
/// two of them at once would only measure each other.
static ENGINE_TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

/// [`cfg`] with two engine slots: a daemon keeps one for local work, so a single-slot daemon
/// serves no peers (`PeerQuota::peer_slots`); this one lets peers use the other.
fn serving_cfg() -> ApiConfig {
    ApiConfig {
        max_concurrent: 2,
        ..cfg()
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

/// A daemon whose identity, peers, audit and settings live in `dir` while its models and engine
/// are the real COMMON_HOME `home` (read, never written): the serving side of a SINGLE-HOST pair.
fn serving_app(home: &std::path::Path, dir: &std::path::Path, config: ApiConfig) -> Arc<AppState> {
    let real = Paths::at(home);
    let backend = common_runtime::backend_by_name("llama.cpp", engine_dir(&real), real.clone()).unwrap();
    let manager = ModelManager::new(
        real.clone(),
        Store::new(real),
        backend,
        common_hardware::probe().unwrap(),
    );
    app_with(config, Paths::at(dir), Arc::new(manager))
}

/// Pin `id` by hand (pairing has its own tests): `addr` is where this device dials it; `None`
/// means this device never dials it and only accepts its connections.
fn pin(srv: &Server, id: PeerId, name: &str, addr: Option<String>) {
    srv.state
        .peers
        .pin(PeerRecord {
            id,
            name: name.into(),
            paired_at_utc: common_identity::utc_now(),
            last_seen_utc: None,
            last_addr: addr,
        })
        .unwrap();
}

async fn put_group(srv: &Server, name: &str, members: &[PeerId], remote: bool, pooled: bool) {
    let members: Vec<String> = members.iter().map(PeerId::to_hex).collect();
    let (status, v) = put(
        srv,
        "/api/groups",
        json!({ "name": name, "members": members, "policy": { "allow_remote_inference": remote, "allow_pooled_inference": pooled } }),
    )
    .await;
    assert_eq!(status, 200, "{v}");
}

fn listen_addr(v: &Value) -> String {
    v["listen_addr"].as_str().unwrap().to_string()
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
    assert_eq!(get(&a, "/api/peers").await["peers"][0]["health"], "connected");
    wait_for("B reports A healthy", || async {
        (get(&b, "/api/peers").await["peers"][0]["health"] == "connected").then_some(())
    })
    .await;
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
    // B still pins A; its view of A says the connection was closed on purpose.
    wait_for("B reports A disconnected", || async {
        (get(&b, "/api/peers").await["peers"][0]["health"] == "disconnected").then_some(())
    })
    .await;
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
    let _engine = ENGINE_TESTS.lock().await;
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = Server::start(app(cfg(), da.path().to_path_buf(), HardwareReport::default())).await;
    let b = Server::start(serving_app(&home, db.path(), serving_cfg())).await;
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
    // Both sides' entries join on the request id the RemoteRequest carried.
    let ids = |srv: &Server, phase: Option<&str>| {
        let st = srv.state.clone();
        let phase = phase.map(str::to_string);
        async move {
            let mut v: Vec<String> = st
                .audit
                .tail(1000)
                .unwrap()
                .into_iter()
                .filter(|e| e.kind == "remote_request" && phase.as_deref().is_none_or(|p| e.fields["phase"] == p))
                .map(|e| e.fields["request_id"].as_str().expect("request_id").to_string())
                .collect();
            v.sort();
            v
        }
    };
    // (A's `sent` entry is written before the request, and so its id, exists; `finished` has it.)
    let server_ids = ids(&b, None).await;
    assert_eq!(server_ids.len(), 3);
    assert_eq!(ids(&a, Some("finished")).await, server_ids, "sender 'finished' entries");
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

/// The desktop's paste-the-QR path (no address typed) pairs; a QR naming another device fails
/// inside the TLS handshake: nothing pinned on either side, A's earlier revocation of the device
/// that answered unchanged, the initiator's window still open, and the genuine QR then pairs on
/// that same window.
#[tokio::test(flavor = "multi_thread")]
async fn qr_join_pairs_and_a_qr_for_another_device_pins_nothing_single_host() {
    let (a, _da) = empty_server().await;
    let (b, _db) = empty_server().await;
    enable_network(&a, false).await;
    enable_network(&b, false).await;
    let (ida, idb) = (a.state.identity.id, b.state.identity.id);
    let (status, _) = post(&a, &format!("/api/peers/{}/revoke", idb.to_hex()), json!({})).await;
    assert_eq!(status, 200);
    let revocations = a.state.peers.revocations_since(0).unwrap();

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
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not the one in the QR code"),
        "{v}"
    );
    assert!(!a.state.peers.is_pinned(&idb), "B is not pinned on A");
    assert!(!b.state.peers.is_pinned(&ida), "A is not pinned on B");
    assert_eq!(
        a.state.peers.revocations_since(0).unwrap(),
        revocations,
        "A's revocation of B is untouched"
    );
    let events = get(&a, "/api/audit").await;
    let mismatch = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "pairing_failed" && e["fields"]["reason"] == "qr_peer_mismatch")
        .unwrap_or_else(|| panic!("{events}"));
    assert_eq!(mismatch["fields"]["expected"], stranger.to_hex());
    let kinds = audit_kinds(&get(&b, "/api/audit").await);
    assert!(
        !kinds.iter().any(|k| k == "pairing_failed" || k == "pairing_succeeded"),
        "the initiator counts nothing: {kinds:?}"
    );
    assert_eq!(get(&b, "/api/pair/status").await["active"], true, "window still open");

    // The genuine QR on the same window: both pinned (a re-pair lifts A's revocation of B).
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

/// ADR-0003 D7 fan-out, SINGLE-HOST: B revokes A (an id only, no daemon). C shares B's group
/// with A and gets the revocation; D is connected to B but in another group there, so it never
/// gets it although D itself would accept it (on D, A and B share a group). After C reconnects
/// (B offers the record again, at least once per connection) C still has exactly one
/// `peer_revoked` line: a held record is ignored. That B does resend is shown by
/// `revocation_refused_once_is_offered_again_on_reconnect_single_host`.
#[tokio::test(flavor = "multi_thread")]
async fn revocation_goes_only_to_members_sharing_a_group_single_host() {
    let (b, _db) = empty_server().await;
    let (c, _dc) = empty_server().await;
    let (d, _dd) = empty_server().await;
    let addr_b = listen_addr(&enable_network(&b, true).await);
    let addr_c = listen_addr(&enable_network(&c, true).await);
    let addr_d = listen_addr(&enable_network(&d, true).await);
    let ida = DeviceIdentity::generate().unwrap().id;
    let (idb, idc, idd) = (b.state.identity.id, c.state.identity.id, d.state.identity.id);
    pin(&b, ida, "A", None);
    pin(&b, idc, "C", Some(addr_c));
    pin(&b, idd, "D", Some(addr_d));
    pin(&c, idb, "B", Some(addr_b.clone()));
    pin(&d, idb, "B", Some(addr_b));
    put_group(&b, "home", &[ida, idc], false, false).await;
    put_group(&b, "work", &[idd], false, false).await;
    put_group(&c, "home", &[ida, idb], false, false).await;
    put_group(&d, "home", &[ida, idb], false, false).await;
    let nb = b.state.node().await.unwrap();
    wait_for("B connected to C and D", || async {
        let ids: Vec<PeerId> = nb.connections().iter().map(|x| x.peer_id).collect();
        (ids.contains(&idc) && ids.contains(&idd)).then_some(())
    })
    .await;

    let (status, rev) = post(&b, &format!("/api/peers/{}/revoke", ida.to_hex()), json!({})).await;
    assert_eq!(status, 200, "{rev}");
    wait_for("C applies B's revocation", || async {
        c.state.peers.is_revoked(&ida).then_some(())
    })
    .await;
    // D would apply it (B pinned there, A and B share D's group): it was never sent.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(
        d.state.peers.revocations_since(0).unwrap().is_empty(),
        "D shares no group with A on B and must not get the revocation"
    );
    assert!(
        nb.connections().iter().any(|x| x.peer_id == idd),
        "D stayed connected to B"
    );

    // C reconnects; a record C already holds changes nothing there, and D still never gets it.
    for enabled in [false, true] {
        let (status, v) = put(&c, "/api/network", json!({ "enabled": enabled })).await;
        assert_eq!(status, 200, "{v}");
    }
    let nc = c.state.node().await.unwrap();
    wait_for("C reconnected to B", || async {
        nc.connections().iter().any(|x| x.peer_id == idb).then_some(())
    })
    .await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let revoked_lines = |srv: &Server| {
        srv.state
            .audit
            .tail(1000)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "peer_revoked" && e.peer_id.as_deref() == Some(ida.to_hex().as_str()))
            .count()
    };
    assert_eq!(revoked_lines(&c), 1, "C audits the revocation once");
    assert_eq!(revoked_lines(&b), 1, "B audits its own revocation once");
    assert!(d.state.peers.revocations_since(0).unwrap().is_empty());
    for srv in [b, c, d] {
        srv.stop().await;
    }
}

/// SINGLE-HOST mDNS on this Mac's LAN interface: two daemons with discovery on. A lists B under
/// `discovered` and never itself (its own advert comes back over multicast loopback).
/// `COMMON_MDNS=0` skips it.
/// ADR-0003 D7 at-least-once, SINGLE-HOST: a revocation written to a member's control stream
/// is not taken as delivered. C first refuses B's revocation of A (on C, A and B share no group
/// yet, so `apply_revocation` rejects it); once C has that group, C's reconnect makes B offer the
/// record again and C applies it. A sender that marked the record done when it was written would
/// never resend it, and C would keep trusting A.
#[tokio::test(flavor = "multi_thread")]
async fn revocation_refused_once_is_offered_again_on_reconnect_single_host() {
    let (b, _db) = empty_server().await;
    let (c, _dc) = empty_server().await;
    let addr_b = listen_addr(&enable_network(&b, true).await);
    enable_network(&c, true).await;
    let ida = DeviceIdentity::generate().unwrap().id;
    let (idb, idc) = (b.state.identity.id, c.state.identity.id);
    pin(&b, ida, "A", None);
    pin(&b, idc, "C", None);
    pin(&c, idb, "B", Some(addr_b));
    put_group(&b, "home", &[ida, idc], false, false).await;
    put_group(&c, "home", &[idb], false, false).await;
    let nb = b.state.node().await.unwrap();
    wait_for("C connected to B", || async {
        nb.connections().iter().any(|x| x.peer_id == idc).then_some(())
    })
    .await;

    let (status, rev) = post(&b, &format!("/api/peers/{}/revoke", ida.to_hex()), json!({})).await;
    assert_eq!(status, 200, "{rev}");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(
        c.state.peers.revocations_since(0).unwrap().is_empty(),
        "C refused it: A and B share no group on C"
    );

    put_group(&c, "home", &[ida, idb], false, false).await;
    for enabled in [false, true] {
        let (status, v) = put(&c, "/api/network", json!({ "enabled": enabled })).await;
        assert_eq!(status, 200, "{v}");
    }
    wait_for("B offers the revocation again and C applies it", || async {
        c.state.peers.is_revoked(&ida).then_some(())
    })
    .await;
    eprintln!("SINGLE-HOST D7: a revocation C refused once reached it on the next connection");
    for srv in [b, c] {
        srv.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn peers_list_never_shows_this_device_as_discovered_single_host() {
    if std::env::var("COMMON_MDNS").as_deref() == Ok("0") {
        eprintln!("SKIP peers_list_never_shows_this_device_as_discovered_single_host: COMMON_MDNS=0");
        return;
    }
    let mdns = || ApiConfig {
        network: NetworkConfig {
            mdns: true,
            ..cfg().network
        },
        ..cfg()
    };
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = Server::start(app(mdns(), da.path().to_path_buf(), HardwareReport::default())).await;
    let b = Server::start(app(mdns(), db.path().to_path_buf(), HardwareReport::default())).await;
    for srv in [&a, &b] {
        let (status, v) = put(srv, "/api/network", json!({ "enabled": true })).await;
        assert_eq!((status, v["mdns"].as_bool()), (200, Some(true)), "{v}");
    }
    let (ida, idb) = (a.state.identity.id.to_hex(), b.state.identity.id.to_hex());
    let t0 = Instant::now();
    let mut seen_b: Option<Instant> = None;
    while seen_b.is_none_or(|t| t.elapsed() < Duration::from_secs(1)) {
        let v = get(&a, "/api/peers").await;
        let ids: Vec<&str> = v["discovered"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["id"].as_str().unwrap())
            .collect();
        assert!(!ids.contains(&ida.as_str()), "own id listed: {v}");
        if seen_b.is_none() && ids.contains(&idb.as_str()) {
            eprintln!(
                "SINGLE-HOST: B discovered by A after {:?}; A never listed itself",
                t0.elapsed()
            );
            seen_b = Some(Instant::now());
        }
        assert!(
            t0.elapsed() < Duration::from_secs(8),
            "SINGLE-HOST: B was never discovered: {v}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    a.stop().await;
    b.stop().await;
}

// ------------------------------------------------------------- relayed pair (F-06-07, F-10-10)

/// TEST-ONLY UDP relay in front of one QUIC endpoint (`server`), for one client endpoint: forwards
/// every datagram both ways and counts them (datagrams, bytes, largest datagram since the last
/// `window()`). It is the only path between the two daemons of [`relayed_pair`]. No such socket
/// exists in the crates' sources.
struct Relay {
    addr: std::net::SocketAddr,
    datagrams: Arc<std::sync::atomic::AtomicU64>,
    bytes: Arc<std::sync::atomic::AtomicU64>,
    largest: Arc<std::sync::atomic::AtomicU64>,
    task: tokio::task::JoinHandle<()>,
}

impl Relay {
    async fn start(server: std::net::SocketAddr) -> Relay {
        use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
        let front = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let back = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = front.local_addr().unwrap();
        let (datagrams, bytes, largest) = (
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
        );
        let (dg, by, lg) = (datagrams.clone(), bytes.clone(), largest.clone());
        let count = move |n: usize| {
            dg.fetch_add(1, Relaxed);
            by.fetch_add(n as u64, Relaxed);
            lg.fetch_max(n as u64, Relaxed);
        };
        let task = tokio::spawn(async move {
            let (mut bi, mut bo) = (vec![0u8; 65535], vec![0u8; 65535]);
            let mut client = None;
            loop {
                tokio::select! {
                    r = front.recv_from(&mut bi) => {
                        let Ok((n, from)) = r else { continue };
                        client = Some(from);
                        count(n);
                        let _ = back.send_to(&bi[..n], server).await;
                    }
                    r = back.recv_from(&mut bo) => {
                        let Ok((n, _)) = r else { continue };
                        count(n);
                        if let Some(c) = client {
                            let _ = front.send_to(&bo[..n], c).await;
                        }
                    }
                }
            }
        });
        Relay {
            addr,
            datagrams,
            bytes,
            largest,
            task,
        }
    }

    /// (datagrams so far, bytes so far, largest datagram since the previous call).
    fn window(&self) -> (u64, u64, u64) {
        use std::sync::atomic::Ordering::Relaxed;
        let largest = self.largest.swap(0, Relaxed);
        (self.datagrams.load(Relaxed), self.bytes.load(Relaxed), largest)
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The most datagrams QUIC keep-alives alone can put through the relay in `elapsed`: each side
/// sends at most one PING per keep-alive interval it stays idle (its timer restarts on every
/// ack-eliciting packet) and the other side answers each with at most one ACK-only datagram, so
/// per side and interval 2 datagrams, plus one interval for a window that straddles two timers.
fn keepalive_bound(elapsed: Duration, keep_alive: Duration) -> u64 {
    4 * (elapsed.as_millis() / keep_alive.as_millis() + 1) as u64
}

/// Largest datagram a keep-alive or its ACK can be here: short header (1 + 8-byte connection id +
/// up to 4-byte packet number), a PING and/or ACK frame and the 16-byte AEAD tag. A RemoteRequest
/// (model id, sha256, JSON request) can never fit.
const KEEPALIVE_MAX_BYTES: u64 = 100;

/// A with no models (the client: its requests can only run on a peer), B serving the real fixture,
/// trusted by hand both ways. A reaches B only at the relay's address; B pins A without an address,
/// so B never dials A and every packet between them crosses the relay. B serves A (group policy
/// allows remote inference there); what A may send is left to each test.
struct Relayed {
    a: Server,
    b: Server,
    relay: Relay,
    _dirs: (tempfile::TempDir, tempfile::TempDir),
}

async fn relayed_pair(home: &std::path::Path) -> Relayed {
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    // B's quota must not be what refuses A here (F-06-04 has its own test).
    Settings {
        peer_quota: common_api::mesh::PeerQuota {
            requests_per_minute: 0,
            ..Default::default()
        },
        ..Settings::default()
    }
    .save(&Paths::at(db.path()))
    .unwrap();
    let a = Server::start(app(cfg(), da.path().to_path_buf(), HardwareReport::default())).await;
    let b = Server::start(serving_app(home, db.path(), serving_cfg())).await;
    let b_addr: std::net::SocketAddr = listen_addr(&enable_network(&b, true).await).parse().unwrap();
    let relay = Relay::start(b_addr).await;
    let (ida, idb) = (a.state.identity.id, b.state.identity.id);
    pin(&a, idb, "B", Some(relay.addr.to_string()));
    pin(&b, ida, "A", None);
    put_group(&b, "home", &[ida], true, false).await;
    // Warm B's engine so the first allowed request does not carry a model load.
    let (status, v) = post(
        &b,
        "/v1/completions",
        json!({ "model": MODEL_ID, "prompt": PROMPT, "max_tokens": 1, "temperature": 0 }),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    Relayed {
        a,
        b,
        relay,
        _dirs: (da, db),
    }
}

/// B's audit lines that only a RemoteRequest frame from a peer can cause (served or refused).
fn server_request_entries(b: &Server) -> Vec<common_observability::AuditEvent> {
    b.state
        .audit
        .tail(1_000_000)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "remote_request" || (e.kind == "peer_rejected" && e.fields.get("request_id").is_some()))
        .collect()
}

/// A's `remote_request` entries of `phase`.
fn sender_entries(a: &Server, phase: &str) -> Vec<common_observability::AuditEvent> {
    a.state
        .audit
        .tail(1_000_000)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "remote_request" && e.fields["phase"] == phase)
        .collect()
}

fn request_ids(entries: &[common_observability::AuditEvent]) -> Vec<String> {
    let mut v: Vec<String> = entries
        .iter()
        .map(|e| e.fields["request_id"].as_str().expect("request_id").to_string())
        .collect();
    v.sort();
    v
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    sorted[((sorted.len() as f64 * p).ceil() as usize).clamp(1, sorted.len()) - 1]
}

/// F-10-10, SINGLE-HOST (two daemons in this process, real engine on B, B reachable only through
/// the counting relay): A in `local_only` with networking on and a connection to B. N requests
/// aimed at B are refused at A with `policy_denied`; B sees no RemoteRequest (no audit entry), A
/// writes no `remote_request` entry, and the relay carries keep-alives only. Leaving local_only
/// without `acknowledged` is refused with the disclosure text; with it, the next request runs on B.
#[tokio::test(flavor = "multi_thread")]
async fn privacy_mode_enforced_end_to_end_single_host() {
    // Spread over ~2.5 s so the window spans several keep-alive intervals.
    const N: usize = 400;
    const PAUSE: Duration = Duration::from_millis(5);
    let Some(home) = fixture_home() else {
        return;
    };
    let _engine = ENGINE_TESTS.lock().await;
    let Relayed { a, b, relay, _dirs } = relayed_pair(&home).await;
    let (ida, idb) = (a.state.identity.id, b.state.identity.id);
    put_group(&a, "home", &[idb], true, false).await;
    enable_network(&a, false).await;
    let na = a.state.node().await.unwrap();
    wait_for("A connected to B through the relay", || async {
        na.connections().iter().any(|c| c.peer_id == idb).then_some(())
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await; // handshake tail, MTU probes
    assert_eq!(get(&a, "/api/privacy").await["mode"], "local_only");

    let (server0, sent0) = (server_request_entries(&b).len(), sender_entries(&a, "sent").len());
    let (d0, b0, _) = relay.window();
    let t0 = Instant::now();
    let http = client();
    let body =
        json!({ "model": MODEL_ID, "prompt": PROMPT, "max_tokens": 4, "target": format!("peer:{}", idb.to_hex()) });
    for i in 0..N {
        let path = if i % 2 == 0 {
            "/v1/completions"
        } else {
            "/v1/chat/completions"
        };
        let body = if i % 2 == 0 {
            body.clone()
        } else {
            chat_body(MODEL_ID, 4, json!({ "target": format!("peer:{}", idb.short_label()) }))
        };
        let r = http.post(a.url(path)).json(&body).send().await.unwrap();
        assert_eq!(r.status(), 403);
        let v: Value = r.json().await.unwrap();
        assert_eq!(v["error"]["code"], "policy_denied", "{v}");
        tokio::time::sleep(PAUSE).await;
    }
    let elapsed = t0.elapsed();
    let (d1, b1, largest) = relay.window();
    let bound = keepalive_bound(elapsed, Duration::from_millis(cfg().network.keep_alive_ms));
    eprintln!(
        "SINGLE-HOST F-10-10: {N} peer-targeted requests in local_only in {elapsed:?}: relay {} datagrams / {} bytes \
         (largest {largest} B; keep-alive bound {bound} datagrams of <= {KEEPALIVE_MAX_BYTES} B); B audit +{}, A sent +{}",
        d1 - d0,
        b1 - b0,
        server_request_entries(&b).len() - server0,
        sender_entries(&a, "sent").len() - sent0,
    );
    assert_eq!(server_request_entries(&b).len(), server0, "no RemoteRequest reached B");
    assert_eq!(sender_entries(&a, "sent").len(), sent0, "nothing was sent");
    assert!(
        sender_entries(&a, "finished").is_empty(),
        "no request was even attempted"
    );
    assert!(
        d1 - d0 <= bound,
        "{} datagrams in {elapsed:?} > keep-alive bound {bound}",
        d1 - d0
    );
    assert!(
        largest <= KEEPALIVE_MAX_BYTES,
        "a {largest}-byte datagram is not a keep-alive"
    );
    assert!(
        na.connections().iter().any(|c| c.peer_id == idb),
        "the connection stayed up"
    );

    // Leaving local_only needs the disclosure acknowledged.
    let (status, v) = put(&a, "/api/privacy", json!({ "mode": "trusted_group" })).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("policy_denied")),
        "{v}"
    );
    assert_eq!(v["error"]["message"], DISCLOSURE);
    assert_eq!(get(&a, "/api/privacy").await["mode"], "local_only");
    let r = http.post(a.url("/v1/completions")).json(&body).send().await.unwrap();
    assert_eq!(r.status(), 403, "still local_only after the refused change");
    assert_eq!(server_request_entries(&b).len(), server0);

    let (status, v) = put(
        &a,
        "/api/privacy",
        json!({ "mode": "trusted_group", "acknowledged": true }),
    )
    .await;
    assert_eq!((status, v["mode"].as_str()), (200, Some("trusted_group")), "{v}");
    let (d2, _, _) = relay.window();
    let r = http.post(a.url("/v1/completions")).json(&body).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["target"], format!("peer:{}", idb.to_hex()), "{v}");
    assert!(v["choices"][0]["text"].as_str().is_some_and(|t| !t.is_empty()), "{v}");
    let served = wait_for("B audits the request", || async {
        let e = server_request_entries(&b);
        (e.len() == server0 + 1).then_some(e)
    })
    .await;
    assert_eq!(sender_entries(&a, "sent").len(), sent0 + 1);
    assert_eq!(
        request_ids(&served[server0..]),
        request_ids(&sender_entries(&a, "finished"))
    );
    assert_eq!(served[server0].peer_id.as_deref(), Some(ida.to_hex().as_str()));
    let (d3, _, _) = relay.window();
    eprintln!(
        "SINGLE-HOST F-10-10: the acknowledged request ran on B: {} datagrams through the relay",
        d3 - d2
    );

    // F-06-10: when A cannot write its `sent` entry the request is refused (`io`) before any byte
    // of it leaves: B never sees it and the relay carries keep-alives only.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let log = a.state.audit.path.clone();
        tokio::time::sleep(Duration::from_millis(300)).await; // the previous request's tail
        let (d4, _, _) = relay.window();
        let t4 = Instant::now();
        std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o400)).unwrap();
        let r = http.post(a.url("/v1/completions")).json(&body).send().await.unwrap();
        let status = r.status().as_u16();
        let v: Value = r.json().await.unwrap();
        std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!((status, v["error"]["code"].as_str()), (500, Some("io")), "{v}");
        assert!(
            v["error"]["message"].as_str().unwrap().contains("request not sent"),
            "{v}"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
        let (d5, _, largest) = relay.window();
        let bound = keepalive_bound(t4.elapsed(), Duration::from_millis(cfg().network.keep_alive_ms));
        eprintln!(
            "SINGLE-HOST F-06-10: request with A's audit log unwritable: relay {} datagrams (largest {largest} B; bound {bound})",
            d5 - d4
        );
        assert_eq!(
            server_request_entries(&b).len(),
            server0 + 1,
            "the refused request never reached B"
        );
        assert_eq!(
            sender_entries(&a, "sent").len(),
            sent0 + 1,
            "the refused request left no entry"
        );
        assert_eq!(sender_entries(&a, "finished").len(), 1, "and no outcome");
        assert!(
            largest <= KEEPALIVE_MAX_BYTES,
            "a {largest}-byte datagram left A: the request was sent"
        );
    }
    a.stop().await;
    b.stop().await;
}

/// splitmix64: a deterministic, seedable PRNG without a dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn peer_id(&mut self) -> PeerId {
        let mut b = [0u8; 32];
        for chunk in b.chunks_mut(8) {
            chunk.copy_from_slice(&self.next().to_le_bytes());
        }
        PeerId(b)
    }
}

/// A's group policy for B in one epoch of the F-06-07 run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GroupPolicy {
    Remote,
    PooledOnly,
    NoGroup,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
enum Target {
    Local,
    B,
    Unknown,
    Revoked,
    Unpaired,
}

/// The routing rule, written out from the policy's truth table (not by calling it): `None` when a
/// request aimed at a peer may leave this device, else the typed code it must be refused with.
fn policy_model(
    privacy: common_policy::PrivacyMode,
    network: bool,
    group: GroupPolicy,
    t: Target,
) -> Option<&'static str> {
    use common_policy::PrivacyMode::*;
    match (privacy, network, t) {
        (_, _, Target::Local) => unreachable!("local requests are not peer-targeted"),
        (LocalOnly | SwarmAllowed, _, _) | (TrustedGroup, false, _) => Some("policy_denied"),
        (TrustedGroup, true, Target::Revoked) => Some("peer_revoked"),
        (TrustedGroup, true, Target::Unknown | Target::Unpaired) => Some("peer_unauthorized"),
        (TrustedGroup, true, Target::B) if group == GroupPolicy::Remote => None,
        (TrustedGroup, true, Target::B) => Some("peer_unauthorized"),
    }
}

/// F-06-07, SINGLE-HOST: 10,000 requests through A's HTTP API with randomized (privacy mode,
/// target local / B / unknown / revoked / unpaired, A's group policy for B, A's networking on/off)
/// from a seeded PRNG (`COMMON_TEST_SEED` replays a run). Settings change per epoch of 20-80
/// requests; targets, endpoint and body/header form per request. B (real engine) is reachable only
/// through the counting relay. Asserted: (a) every request the policy model denies gets exactly
/// the typed error, and causes no RemoteRequest at B (B's audit unchanged) and no sender `sent`
/// entry; (b) every allowed one (max_tokens 1) produces exactly one A `sent` entry and one B entry
/// with the same request id; (c) in every window that held only denied requests the relay carried
/// no more datagrams than keep-alives can explain ([`keepalive_bound`]) and none larger than a
/// keep-alive ([`KEEPALIVE_MAX_BYTES`]); (d) `AppState::policy()` + `evaluate` (what a request runs
/// first) has p99 < 1 ms on the deny path, timed in-process for every denied peer request.
#[tokio::test(flavor = "multi_thread")]
async fn local_only_never_leaves_device_10k_single_host() {
    use common_policy::{ExecutionTarget, PrivacyMode, WorkloadKind};
    const TOTAL: usize = 10_000;
    // Paced so the denied-only windows add up to tens of seconds of real keep-alive traffic.
    const PACE: Duration = Duration::from_millis(2);
    let Some(home) = fixture_home() else {
        return;
    };
    let _engine = ENGINE_TESTS.lock().await;
    let seed = std::env::var("COMMON_TEST_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0x0C0A_F067_u64);
    eprintln!("SINGLE-HOST F-06-07: seed {seed} (COMMON_TEST_SEED={seed} replays this run)");
    let mut rng = Rng(seed);
    let started = Instant::now();
    let Relayed { a, b, relay, _dirs } = relayed_pair(&home).await;
    let idb = b.state.identity.id;
    let keep_alive = Duration::from_millis(cfg().network.keep_alive_ms);
    let paths_a = a.state.paths.clone();
    // A revoked one device and unpaired another.
    let (revoked, unpaired) = (rng.peer_id(), rng.peer_id());
    pin(&a, revoked, "revoked", Some("127.0.0.1:9".into()));
    pin(&a, unpaired, "unpaired", Some("127.0.0.1:9".into()));
    let (status, _) = post(&a, &format!("/api/peers/{}/revoke", revoked.to_hex()), json!({})).await;
    assert_eq!(status, 200);
    let (status, _) = send(
        &a,
        reqwest::Method::DELETE,
        &format!("/api/peers/{}", unpaired.to_hex()),
        Value::Null,
    )
    .await;
    assert_eq!(status, 204);

    let http = client();
    let mut current: Option<(PrivacyMode, bool, GroupPolicy)> = None;
    let mut sent = 0usize;
    let mut by_outcome: std::collections::BTreeMap<String, usize> = Default::default();
    let mut policy_times: Vec<Duration> = vec![];
    let mut deny_http: Vec<Duration> = vec![];
    let (mut epochs, mut denied_windows, mut allowed_total) = (0usize, 0usize, 0usize);
    let (mut denied_window_time, mut denied_window_dgrams, mut denied_window_bound, mut largest_denied) =
        (Duration::ZERO, 0u64, 0u64, 0u64);
    // Denied-only window time with A's endpoint up (keep-alives can flow), and relay datagrams
    // during the windows that held allowed requests (proof the relay is the path).
    let (mut denied_time_connected, mut allowed_window_dgrams) = (Duration::ZERO, 0u64);
    while sent < TOTAL {
        let privacy = [
            PrivacyMode::LocalOnly,
            PrivacyMode::TrustedGroup,
            PrivacyMode::SwarmAllowed,
        ][rng.below(3) as usize];
        let network = rng.below(2) == 1;
        let group = [GroupPolicy::Remote, GroupPolicy::PooledOnly, GroupPolicy::NoGroup][rng.below(3) as usize];
        let len = (20 + rng.below(61) as usize).min(TOTAL - sent);
        // Apply only what changed, through the same routes the desktop and CLI use.
        let before = current;
        if before.map(|c| c.0) != Some(privacy) {
            match privacy {
                PrivacyMode::SwarmAllowed => {
                    // Not selectable through the API (Gate F); what a hand-edited config.json does.
                    let on_disk = Settings::load(&paths_a).unwrap().unwrap_or_default();
                    Settings { privacy, ..on_disk }.save(&paths_a).unwrap();
                }
                _ => {
                    let (status, v) = put(&a, "/api/privacy", json!({ "mode": privacy, "acknowledged": true })).await;
                    assert_eq!(status, 200, "{v}");
                }
            }
            assert_eq!(get(&a, "/api/privacy").await["mode"], json!(privacy));
        }
        let toggled_on = network && before.map(|c| c.1) != Some(true);
        if before.map(|c| c.1) != Some(network) {
            let (status, v) = put(&a, "/api/network", json!({ "enabled": network })).await;
            assert_eq!((status, v["enabled"].as_bool()), (200, Some(network)), "{v}");
        }
        if before.map(|c| c.2) != Some(group) {
            let members: &[PeerId] = if group == GroupPolicy::NoGroup { &[] } else { &[idb] };
            put_group(
                &a,
                "home",
                members,
                group == GroupPolicy::Remote,
                group == GroupPolicy::PooledOnly,
            )
            .await;
        }
        current = Some((privacy, network, group));
        // Settle: with networking on, A's sync loop keeps a connection to B (control plane).
        if network {
            let na = a.state.node().await.unwrap();
            wait_for("A connected to B through the relay", || async {
                na.connections().iter().any(|c| c.peer_id == idb).then_some(())
            })
            .await;
        }
        tokio::time::sleep(Duration::from_millis(if toggled_on { 250 } else { 50 })).await;

        let (server0, sent0) = (server_request_entries(&b).len(), sender_entries(&a, "sent").len());
        let finished0 = sender_entries(&a, "finished").len();
        let (d0, _, _) = relay.window();
        let t0 = Instant::now();
        let mut allowed = 0usize;
        for _ in 0..len {
            let target = [
                Target::Local,
                Target::B,
                Target::Unknown,
                Target::Revoked,
                Target::Unpaired,
            ][rng.below(5) as usize];
            let id = match target {
                Target::Local => None,
                Target::B => Some(idb),
                Target::Unknown => Some(rng.peer_id()),
                Target::Revoked => Some(revoked),
                Target::Unpaired => Some(unpaired),
            };
            let chat = rng.below(2) == 1;
            let in_header = rng.below(2) == 1;
            let label = id.map(|id| format!("peer:{}", id.to_hex()));
            let mut body = if chat {
                chat_body(MODEL_ID, 1, json!({}))
            } else {
                json!({ "model": MODEL_ID, "prompt": PROMPT, "max_tokens": 1, "temperature": 0 })
            };
            let mut req = http.post(a.url(if chat {
                "/v1/chat/completions"
            } else {
                "/v1/completions"
            }));
            match (&label, in_header) {
                (Some(l), true) => req = req.header("x-common-target", l),
                (Some(l), false) => body["target"] = json!(l),
                (None, _) => {}
            }
            let t = Instant::now();
            let r = req.json(&body).send().await.unwrap();
            let took = t.elapsed();
            let status = r.status().as_u16();
            let v: Value = r.json().await.unwrap();
            let code = v["error"]["code"].as_str().unwrap_or("").to_string();
            sent += 1;
            tokio::time::sleep(PACE).await;
            let Some(id) = id else {
                // A has no models: a local run fails here, typed, and touches no network.
                assert_eq!((status, code.as_str()), (404, "model_not_downloaded"), "local: {v}");
                *by_outcome.entry("local:model_not_downloaded".into()).or_default() += 1;
                continue;
            };
            match policy_model(privacy, network, group, target) {
                None => {
                    assert_eq!(
                        status, 200,
                        "allowed {target:?} {privacy:?} net={network} {group:?}: {v}"
                    );
                    assert_eq!(v["target"], json!(label.unwrap()), "{v}");
                    allowed += 1;
                    *by_outcome.entry("peer:allowed".into()).or_default() += 1;
                }
                Some(want) => {
                    assert_eq!(
                        (status, code.as_str()),
                        (403, want),
                        "denied {target:?} {privacy:?} net={network} {group:?}: {v}"
                    );
                    deny_http.push(took);
                    let t = Instant::now();
                    let d =
                        a.state
                            .policy()
                            .evaluate(privacy, ExecutionTarget::Peer(id), WorkloadKind::RemoteInference);
                    policy_times.push(t.elapsed());
                    assert!(
                        matches!(&d, common_policy::Decision::Deny { code, .. } if code.as_str() == want),
                        "{d:?}"
                    );
                    *by_outcome.entry(format!("peer:{want}")).or_default() += 1;
                }
            }
        }
        let elapsed = t0.elapsed();
        let (d1, _, largest) = relay.window();
        // (a) + (b): B saw exactly the allowed requests, A audited exactly those, ids joined.
        let served = wait_for("B audits every allowed request", || async {
            let e = server_request_entries(&b);
            (e.len() >= server0 + allowed).then_some(e)
        })
        .await;
        assert_eq!(served.len(), server0 + allowed, "B saw a request the policy denied");
        assert!(
            served[server0..]
                .iter()
                .all(|e| e.kind == "remote_request" && e.fields["outcome"] == "done"),
            "{:?}",
            &served[server0..]
        );
        let sender = sender_entries(&a, "sent");
        assert_eq!(sender.len(), sent0 + allowed, "A sent something the policy denied");
        let finished = sender_entries(&a, "finished");
        assert_eq!(request_ids(&finished[finished0..]), request_ids(&served[server0..]));
        // (c): a window of denied requests only carries keep-alives at most.
        if allowed == 0 {
            let bound = keepalive_bound(elapsed, keep_alive);
            assert!(
                d1 - d0 <= bound && largest <= KEEPALIVE_MAX_BYTES,
                "epoch {epochs}: {} datagrams (largest {largest} B) in {elapsed:?}, keep-alive bound {bound}",
                d1 - d0
            );
            denied_windows += 1;
            denied_window_time += elapsed;
            if network {
                denied_time_connected += elapsed;
            }
            denied_window_dgrams += d1 - d0;
            denied_window_bound += bound;
            largest_denied = largest_denied.max(largest);
        }
        if allowed > 0 {
            allowed_window_dgrams += d1 - d0;
        }
        allowed_total += allowed;
        epochs += 1;
    }
    let total_finished = sender_entries(&a, "finished");
    assert_eq!(
        total_finished.len(),
        allowed_total,
        "one finished entry per allowed request"
    );
    policy_times.sort();
    deny_http.sort();
    let p99 = percentile(&policy_times, 0.99);
    eprintln!(
        "SINGLE-HOST F-06-07: {sent} requests in {epochs} epochs ({:?} total): {by_outcome:?}",
        started.elapsed()
    );
    eprintln!(
        "SINGLE-HOST F-06-07: allowed {allowed_total} (each: one A 'sent' + one B entry, same request_id; \
         {allowed_window_dgrams} relay datagrams in their windows); denied-only windows {denied_windows} over \
         {denied_window_time:?} ({denied_time_connected:?} of it with A's endpoint up): relay {denied_window_dgrams} \
         datagrams (keep-alive bound {denied_window_bound}), largest {largest_denied} B"
    );
    eprintln!(
        "SINGLE-HOST F-06-07: policy()+evaluate on the deny path, n={}: p50 {:?} p99 {p99:?} max {:?}; \
         denied HTTP round trip p50 {:?} p99 {:?}",
        policy_times.len(),
        percentile(&policy_times, 0.5),
        policy_times.last().unwrap(),
        percentile(&deny_http, 0.5),
        percentile(&deny_http, 0.99),
    );
    assert!(allowed_total > 0, "the run must exercise the allow path too");
    assert!(p99 < Duration::from_millis(1), "policy evaluation p99 {p99:?}");
    a.stop().await;
    b.stop().await;
}

/// F-06-04 through the daemon, SINGLE-HOST (both daemons in this process on one Mac; real engine
/// on B). B has two engine slots (`max_concurrent: 2`) and lets a peer run one generation at a
/// time (`config.json` `peer_quota.max_concurrent: 1`, also the cap across peers), so peers hold
/// at most one slot and local work keeps the other. A floods B through its own API with
/// `FLOOD_TASKS` loops of 32-token requests while a sequential local client uses B's API.
/// Asserted: every flood request either runs or gets the typed `overloaded` (429), at least one is
/// refused, and B's local p95 latency under the flood is within 20 % of its p95 without it.
/// Timing-sensitive: concurrent builds on this machine move both numbers.
#[tokio::test(flavor = "multi_thread")]
async fn peer_flood_throttled_through_daemon_single_host() {
    const LOCAL: usize = 40;
    const FLOOD_TASKS: usize = 8;
    let Some(home) = fixture_home() else {
        return;
    };
    let _engine = ENGINE_TESTS.lock().await;
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    Settings {
        peer_quota: common_api::mesh::PeerQuota {
            max_concurrent: 1,
            ..Default::default()
        },
        ..Settings::default()
    }
    .save(&Paths::at(db.path()))
    .unwrap();
    let a = Server::start(app(cfg(), da.path().to_path_buf(), HardwareReport::default())).await;
    let b = Server::start(serving_app(
        &home,
        db.path(),
        ApiConfig {
            max_concurrent: 2,
            ..cfg()
        },
    ))
    .await;
    let net_b = enable_network(&b, true).await;
    assert_eq!(net_b["peer_quota"]["max_concurrent"], 1, "{net_b}");
    assert_eq!(net_b["peer_slots"], 1, "{net_b}");
    enable_network(&a, false).await;
    let (ida, idb) = (a.state.identity.id, b.state.identity.id);
    pin(&a, idb, "B", Some(listen_addr(&net_b)));
    pin(&b, ida, "A", None);
    put_group(&a, "home", &[idb], true, false).await;
    put_group(&b, "home", &[ida], true, false).await;
    let (status, _) = put(
        &a,
        "/api/privacy",
        json!({ "mode": "trusted_group", "acknowledged": true }),
    )
    .await;
    assert_eq!(status, 200);

    let http = client();
    let local_body = json!({ "model": MODEL_ID, "prompt": PROMPT, "max_tokens": 32, "temperature": 0 });
    let local_run = |n: usize| {
        let (http, url, body) = (http.clone(), b.url("/v1/completions"), local_body.clone());
        async move {
            let mut t = Vec::with_capacity(n);
            for _ in 0..n {
                let t0 = Instant::now();
                let r = http.post(&url).json(&body).send().await.unwrap();
                assert_eq!(r.status(), 200, "local request on B");
                let v: Value = r.json().await.unwrap();
                assert_eq!(v["choices"][0]["text"], GOLDEN);
                t.push(t0.elapsed());
            }
            t.sort();
            t
        }
    };
    local_run(5).await; // load the engine (-np 2) and warm it
    let base = local_run(LOCAL).await;

    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flood: Vec<_> = (0..FLOOD_TASKS)
        .map(|_| {
            let (http, url, stop) = (http.clone(), a.url("/v1/completions"), stop.clone());
            let body = json!({ "model": MODEL_ID, "prompt": PROMPT, "max_tokens": 32, "temperature": 0, "target": format!("peer:{}", idb.to_hex()) });
            tokio::spawn(async move {
                let mut seen: std::collections::BTreeMap<String, usize> = Default::default();
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let r = http.post(&url).json(&body).send().await.unwrap();
                    let status = r.status().as_u16();
                    let v: Value = r.json().await.unwrap();
                    let key = match status {
                        200 => "ok".to_string(),
                        429 => format!("429 {}", v["error"]["code"].as_str().unwrap_or("?")),
                        // Anything unexpected keeps its message so a rare failure is diagnosable.
                        s => format!("{s} {}: {}", v["error"]["code"].as_str().unwrap_or("?"), v["error"]["message"].as_str().unwrap_or("")),
                    };
                    *seen.entry(key).or_default() += 1;
                }
                seen
            })
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let t_flood = Instant::now();
    let flooded = local_run(LOCAL).await;
    let flood_time = t_flood.elapsed();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let mut seen: std::collections::BTreeMap<String, usize> = Default::default();
    for t in flood {
        for (k, n) in t.await.unwrap() {
            *seen.entry(k).or_default() += n;
        }
    }
    let (p95_base, p95_flood) = (percentile(&base, 0.95), percentile(&flooded, 0.95));
    let ratio = p95_flood.as_secs_f64() / p95_base.as_secs_f64();
    let total: usize = seen.values().sum();
    eprintln!(
        "SINGLE-HOST F-06-04: local p50/p95 without flood {:?}/{p95_base:?}, with flood {:?}/{p95_flood:?} \
         (p95 x{ratio:.3}); flood: {total} requests from {FLOOD_TASKS} loops in ~{flood_time:?}: {seen:?}",
        percentile(&base, 0.5),
        percentile(&flooded, 0.5),
    );
    assert!(
        seen.keys().all(|k| k == "ok" || k == "429 overloaded"),
        "only successes and typed overloaded refusals: {seen:?}"
    );
    assert!(
        seen.get("429 overloaded").copied().unwrap_or(0) > 0,
        "the flood was throttled: {seen:?}"
    );
    assert!(ratio < 1.2, "local p95 degraded x{ratio:.3} under the peer flood");
    a.stop().await;
    b.stop().await;
}

/// F-06-04, SINGLE-HOST, no engine (the refusal comes before any load): peers never take the
/// engine slot local work needs. The library default (one engine slot) serves no peers at all;
/// B has two slots and the default peer quota (4), so peers may hold one. With both of B's slots
/// held by local admissions, A's request to B is refused `overloaded` at once (the shared engine
/// semaphore refuses it, never a wait in B's queue); with them free it passes that check and fails
/// on B's missing model instead.
#[tokio::test(flavor = "multi_thread")]
async fn peer_request_is_refused_while_local_work_holds_every_engine_slot_single_host() {
    let (a, _da) = empty_server().await;
    let db = tempfile::tempdir().unwrap();
    let b = Server::start(app(serving_cfg(), db.path().to_path_buf(), HardwareReport::default())).await;
    assert_eq!(
        get(&a, "/api/network").await["peer_slots"],
        0,
        "one engine slot: it stays with local work"
    );
    let net_b = enable_network(&b, true).await;
    assert_eq!(net_b["peer_quota"]["max_concurrent"], 4, "{net_b}");
    assert_eq!(net_b["peer_slots"], 1, "{net_b}");
    enable_network(&a, false).await;
    let (ida, idb) = (a.state.identity.id, b.state.identity.id);
    pin(&a, idb, "B", Some(listen_addr(&net_b)));
    pin(&b, ida, "A", None);
    put_group(&a, "home", &[idb], true, false).await;
    put_group(&b, "home", &[ida], true, false).await;
    let (status, _) = put(
        &a,
        "/api/privacy",
        json!({ "mode": "trusted_group", "acknowledged": true }),
    )
    .await;
    assert_eq!(status, 200);
    let body =
        json!({ "model": MODEL_ID, "prompt": PROMPT, "max_tokens": 4, "target": format!("peer:{}", idb.to_hex()) });

    let held = (
        b.state.admission.acquire().await.unwrap(),
        b.state.admission.acquire().await.unwrap(),
    );
    let t0 = Instant::now();
    let (status, v) = post(&a, "/v1/completions", body.clone()).await;
    let refused_in = t0.elapsed();
    eprintln!("SINGLE-HOST F-06-04: peer request with B's engine slots all local: {status} in {refused_in:?}");
    assert_eq!((status, v["error"]["code"].as_str()), (429, Some("overloaded")), "{v}");
    assert!(
        v["error"]["message"].as_str().unwrap().contains("local work"),
        "the engine slots refused it, not the peer share: {v}"
    );
    assert!(
        refused_in < Duration::from_secs(5),
        "refused, not queued: {refused_in:?}"
    );
    drop(held);
    let (_, v) = post(&a, "/v1/completions", body).await;
    assert_eq!(v["error"]["code"], "model_not_downloaded", "past the engine check: {v}");
    a.stop().await;
    b.stop().await;
}
