//! UCompute Mesh through the daemon API (F-02-01/02/05, F-06-02/05/07/10, F-10-05/10 backends).
//!
//! **SINGLE-HOST**: tests that run two or three servers run them all in this process on this one Mac
//! (`127.0.0.1`, separate temp `UCOMPUTE_HOME`s, distinct identities, ephemeral ports). Nothing here
//! satisfies a two-machine criterion. The remote-inference test runs the real pinned llama-server
//! on the serving side (skips loudly without the fixtures; `UCOMPUTE_REQUIRE_FIXTURES=1` fails
//! instead). Nothing is mocked.

mod support;

use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use support::*;
use ucompute_api::mesh::{Settings, DISCLOSURE};
use ucompute_api::{ApiConfig, AppState};
use ucompute_core::Paths;
use ucompute_hardware::HardwareReport;
use ucompute_identity::{DeviceIdentity, PeerId, PeerRecord};
use ucompute_network::NetworkConfig;

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

/// A free UDP port on loopback, released before it is returned (a fixed port is what the shipped
/// daemon uses; the ephemeral port in [`cfg`] hides rebind bugs because every start gets a new one).
fn free_udp_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Changing network settings while networking is on restarts the endpoint on the SAME fixed port.
/// Regression: the old endpoint's socket was still bound when the new one started, so the second
/// PUT failed with "Address already in use" and left networking off (found driving the desktop UI).
#[tokio::test]
async fn network_settings_change_while_enabled_rebinds_the_fixed_port_single_host() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_udp_port();
    let mut c = cfg();
    c.network.listen_port = port;
    let srv = Server::start(app(c, dir.path().to_path_buf(), HardwareReport::default())).await;
    let want = format!("127.0.0.1:{port}");
    for (i, serve_peers) in [false, true, false, true, true].into_iter().enumerate() {
        let (status, v) = put(
            &srv,
            "/api/network",
            json!({ "enabled": true, "serve_peers": serve_peers }),
        )
        .await;
        assert_eq!(status, 200, "change {i}: {v}");
        assert_eq!(v["enabled"], true, "change {i}: {v}");
        assert_eq!(v["serve_peers"], serve_peers, "change {i}: {v}");
        assert_eq!(v["listen_addr"], want.as_str(), "change {i}: {v}");
    }
    let (status, v) = put(&srv, "/api/network", json!({ "enabled": false, "serve_peers": false })).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["enabled"], false, "{v}");
    let (status, v) = put(&srv, "/api/network", json!({ "enabled": true, "serve_peers": false })).await;
    assert_eq!(status, 200, "off then on again: {v}");
    assert_eq!(v["listen_addr"], want.as_str(), "{v}");
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
        .starts_with(&format!("ucompute-pair:v1:{}:127.0.0.1:{port}:", me.to_hex())));
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

/// Pin `id` by hand (pairing has its own tests): `addr` is where this device dials it; `None`
/// means this device never dials it and only accepts its connections.
fn pin(srv: &Server, id: PeerId, name: &str, addr: Option<String>) {
    srv.state
        .peers
        .pin(PeerRecord {
            id,
            name: name.into(),
            paired_at_utc: ucompute_identity::utc_now(),
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
        .header("x-ucompute-target", format!("peer:{}", stranger.to_hex()))
        .json(&json!({ "model": MODEL_ID, "prompt": "x", "max_tokens": 1 }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    assert_eq!(r.json::<Value>().await.unwrap()["error"]["code"], "policy_denied");
    // D-029/D-030: any other `X-*-Target` header is not read, and a client sending one is refused with a readable
    // error rather than having its request run locally.
    let r = client()
        .post(srv.url("/v1/completions"))
        .header("x-other-target", format!("peer:{}", stranger.to_hex()))
        .json(&json!({ "model": MODEL_ID, "prompt": "x", "max_tokens": 1 }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    let v = r.json::<Value>().await.unwrap();
    assert_eq!(v["error"]["code"], "invalid_request", "{v}");
    let msg = v["error"]["message"].as_str().unwrap();
    assert!(
        msg.contains("x-other-target") && msg.contains("X-UCompute-Target"),
        "{v}"
    );
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
        .header("x-ucompute-target", format!("peer:{}", stranger.to_hex()))
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
        ucompute_policy::PrivacyMode::TrustedGroup
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
            paired_at_utc: ucompute_identity::utc_now(),
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

/// WP-04 C6 (ADR-0005 D5, I-05): a split target is decided by the policy for POOLED inference
/// before admission, any load or any network activity. Remote inference alone does not grant it.
/// One server, nothing listens at the pinned address; no model file is needed.
#[tokio::test(flavor = "multi_thread")]
async fn split_target_denied_before_any_connection_without_pooled_policy() {
    let (srv, _dir) = empty_server().await;
    let stranger = DeviceIdentity::generate().unwrap().id;
    let body = json!({ "model": MODEL_ID, "prompt": PROMPT, "max_tokens": 4, "target": format!("split:{}", stranger.to_hex()), "split_at": 3, "acknowledged": true });
    let refused = |want: &'static str| {
        let body = body.clone();
        let srv = &srv;
        async move {
            let (status, v) = post(srv, "/v1/completions", body).await;
            assert_eq!((status, v["error"]["code"].as_str()), (403, Some(want)), "{v}");
        }
    };
    refused("policy_denied").await; // local_only
    let (status, _) = put(
        &srv,
        "/api/privacy",
        json!({ "mode": "trusted_group", "acknowledged": true }),
    )
    .await;
    assert_eq!(status, 200);
    refused("policy_denied").await; // networking off
    enable_network(&srv, false).await;
    refused("peer_unauthorized").await; // not paired
    pin(&srv, stranger, "stranger", Some("127.0.0.1:1".into()));
    refused("peer_unauthorized").await; // paired, in no group
    put_group(&srv, "home", &[stranger], true, false).await;
    refused("peer_unauthorized").await; // remote inference only: not pooling
                                        // the header form is decided the same way
    let r = client()
        .post(srv.url("/v1/completions"))
        .header("x-ucompute-target", format!("split:{}", stranger.short_label()))
        .json(&json!({ "model": MODEL_ID, "prompt": PROMPT, "split_at": 3, "acknowledged": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    assert_eq!(srv.state.split_slot().as_ref().map(|a| a.peer), None);
    assert_no_network_activity(&srv).await;
    let kinds = audit_kinds(&get(&srv, "/api/audit?n=1000").await);
    assert!(
        !kinds.iter().any(|k| k.starts_with("session_") || k.starts_with("rpc_")),
        "{kinds:?}"
    );
    srv.stop().await;
}

/// WP-04 C6 (ADR-0005 D5, D8, X-08): once the policy allows pooling, `split_at` is required, the
/// disclosure must be acknowledged (403 carrying its text), and a model whose licence does not
/// allow uploading its weights is refused, all before any connection. The model is the REAL
/// stories15M file (licence unknown, B-011) from UCOMPUTE_HOME; no engine runs. This binary never
/// sets `UCOMPUTE_TEST_SINGLE_HOST_POOL`, so nothing lifts the licence gate.
#[tokio::test(flavor = "multi_thread")]
async fn split_requires_split_at_and_acknowledgement() {
    let Some(home) = fixture_home() else {
        return;
    };
    assert!(std::env::var_os(ucompute_network::rpc::TEST_POOL_ENV).is_none());
    let dir = tempfile::tempdir().unwrap();
    let srv = Server::start(serving_app(&home, dir.path(), cfg())).await;
    let (status, _) = put(
        &srv,
        "/api/privacy",
        json!({ "mode": "trusted_group", "acknowledged": true }),
    )
    .await;
    assert_eq!(status, 200);
    enable_network(&srv, false).await;
    let stranger = DeviceIdentity::generate().unwrap().id;
    pin(&srv, stranger, "stranger", Some("127.0.0.1:1".into()));
    put_group(&srv, "home", &[stranger], false, true).await;
    let target = format!("split:{}", stranger.to_hex());
    let post_c = |extra: Value| {
        let mut body = json!({ "model": MODEL_ID, "prompt": PROMPT, "max_tokens": 4, "target": target });
        body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let srv = &srv;
        async move { post(srv, "/v1/completions", body).await }
    };
    let (status, v) = post_c(json!({ "acknowledged": true })).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (400, Some("invalid_request")),
        "{v}"
    );
    assert!(v["error"]["message"].as_str().unwrap().contains("split_at"), "{v}");
    let (status, v) = post_c(json!({ "split_at": 3 })).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("policy_denied")),
        "{v}"
    );
    assert_eq!(
        v["error"]["message"],
        ucompute_api::split::disclosure(&stranger.short_label())
    );
    assert!(v["error"]["hint"].as_str().unwrap().contains("acknowledged"), "{v}");
    let (status, v) = post_c(json!({ "split_at": 3, "acknowledged": true })).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("policy_denied")),
        "{v}"
    );
    assert!(
        v["error"]["message"].as_str().unwrap().contains("licence (unknown)"),
        "{v}"
    );
    // chat takes the same fields
    let (status, v) = post(
        &srv,
        "/v1/chat/completions",
        chat_body(MODEL_ID, 4, json!({ "target": target, "split_at": 3 })),
    )
    .await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("policy_denied")),
        "{v}"
    );
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("Pooling splits one model"),
        "{v}"
    );
    assert_no_remote_request(&srv).await;
    assert!(srv.state.manager.snapshot().loaded.is_none(), "nothing was loaded");
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
    // B is still pinned to A, but A's verifier refuses the revoked key inside the handshake. That
    // alert comes before A sent any control data, so B cannot tell it from a forged one: it reads
    // as a device that did not prove it is A, never as revoked (trust-3).
    let e = b.state.peer_connection(&nb, ida).await.unwrap_err();
    assert_eq!(e.code, ucompute_core::ErrorCode::PeerUnauthorized, "{e}");
    assert!(e.message.contains("did not prove"), "{e}");
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
        .header("x-ucompute-target", &target)
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
            &format!("ucompute_remote_requests_total{{peer=\"{}\"}}", idb.short_label())
        ),
        Some(3.0),
        "{metrics}"
    );
    // Tokens B decoded are not A's generation: A's own counters stay at zero.
    assert_eq!(metric(&metrics, "ucompute_tokens_generated_total"), Some(0.0));
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

/// `ucompute privacy local-only` writes config.json from another process while the daemon runs: the
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
        privacy: ucompute_policy::PrivacyMode::LocalOnly,
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
        (ucompute_policy::PrivacyMode::LocalOnly, true, true)
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
    // C's verifier refuses A inside the handshake: unproven at A (trust-3), never `peer_revoked`.
    let e = a.state.peer_connection(&na, idc).await.unwrap_err();
    assert_eq!(e.code, ucompute_core::ErrorCode::PeerUnauthorized, "{e}");
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
    assert_eq!(s.privacy, ucompute_policy::PrivacyMode::LocalOnly);
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
/// `UCOMPUTE_MDNS=0` skips it.
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
    if std::env::var("UCOMPUTE_MDNS").ok().as_deref() == Some("0") {
        eprintln!("SKIP peers_list_never_shows_this_device_as_discovered_single_host: UCOMPUTE_MDNS=0");
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
/// Plus 2 per loss probe (`quic_sent`): the probe and the immediate ACK it asks for.
fn keepalive_bound(elapsed: Duration, keep_alive: Duration, probes: u64) -> u64 {
    4 * (elapsed.as_millis() / keep_alive.as_millis() + 1) as u64 + 2 * probes
}

/// What the QUIC stacks of `servers` have sent so far, over all their connections: (loss probes,
/// STREAM + DATAGRAM frames). A keep-alive PING is ACKed after the peer's ACK delay (up to 25 ms),
/// and its PTO is that delay plus ~1-2 ms on loopback, so timer jitter can fire it: quinn then
/// sends two probes, each an IMMEDIATE_ACK frame (ack-frequency extension) answered at once. STREAM
/// and DATAGRAM frames are the only ones that carry application bytes.
async fn quic_sent(servers: [&Server; 2]) -> (u64, u64) {
    let mut out = (0, 0);
    for srv in servers {
        let Ok(node) = srv.state.node().await else { continue };
        for c in node.connections() {
            let f = c.conn.stats().frame_tx;
            out.0 += f.immediate_ack;
            out.1 += f.stream + f.datagram;
        }
    }
    out
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
        peer_quota: ucompute_api::mesh::PeerQuota {
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
fn server_request_entries(b: &Server) -> Vec<ucompute_observability::AuditEvent> {
    b.state
        .audit
        .tail(1_000_000)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "remote_request" || (e.kind == "peer_rejected" && e.fields.get("request_id").is_some()))
        .collect()
}

/// A's `remote_request` entries of `phase`.
fn sender_entries(a: &Server, phase: &str) -> Vec<ucompute_observability::AuditEvent> {
    a.state
        .audit
        .tail(1_000_000)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "remote_request" && e.fields["phase"] == phase)
        .collect()
}

fn request_ids(entries: &[ucompute_observability::AuditEvent]) -> Vec<String> {
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
    let q0 = quic_sent([&a, &b]).await;
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
    let q1 = quic_sent([&a, &b]).await;
    let probes = q1.0.saturating_sub(q0.0);
    let bound = keepalive_bound(elapsed, Duration::from_millis(cfg().network.keep_alive_ms), probes);
    eprintln!(
        "SINGLE-HOST F-10-10: {N} peer-targeted requests in local_only in {elapsed:?}: relay {} datagrams / {} bytes \
         (largest {largest} B; keep-alive bound {bound} datagrams of <= {KEEPALIVE_MAX_BYTES} B, {probes} loss probes); B audit +{}, A sent +{}",
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
    assert_eq!(q1.1, q0.1, "no STREAM or DATAGRAM frame left either device");
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
        let q4 = quic_sent([&a, &b]).await;
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
        let probes = quic_sent([&a, &b]).await.0.saturating_sub(q4.0);
        let bound = keepalive_bound(t4.elapsed(), Duration::from_millis(cfg().network.keep_alive_ms), probes);
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
    privacy: ucompute_policy::PrivacyMode,
    network: bool,
    group: GroupPolicy,
    t: Target,
) -> Option<&'static str> {
    use ucompute_policy::PrivacyMode::*;
    match (privacy, network, t) {
        (_, _, Target::Local) => unreachable!("local requests are not peer-targeted"),
        (LocalOnly | SwarmAllowed, _, _) | (TrustedGroup, false, _) => Some("policy_denied"),
        (TrustedGroup, true, Target::Revoked) => Some("peer_revoked"),
        (TrustedGroup, true, Target::Unknown | Target::Unpaired) => Some("peer_unauthorized"),
        (TrustedGroup, true, Target::B) if group == GroupPolicy::Remote => None,
        (TrustedGroup, true, Target::B) => Some("peer_unauthorized"),
    }
}

/// The same for a `split:` target (WP-04): pooled inference needs a group allowing pooling, which
/// remote inference does not grant. A split the policy allows still never leaves this device here:
/// A has no model file to split (`model_not_downloaded`, before the licence gate or any session).
fn split_model(
    privacy: ucompute_policy::PrivacyMode,
    network: bool,
    group: GroupPolicy,
    t: Target,
) -> (u16, &'static str) {
    match (privacy, network, t) {
        (ucompute_policy::PrivacyMode::TrustedGroup, true, Target::B) if group == GroupPolicy::PooledOnly => {
            (404, "model_not_downloaded")
        }
        (ucompute_policy::PrivacyMode::TrustedGroup, true, Target::B) => (403, "peer_unauthorized"),
        _ => (403, policy_model(privacy, network, group, t).expect("denied")),
    }
}

/// F-06-07, SINGLE-HOST: 10,000 requests through A's HTTP API with randomized (privacy mode,
/// target local / B / unknown / revoked / unpaired, as `peer:` or (WP-04) `split:`, A's group
/// policy for B, A's networking on/off)
/// from a seeded PRNG (`UCOMPUTE_TEST_SEED` replays a run). Settings change per epoch of 20-80
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
    use ucompute_policy::{ExecutionTarget, PrivacyMode, WorkloadKind};
    const TOTAL: usize = 10_000;
    // Paced so the denied-only windows add up to tens of seconds of real keep-alive traffic.
    const PACE: Duration = Duration::from_millis(2);
    let Some(home) = fixture_home() else {
        return;
    };
    let _engine = ENGINE_TESTS.lock().await;
    let seed = std::env::var("UCOMPUTE_TEST_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0x0C0A_F067_u64);
    eprintln!("SINGLE-HOST F-06-07: seed {seed} (UCOMPUTE_TEST_SEED={seed} replays this run)");
    let mut rng = Rng(seed);
    let started = Instant::now();
    let Relayed { a, b, relay, _dirs } = relayed_pair(&home).await;
    // This test is about policy (F-06-07), not scheduling: on a CPU-only host (sentinel) the D-026 default
    // `refuse_new` would refuse allowed peer requests near the target's own local work, read as 429 `overloaded`.
    for d in [&a, &b] {
        let (status, _) = put(d, "/api/network", json!({ "local_priority": "none" })).await;
        assert_eq!(status, 200);
    }
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
    let (mut denied_time_connected, mut allowed_window_dgrams, mut denied_probes) = (Duration::ZERO, 0u64, 0u64);
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
        let q0 = quic_sent([&a, &b]).await;
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
            // WP-04: a third of the peer-targeted requests ask for a split instead
            let split = id.is_some() && rng.below(3) == 0;
            let label = id.map(|id| format!("{}:{}", if split { "split" } else { "peer" }, id.to_hex()));
            let mut body = if chat {
                chat_body(MODEL_ID, 1, json!({}))
            } else {
                json!({ "model": MODEL_ID, "prompt": PROMPT, "max_tokens": 1, "temperature": 0 })
            };
            if split {
                body["split_at"] = json!(3);
                body["acknowledged"] = json!(true);
            }
            let mut req = http.post(a.url(if chat {
                "/v1/chat/completions"
            } else {
                "/v1/completions"
            }));
            match (&label, in_header) {
                (Some(l), true) => req = req.header("x-ucompute-target", l),
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
            if split {
                let want = split_model(privacy, network, group, target);
                assert_eq!(
                    (status, code.as_str()),
                    want,
                    "split {target:?} {privacy:?} net={network} {group:?}: {v}"
                );
                *by_outcome.entry(format!("split:{}", want.1)).or_default() += 1;
                continue;
            }
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
                        matches!(&d, ucompute_policy::Decision::Deny { code, .. } if code.as_str() == want),
                        "{d:?}"
                    );
                    *by_outcome.entry(format!("peer:{want}")).or_default() += 1;
                }
            }
        }
        let elapsed = t0.elapsed();
        let (d1, _, largest) = relay.window();
        let q1 = quic_sent([&a, &b]).await;
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
            let probes = q1.0.saturating_sub(q0.0);
            let bound = keepalive_bound(elapsed, keep_alive, probes);
            assert!(
                d1 - d0 <= bound && largest <= KEEPALIVE_MAX_BYTES,
                "epoch {epochs} ({privacy:?} net={network} {group:?}, before {before:?}): {} datagrams (largest {largest} B) in {elapsed:?}, keep-alive bound {bound}",
                d1 - d0
            );
            assert_eq!(
                q1.1, q0.1,
                "epoch {epochs}: a STREAM or DATAGRAM frame in a denied-only window"
            );
            denied_probes += probes;
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
         datagrams (keep-alive bound {denied_window_bound}, {denied_probes} loss probes), largest {largest_denied} B, \
         no STREAM or DATAGRAM frame"
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
    // no split target opened a pooled session or a tunnel on either device
    for srv in [&a, &b] {
        let all = srv.state.audit.tail(10_000_000).unwrap();
        assert!(!all
            .iter()
            .any(|e| e.kind.starts_with("session_") || e.kind.starts_with("rpc_")));
    }
    assert!(by_outcome.contains_key("split:model_not_downloaded"), "{by_outcome:?}");
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
        peer_quota: ucompute_api::mesh::PeerQuota {
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

// ------------------------------------------- addresses, reconnection and shutdown (D2a, D3, D5, E1, E4, E6)

/// `POST /api/peers/{id}/connect` from `srv`.
async fn connect_peer(srv: &Server, id: &str, body: Value) -> (u16, Value) {
    post(srv, &format!("/api/peers/{id}/connect"), body).await
}

/// `peer_rejected` lines in `srv`'s audit log.
fn rejections(srv: &Server) -> Vec<ucompute_observability::AuditEvent> {
    srv.state
        .audit
        .tail(1000)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "peer_rejected")
        .collect()
}

/// D2a, SINGLE-HOST: a stale or spoofed hint that reaches another UCompute device (C) is refused by
/// the pinned-key check, and the dial goes on to B's pinned address instead of stopping at that
/// refusal; the refused address is never stored. A's Node is started by hand (networking stays
/// off in A's settings), so no sync loop races this dial.
#[tokio::test(flavor = "multi_thread")]
async fn wrong_device_at_hint_falls_through_to_pinned_single_host() {
    let (a, _da) = empty_server().await;
    let (b, _db) = empty_server().await;
    let (c, _dc) = empty_server().await;
    let b_addr = listen_addr(&enable_network(&b, true).await);
    let c_addr: std::net::SocketAddr = listen_addr(&enable_network(&c, true).await).parse().unwrap();
    let (ida, idb) = (a.state.identity.id, b.state.identity.id);
    pin(&a, idb, "B", Some(b_addr.clone()));
    pin(&b, ida, "A", None);
    let na = ucompute_network::transport::Node::start(
        NetworkConfig {
            enabled: true,
            ..cfg().network
        },
        a.state.identity.clone(),
        a.state.peers.clone(),
        a.state.policy(),
        a.state.audit.clone(),
    )
    .await
    .unwrap();

    let conn = a.state.peer_connection_via(&na, idb, Some(c_addr)).await.unwrap();
    assert_eq!((conn.peer_id, conn.remote_addr.to_string()), (idb, b_addr.clone()));
    assert_eq!(
        a.state.peers.get(&idb).unwrap().unwrap().last_addr,
        Some(b_addr),
        "C's address is never stored"
    );
    let refused = rejections(&a);
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert_eq!(
        (refused[0].fields["addr"].as_str(), refused[0].fields["reason"].as_str()),
        (Some(c_addr.to_string().as_str()), Some("peer_unauthorized"))
    );
    assert!(c.state.node().await.unwrap().connections().is_empty());
    eprintln!("SINGLE-HOST D2a: a hint at another device was refused and the pinned address used");
    na.shutdown().await;
    for srv in [a, b, c] {
        srv.stop().await;
    }
}

/// D2a + trust-3, SINGLE-HOST: a refusal closed before the device at an address proved B's key
/// with control data does not end the dial, even when that device showed B's key and signed the
/// handshake with it: the dialler cannot tell such a close from one forged in an Initial packet on
/// the path (ucompute-network `a_close_forged_after_the_genuine_flight_is_not_a_refusal_single_host`),
/// and stopping there would let any host at a hinted or discovered address keep A from B. Two
/// endpoints holding B's key stand in at the hint: one whose store revoked A (a
/// `certificate_revoked` alert), one that holds A without a held exchange (`CLOSE_HELD` right after
/// TLS, as a minor-0 build). Each time the dial goes on to B's pinned address and connects; the
/// hint is audited `peer_unauthorized`, never `peer_revoked` or `peer_held`, and never stored.
/// (A proven refusal still ends the dial: `a_held_refusal_ends_the_dial_single_host`.)
#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_before_the_device_proved_its_key_does_not_end_the_dial_single_host() {
    let (a, _da) = empty_server().await;
    let (b, _db) = empty_server().await;
    let b_addr = listen_addr(&enable_network(&b, true).await);
    let (ida, idb) = (a.state.identity.id, b.state.identity.id);
    pin(&a, idb, "B", Some(b_addr.clone()));
    pin(&b, ida, "A", None);
    let net = NetworkConfig {
        enabled: true,
        ..cfg().network
    };
    let start = |srv: &Server, peers| {
        ucompute_network::transport::Node::start(
            net.clone(),
            srv.state.identity.clone(),
            peers,
            srv.state.policy(),
            srv.state.audit.clone(),
        )
    };
    let store = |dir: &tempfile::TempDir| Arc::new(ucompute_identity::PeerStore::open(&Paths::at(dir.path())).unwrap());
    let (d2, d3) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let revoker = store(&d2);
    revoker.revoke(&ida, &b.state.identity).unwrap();
    let holds = store(&d3);
    holds
        .pin(PeerRecord {
            id: ida,
            name: "A".into(),
            paired_at_utc: "2026-10-22T00:00:00Z".into(),
            last_seen_utc: None,
            last_addr: None,
        })
        .unwrap();
    let (nb2, nb3) = (start(&b, revoker).await.unwrap(), start(&b, holds).await.unwrap());
    nb3.hold(&ida);
    let na = start(&a, a.state.peers.clone()).await.unwrap();

    for fake in [nb2.local_addr().unwrap(), nb3.local_addr().unwrap()] {
        let conn = a.state.peer_connection_via(&na, idb, Some(fake)).await.unwrap();
        assert_eq!(
            (conn.peer_id, conn.remote_addr.to_string()),
            (idb, b_addr.clone()),
            "{fake}"
        );
        assert_eq!(
            a.state.peers.get(&idb).unwrap().unwrap().last_addr,
            Some(b_addr.clone())
        );
        let refused: Vec<_> = rejections(&a)
            .into_iter()
            .filter(|e| e.fields["addr"] == fake.to_string())
            .collect();
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert_eq!(refused[0].fields["reason"], "peer_unauthorized", "{refused:?}");
        conn.conn.close(0u32.into(), b"next");
        assert!(na.connections().is_empty());
    }
    eprintln!("SINGLE-HOST D2a: refusals before the key was proven did not end the dial");
    for n in [na, nb2, nb3] {
        n.shutdown().await;
    }
    a.stop().await;
    b.stop().await;
}

/// Requirements 5 and 8, SINGLE-HOST: `POST /api/peers/{id}/connect` reaches a paired device at an
/// address typed by hand (no mDNS, and A has no address on record for B), but an address proves
/// nothing by itself: an id that is not paired sends no packet at all, another device at the given
/// address is refused by the pinned-key check and nothing is stored, and the reason shows in
/// `last_error` until a connection succeeds.
#[tokio::test(flavor = "multi_thread")]
async fn manual_address_reaches_a_paired_device_and_proves_nothing_by_itself_single_host() {
    let (a, _da) = empty_server().await;
    let (b, _db) = empty_server().await;
    let (c, _dc) = empty_server().await;
    enable_network(&a, true).await;
    let b_addr = listen_addr(&enable_network(&b, true).await);
    let c_addr = listen_addr(&enable_network(&c, true).await);
    let (ida, idb) = (a.state.identity.id, b.state.identity.id);
    // Neither side has an address for the other: only the dials below can connect them.
    pin(&a, idb, "B", None);
    pin(&b, ida, "A", None);

    // Not paired here: refused before anything is sent (a socket at that address hears nothing).
    let trap = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let stranger = DeviceIdentity::generate().unwrap().id;
    let (status, v) = connect_peer(
        &a,
        &stranger.to_hex(),
        json!({ "addr": trap.local_addr().unwrap().to_string() }),
    )
    .await;
    assert_eq!(
        (status, v["error"]["code"].as_str(), v["error"]["hint"].as_str()),
        (403, Some("peer_unauthorized"), Some("pair it first")),
        "{v}"
    );
    let mut buf = [0u8; 64];
    assert!(
        tokio::time::timeout(Duration::from_millis(500), trap.recv_from(&mut buf))
            .await
            .is_err(),
        "a packet left for a device that is not paired"
    );
    for bad in ["nope", "127.0.0.1", "127.0.0.1:0", "0.0.0.0:8193", "224.0.0.251:5353"] {
        let (status, v) = connect_peer(&a, &idb.to_hex(), json!({ "addr": bad })).await;
        assert_eq!(
            (status, v["error"]["code"].as_str()),
            (400, Some("invalid_request")),
            "{bad}: {v}"
        );
    }

    // Another UCompute device at the typed address: its key is not B's, so nothing connects or is stored.
    let (status, v) = connect_peer(&a, &idb.short_label(), json!({ "addr": c_addr })).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("peer_unauthorized")),
        "{v}"
    );
    assert_eq!(a.state.peers.get(&idb).unwrap().unwrap().last_addr, None);
    assert_no_remote_request(&a).await;
    let peers = get(&a, "/api/peers").await;
    let err = &peers["peers"][0]["last_error"];
    assert_eq!(
        (err["addr"].as_str(), err["code"].as_str()),
        (Some(c_addr.as_str()), Some("peer_unauthorized")),
        "{peers}"
    );
    assert!(err["at_utc"].is_string() && err["message"].is_string(), "{err}");

    // B at its own address: connected, and only now is that address stored.
    let (status, rec) = connect_peer(&a, &idb.to_hex(), json!({ "addr": b_addr })).await;
    assert_eq!(status, 200, "{rec}");
    assert_eq!(
        (rec["id"].as_str(), rec["last_addr"].as_str()),
        (Some(idb.to_hex().as_str()), Some(b_addr.as_str()))
    );
    let peers = get(&a, "/api/peers").await;
    assert_eq!(peers["peers"][0]["connected"], true, "{peers}");
    assert!(
        peers["peers"][0]["last_error"].is_null(),
        "cleared by the connection: {peers}"
    );
    let (status, rec) = connect_peer(&a, &idb.to_hex(), json!({})).await;
    assert_eq!(status, 200, "already connected: {rec}");
    // B has no pinned port for A, so A's inbound connection teaches it no address.
    assert_eq!(b.state.peers.get(&ida).unwrap().unwrap().last_addr, None);

    let (status, _) = put(&a, "/api/network", json!({ "enabled": false })).await;
    assert_eq!(status, 200);
    let (status, v) = connect_peer(&a, &idb.to_hex(), json!({})).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("policy_denied")),
        "{v}"
    );
    eprintln!("SINGLE-HOST requirements 5/8: manual address connects only the pinned key");
    for srv in [a, b, c] {
        srv.stop().await;
    }
}

/// D3b, SINGLE-HOST: B has A on record at an address that no longer answers (127.0.0.2 with A's
/// port: "A moved"). When A connects in, B stores A's authenticated source IP with the pinned
/// port, so B can dial A again after A's IP changed on a network that drops mDNS.
#[tokio::test(flavor = "multi_thread")]
async fn inbound_connection_updates_a_stale_pinned_address_single_host() {
    let (a, _da) = empty_server().await;
    let (b, _db) = empty_server().await;
    let a_addr: std::net::SocketAddr = listen_addr(&enable_network(&a, true).await).parse().unwrap();
    let b_addr = listen_addr(&enable_network(&b, true).await);
    let (ida, idb) = (a.state.identity.id, b.state.identity.id);
    let stale = std::net::SocketAddr::new([127, 0, 0, 2].into(), a_addr.port());
    pin(&b, ida, "A", Some(stale.to_string()));
    pin(&a, idb, "B", Some(b_addr));
    let rec = wait_for("B learns A's address from A's connection", || async {
        b.state
            .peers
            .get(&ida)
            .unwrap()
            .filter(|r| r.last_addr == Some(a_addr.to_string()))
    })
    .await;
    assert!(rec.last_seen_utc.is_some(), "{rec:?}");
    eprintln!("SINGLE-HOST D3b: {stale} replaced by {a_addr} from A's authenticated connection");
    a.stop().await;
    b.stop().await;
}

/// D5/E4, SINGLE-HOST: a pinned device whose address never answers (a UDP socket that only counts
/// what arrives). The sync loop keeps dialling with backoff but audits the failure once, not per
/// attempt; GET /api/peers says why (`last_error`: the address, `peer_lost`, "no answer", a hint);
/// a user's connect is not held back by the backoff and fails within the idle timeout (502).
#[tokio::test(flavor = "multi_thread")]
async fn unreachable_paired_device_is_audited_once_and_says_why_single_host() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = cfg();
    config.network.idle_timeout_ms = 1000;
    config.network.keep_alive_ms = 250;
    let a = Server::start(app(config, dir.path().to_path_buf(), HardwareReport::default())).await;
    let silent = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let silent_addr = silent.local_addr().unwrap();
    // One dial = one QUIC connection = one destination connection id in its Initial packets
    // (long header: flags, version (4 bytes), DCID length, DCID).
    let dcids = Arc::new(std::sync::Mutex::new(std::collections::HashSet::<Vec<u8>>::new()));
    let seen = dcids.clone();
    let counter = tokio::spawn(async move {
        let mut buf = vec![0u8; 2048];
        while let Ok((n, _)) = silent.recv_from(&mut buf).await {
            let l = usize::from(*buf.get(5).unwrap_or(&0));
            if n > 6 + l && buf[0] & 0x80 != 0 {
                seen.lock().unwrap().insert(buf[6..6 + l].to_vec());
            }
        }
    });
    let dials = || dcids.lock().unwrap().len();
    let x = DeviceIdentity::generate().unwrap().id;
    pin(&a, x, "X", Some(silent_addr.to_string()));
    enable_network(&a, true).await;
    wait_for("two dials by the sync loop", || async { (dials() >= 2).then_some(()) }).await;
    // The second dial fails too: a handshake waits at least three initial probe timeouts (~3 s).
    tokio::time::sleep(Duration::from_millis(3500)).await;

    let peers = get(&a, "/api/peers").await;
    let err = &peers["peers"][0]["last_error"];
    assert_eq!(
        (err["addr"].as_str(), err["code"].as_str()),
        (Some(silent_addr.to_string().as_str()), Some("peer_lost")),
        "{peers}"
    );
    assert!(err["message"].as_str().unwrap().contains("no answer"), "{err}");
    let port = format!("UDP port {}", silent_addr.port());
    assert!(err["hint"].as_str().unwrap().contains(&port), "{err}");
    assert_eq!(rejections(&a).len(), 1, "one line per state change, {} dials", dials());

    let before = dials();
    let t0 = Instant::now();
    let (status, v) = connect_peer(&a, &x.to_hex(), json!({})).await;
    eprintln!(
        "SINGLE-HOST D5: {} sync-loop dials, one audit line; a manual connect answered {status} in {:?}",
        before,
        t0.elapsed()
    );
    assert_eq!((status, v["error"]["code"].as_str()), (502, Some("peer_lost")), "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("no answer"), "{v}");
    assert!(v["error"]["hint"].is_string(), "{v}");
    // The handler's own dial (only it answers this 502), one and at once: a handshake gives up
    // after the idle timeout but never before three initial probe timeouts (RFC 9000 10.1), about
    // 3 s. A dial count cannot tell it from the sync loop's redial; that the connect clears the
    // backoff is mesh::tests::connect_clears_the_backoff_so_the_sync_loop_dials_at_once_single_host.
    assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
    assert_eq!(rejections(&a).len(), 1);
    counter.abort();
    a.stop().await;
}

/// E1, SINGLE-HOST: without a name (or with a blank one) both sides of a pairing are named after
/// their host (`device_name`, also in GET /api/network), never a fixed string.
#[tokio::test(flavor = "multi_thread")]
async fn pairing_without_a_name_uses_the_host_name_single_host() {
    let (a, _da) = empty_server().await;
    let (b, _db) = empty_server().await;
    let host = ucompute_api::mesh::device_name();
    let net = get(&a, "/api/network").await;
    assert_eq!(net["device_name"], host.as_str());
    assert!(net["dial_addr"].is_null(), "{net}");
    let net = enable_network(&a, true).await;
    assert_eq!(net["dial_addr"], net["listen_addr"], "bound to one address: that one");
    enable_network(&b, true).await;
    let (status, start) = post(&b, "/api/pair/start", json!({ "name": "  " })).await;
    assert_eq!(status, 200, "{start}");
    let (status, rec) = post(
        &a,
        "/api/pair/join",
        json!({ "addr": start["pair_addr"], "code": start["code"] }),
    )
    .await;
    assert_eq!((status, rec["name"].as_str()), (200, Some(host.as_str())), "{rec}");
    let status = wait_for("B pairing result", || async {
        let v = get(&b, "/api/pair/status").await;
        (v["active"] == false).then_some(v)
    })
    .await;
    assert_eq!(status["result"]["peer"]["name"], host.as_str(), "{status}");
    a.stop().await;
    b.stop().await;
}

/// E6, SINGLE-HOST: `POST /api/shutdown` stops the daemon the way SIGTERM does, well inside the
/// 5 s drain grace (a cancelled token alone used to end serving only when that grace ran out),
/// closing the peer endpoint so a connected device sees a close, not a timeout. A cross-origin
/// page cannot do it.
#[tokio::test(flavor = "multi_thread")]
async fn shutdown_route_stops_serving_and_closes_the_peer_endpoint_single_host() {
    let (mut a, _da) = empty_server().await;
    let (b, _db) = empty_server().await;
    let a_addr = listen_addr(&enable_network(&a, true).await);
    enable_network(&b, true).await;
    let (ida, idb) = (a.state.identity.id, b.state.identity.id);
    pin(&a, idb, "B", None);
    pin(&b, ida, "A", Some(a_addr));
    let nb = b.state.node().await.unwrap();
    wait_for("B connected to A", || async {
        nb.connections().iter().any(|c| c.peer_id == ida).then_some(())
    })
    .await;

    let r = client()
        .post(a.url("/api/shutdown"))
        .header("origin", "https://evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    assert!(a.finished(Duration::from_millis(300)).await.is_none(), "still serving");

    let t0 = Instant::now();
    let r = client().post(a.url("/api/shutdown")).send().await.unwrap();
    assert_eq!(r.status(), 202);
    assert_eq!(r.json::<Value>().await.unwrap(), json!({ "status": "shutting_down" }));
    let served = a
        .finished(Duration::from_secs(3))
        .await
        .expect("serve_on returned well inside the 5 s grace");
    assert!(served.is_ok(), "{served:?}");
    let stopped = t0.elapsed();
    assert!(a.state.net.lock().await.node.is_none(), "peer endpoint closed");
    wait_for("B sees A's connection closed", || async {
        nb.connections().iter().all(|c| c.peer_id != ida).then_some(())
    })
    .await;
    let closed = t0.elapsed();
    eprintln!("SINGLE-HOST E6: POST /api/shutdown -> daemon stopped in {stopped:?}, B saw the close at {closed:?}");
    assert!(
        closed < Duration::from_secs(3),
        "a close, not B's 4 s idle timeout: {closed:?}"
    );
    a.stop().await;
    b.stop().await;
}

/// `id`'s entry in a `GET /api/peers` body.
fn peer_entry(v: &Value, id: PeerId) -> Value {
    v["peers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == id.to_hex())
        .cloned()
        .unwrap_or_else(|| panic!("{} not in {v}", id.short_label()))
}

/// `kind` audit events about `peer` in `srv`'s log.
fn audit_about(srv: &Server, kind: &str, peer: PeerId) -> Vec<ucompute_observability::AuditEvent> {
    srv.state
        .audit
        .tail(1000)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == kind && e.peer_id.as_deref() == Some(peer.to_hex().as_str()))
        .collect()
}

async fn wait_connected(srv: &Server, id: PeerId, want: bool) {
    wait_for("connection state", || async {
        (peer_entry(&get(srv, "/api/peers").await, id)["connected"] == want).then_some(())
    })
    .await;
}

/// **SINGLE-HOST** E1 + E2 + E3: A (nothing downloaded) and B (real engine, fixture model), pinned
/// to each other with no address, so only A's explicit connect links them. A sees what B signed at
/// the handshake (hardware, engines, the fixture model resolved to its id) and the live connection
/// (stable id, traffic); a paired device that never connected shows nulls, not stale values. One
/// remote completion: B's engine is loaded for A, both devices list the request as a job with the
/// same `request_id` (sender: target peer; server: served_for_peer), and neither holds any text.
#[tokio::test(flavor = "multi_thread")]
async fn peer_details_engine_origin_and_jobs_on_both_devices_single_host() {
    let Some(home) = fixture_home() else {
        return;
    };
    let _engine = ENGINE_TESTS.lock().await;
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = Server::start(app(cfg(), da.path().to_path_buf(), HardwareReport::default())).await;
    let b = Server::start(serving_app(&home, db.path(), serving_cfg())).await;
    enable_network(&a, false).await;
    let b_addr = listen_addr(&enable_network(&b, true).await);
    let (ida, idb) = (a.state.identity.id, b.state.identity.id);
    pin(&a, idb, "B", None);
    pin(&b, ida, "A", None);
    let ghost = DeviceIdentity::generate().unwrap().id;
    pin(&a, ghost, "Ghost", None);
    put_group(&a, "home", &[idb], true, false).await;
    put_group(&b, "home", &[ida], true, false).await;
    let (status, _) = put(
        &a,
        "/api/privacy",
        json!({ "mode": "trusted_group", "acknowledged": true }),
    )
    .await;
    assert_eq!(status, 200);
    let (status, v) = connect_peer(&a, &idb.to_hex(), json!({ "addr": b_addr })).await;
    assert_eq!(status, 200, "{v}");
    wait_connected(&b, ida, true).await;

    let pb = peer_entry(&get(&a, "/api/peers").await, idb);
    let caps = &pb["capabilities"];
    assert!(caps["hardware"]["cpu"]["logical_cores"].as_u64().unwrap() > 0, "{caps}");
    let probe = ucompute_hardware::probe().unwrap();
    assert_eq!(
        caps["hardware"]["memory"]["total_bytes"], probe.memory.total_bytes,
        "static fields as B probed them"
    );
    assert!(!caps["engines"].as_array().unwrap().is_empty(), "{caps}");
    assert_eq!(caps["ucompute_version"], ucompute_core::VERSION);
    let mut keys: Vec<&str> = caps.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "engines",
            "free_memory_bytes",
            "hardware",
            "issued_at_utc",
            "models",
            "protocol_minor",
            "ucompute_version"
        ],
        "exactly the documented keys"
    );
    assert!(
        caps["issued_at_utc"].is_string() && caps["protocol_minor"].is_u64() && caps["free_memory_bytes"].is_u64(),
        "{caps}"
    );
    assert!(
        caps["models"]
            .as_array()
            .unwrap()
            .contains(&json!({ "sha256": MODEL_SHA, "id": MODEL_ID })),
        "B caches the fixture: {caps}"
    );
    let conn = pb["connection"].clone();
    assert_eq!(
        (conn["direction"].as_str(), conn["remote_addr"].as_str()),
        (Some("outbound"), Some(b_addr.as_str())),
        "{conn}"
    );
    assert!(
        conn["opened_at_utc"].is_string() && conn["rtt_ms"].is_number(),
        "{conn}"
    );
    assert_eq!(pb["held"], false);
    let again = peer_entry(&get(&a, "/api/peers").await, idb);
    assert_eq!(
        again["connection"]["id"], conn["id"],
        "the same connection keeps its id"
    );
    let pa = peer_entry(&get(&b, "/api/peers").await, ida);
    assert_eq!(pa["connection"]["direction"], "inbound", "B was dialled: {pa}");
    assert_eq!(
        pa["capabilities"]["models"],
        json!([]),
        "A has nothing downloaded: {pa}"
    );
    let g = peer_entry(&get(&a, "/api/peers").await, ghost);
    assert_eq!(
        (&g["connected"], &g["capabilities"], &g["connection"], &g["held"]),
        (&json!(false), &Value::Null, &Value::Null, &json!(false)),
        "{g}"
    );

    let r = client()
        .post(a.url("/v1/completions"))
        .header("x-ucompute-client-ref", "remote-ref")
        .json(&json!({ "model": MODEL_ID, "prompt": PROMPT, "max_tokens": 16, "temperature": 0, "target": format!("peer:{}", idb.to_hex()) }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["usage"]["completion_tokens"], 16, "{v}");
    let after = peer_entry(&get(&a, "/api/peers").await, idb)["connection"].clone();
    assert_eq!(after["id"], conn["id"]);
    for k in ["rx_bytes", "tx_bytes"] {
        assert!(
            after[k].as_u64().unwrap() > conn[k].as_u64().unwrap(),
            "{k}: {conn} -> {after}"
        );
    }

    let st = get(&b, "/api/status").await;
    assert_eq!(st["engine"]["state"], "ready", "{st}");
    assert_eq!(
        st["loaded"]["loaded_for"],
        format!("peer:{}", ida.to_hex()),
        "B loaded the model for A: {st}"
    );
    assert_eq!(get(&a, "/api/status").await["loaded"], Value::Null, "A loaded nothing");

    let ja = get(&a, "/api/jobs").await;
    let sent = ja["recent"]
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["client_ref"] == "remote-ref")
        .cloned()
        .unwrap_or_else(|| panic!("{ja}"));
    assert_eq!(
        (
            sent["target"].clone(),
            sent["origin"].as_str(),
            sent["kind"].as_str(),
            sent["state"].as_str()
        ),
        (
            json!(format!("peer:{}", idb.to_hex())),
            Some("local"),
            Some("completion"),
            Some("done")
        ),
        "{sent}"
    );
    assert_eq!(sent["completion_tokens"], 16);
    assert!(
        sent["device_actual"].is_null(),
        "GPU/CPU of a remote run is not reported: {sent}"
    );
    let request_id = sent["request_id"].as_str().expect("request_id").to_string();
    let jb = get(&b, "/api/jobs").await;
    let served = jb["recent"]
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["kind"] == "served_for_peer")
        .cloned()
        .unwrap_or_else(|| panic!("{jb}"));
    assert_eq!(
        (served["id"].as_str(), served["request_id"].as_str()),
        (Some(request_id.as_str()), Some(request_id.as_str()))
    );
    assert_eq!(
        (
            served["origin"].clone(),
            served["target"].as_str(),
            served["state"].as_str()
        ),
        (json!(format!("peer:{}", ida.to_hex())), Some("local"), Some("done")),
        "{served}"
    );
    assert_eq!(served["completion_tokens"], 16);
    assert!(
        served["load_ms"].is_u64() && !served["device_actual"].as_str().unwrap().is_empty(),
        "{served}"
    );
    assert!(
        audit_about(&b, "remote_request", ida)
            .iter()
            .any(|e| e.fields["request_id"] == request_id.as_str()),
        "the job joins B's audit entry"
    );
    // What the app's Jobs and Activity read, on both devices: never the prompt or the reply.
    for srv in [&a, &b] {
        for path in ["/api/jobs", "/api/audit?n=1000", "/api/peers"] {
            let text = client().get(srv.url(path)).send().await.unwrap().text().await.unwrap();
            assert!(
                !text.contains(PROMPT) && !text.contains("rabbit") && !text.contains("Benny"),
                "{path}: {text}"
            );
        }
    }
    a.stop().await;
    b.stop().await;
}

/// **SINGLE-HOST** E4: a connection this device did not close is audited `peer_lost` once (B's
/// networking going off, for A); closes this device makes are not (B's own stop, A turning its
/// networking off, A revoking B). Both are pinned with no address, so no sync loop redials.
#[tokio::test(flavor = "multi_thread")]
async fn only_losses_this_device_did_not_cause_are_peer_lost_single_host() {
    let (a, _da) = empty_server().await;
    let (b, _db) = empty_server().await;
    enable_network(&a, false).await;
    let b_addr = listen_addr(&enable_network(&b, false).await);
    let (ida, idb) = (a.state.identity.id, b.state.identity.id);
    pin(&a, idb, "B", None);
    pin(&b, ida, "A", None);
    let (status, v) = connect_peer(&a, &idb.to_hex(), json!({ "addr": b_addr })).await;
    assert_eq!(status, 200, "{v}");
    wait_connected(&b, ida, true).await;

    let t0 = Instant::now();
    let (status, _) = put(&b, "/api/network", json!({ "enabled": false })).await;
    assert_eq!(status, 200);
    wait_for("A audits the loss", || async {
        (!audit_about(&a, "peer_lost", idb).is_empty()).then_some(())
    })
    .await;
    assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let lost = audit_about(&a, "peer_lost", idb);
    assert_eq!(lost.len(), 1, "{lost:?}");
    assert_eq!(
        (lost[0].fields["reason"].as_str(), lost[0].fields["addr"].as_str()),
        (Some("closed_by_peer"), Some(b_addr.as_str()))
    );
    assert!(lost[0].fields["connected_s"].is_u64(), "{lost:?}");
    assert!(
        audit_about(&b, "peer_lost", ida).is_empty(),
        "B closed it: not a loss for B"
    );

    // A turns its own networking off: a loss for B, not for A.
    let b_addr = listen_addr(&enable_network(&b, false).await);
    let (status, v) = connect_peer(&a, &idb.to_hex(), json!({ "addr": b_addr })).await;
    assert_eq!(status, 200, "{v}");
    wait_connected(&b, ida, true).await;
    let (status, _) = put(&a, "/api/network", json!({ "enabled": false })).await;
    assert_eq!(status, 200);
    wait_for("B audits the loss", || async {
        (!audit_about(&b, "peer_lost", ida).is_empty()).then_some(())
    })
    .await;

    // A revokes B: `peer_disconnected` (revoked), still no `peer_lost` on A.
    enable_network(&a, false).await;
    let (status, v) = connect_peer(&a, &idb.to_hex(), json!({ "addr": b_addr })).await;
    assert_eq!(status, 200, "{v}");
    wait_connected(&b, ida, true).await;
    let (status, _) = post(&a, &format!("/api/peers/{}/revoke", idb.to_hex()), json!({})).await;
    assert_eq!(status, 200);
    let na = a.state.node().await.unwrap();
    wait_for("A closed B", || async { na.connections().is_empty().then_some(()) }).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        audit_about(&a, "peer_lost", idb).len(),
        1,
        "no loss recorded for closes A made"
    );
    assert!(audit_about(&a, "peer_disconnected", idb)
        .iter()
        .any(|e| e.fields["reason"] == "revoked"));
    a.stop().await;
    b.stop().await;
}

/// **SINGLE-HOST** E5: A disconnects B. A shows it held at once and refuses B both ways: B's
/// dials are closed with `CLOSE_HELD` (B reads `peer_held`, never connected in between; A audits
/// one `peer_rejected` reason `held` however often B tries), and A's own request to B is refused
/// by the hold before anything is sent. Connect on A ends the hold.
#[tokio::test(flavor = "multi_thread")]
async fn disconnect_holds_a_device_both_ways_until_connect_single_host() {
    let (a, _da) = empty_server().await;
    let (b, _db) = empty_server().await;
    let a_addr = listen_addr(&enable_network(&a, true).await);
    let b_addr = listen_addr(&enable_network(&b, true).await);
    let (ida, idb) = (a.state.identity.id, b.state.identity.id);
    pin(&a, idb, "B", None);
    pin(&b, ida, "A", None);
    // A may send prompts to B, so what refuses A's request below is the hold, not the policy.
    put_group(&a, "home", &[idb], true, false).await;
    put_group(&b, "home", &[ida], true, false).await;
    let (status, _) = put(
        &a,
        "/api/privacy",
        json!({ "mode": "trusted_group", "acknowledged": true }),
    )
    .await;
    assert_eq!(status, 200);
    let (status, v) = connect_peer(&a, &idb.to_hex(), json!({ "addr": b_addr })).await;
    assert_eq!(status, 200, "{v}");
    wait_connected(&b, ida, true).await;
    assert_eq!(peer_entry(&get(&a, "/api/peers").await, idb)["held"], false);
    let b_connects_before = audit_about(&b, "peer_connected", ida).len();

    let t0 = Instant::now();
    let (status, rec) = post(&a, &format!("/api/peers/{}/disconnect", idb.short_label()), json!({})).await;
    assert_eq!(
        (status, rec["id"].as_str()),
        (200, Some(idb.to_hex().as_str())),
        "{rec}"
    );
    let pb = peer_entry(&get(&a, "/api/peers").await, idb);
    assert_eq!((&pb["connected"], &pb["held"]), (&json!(false), &json!(true)), "{pb}");
    assert!(t0.elapsed() < Duration::from_secs(1));
    assert!(audit_about(&a, "peer_disconnected", idb)
        .iter()
        .any(|e| e.fields["reason"] == "by_user"));
    wait_connected(&b, ida, false).await;

    // B tries to come back (its sync loop has no address for A; these are its redials).
    let watch = tokio::spawn({
        let url = b.url("/api/peers");
        async move {
            let t0 = Instant::now();
            while t0.elapsed() < Duration::from_secs(3) {
                let v: Value = client().get(&url).send().await.unwrap().json().await.unwrap();
                assert_eq!(
                    peer_entry(&v, ida)["connected"],
                    false,
                    "B connected to a device that holds it: {v}"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    });
    for _ in 0..3 {
        let (status, v) = connect_peer(&b, &ida.to_hex(), json!({ "addr": a_addr })).await;
        assert_eq!((status, v["error"]["code"].as_str()), (403, Some("peer_held")), "{v}");
    }
    watch.await.unwrap();
    let pa = peer_entry(&get(&b, "/api/peers").await, ida);
    assert_eq!(pa["last_error"]["code"], "peer_held", "{pa}");
    assert_eq!(
        audit_about(&b, "peer_connected", ida).len(),
        b_connects_before,
        "B never got in"
    );
    let held: Vec<_> = audit_about(&a, "peer_rejected", idb)
        .into_iter()
        .filter(|e| e.fields["reason"] == "held")
        .collect();
    assert_eq!(held.len(), 1, "one line per hold: {held:?}");

    // A's own request to B: refused by the hold, nothing sent.
    let (status, v) = post(
        &a,
        "/v1/chat/completions",
        chat_body(MODEL_ID, 4, json!({ "target": format!("peer:{}", idb.to_hex()) })),
    )
    .await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("policy_denied")),
        "{v}"
    );
    assert!(
        v["error"]["message"].as_str().unwrap().contains("disconnected by you"),
        "{v}"
    );
    let na = a.state.node().await.unwrap();
    assert!(na.connections().is_empty());
    assert!(audit_about(&a, "remote_request", idb).is_empty(), "nothing left A");

    // Connect ends the hold.
    let (status, v) = connect_peer(&a, &idb.to_hex(), json!({ "addr": b_addr })).await;
    assert_eq!(status, 200, "{v}");
    let pb = peer_entry(&get(&a, "/api/peers").await, idb);
    assert_eq!((&pb["connected"], &pb["held"]), (&json!(true), &json!(false)), "{pb}");
    wait_connected(&b, ida, true).await;
    a.stop().await;
    b.stop().await;
}

/// **SINGLE-HOST** E5: a hold belongs to the pairing it was set on. A disconnects B, removes it and
/// pairs it again (pins it): the new pairing is not held, so Connect is not needed to undo a hold
/// the user can no longer see, and A's sync loop and B's dials are admitted.
#[tokio::test(flavor = "multi_thread")]
async fn unpairing_ends_a_hold_single_host() {
    let (a, _da) = empty_server().await;
    let (b, _db) = empty_server().await;
    enable_network(&a, true).await;
    let b_addr = listen_addr(&enable_network(&b, true).await);
    let (ida, idb) = (a.state.identity.id, b.state.identity.id);
    pin(&a, idb, "B", None);
    pin(&b, ida, "A", None);
    let (status, v) = connect_peer(&a, &idb.to_hex(), json!({ "addr": b_addr })).await;
    assert_eq!(status, 200, "{v}");
    let (status, _) = post(&a, &format!("/api/peers/{}/disconnect", idb.to_hex()), json!({})).await;
    assert_eq!(status, 200);
    assert_eq!(peer_entry(&get(&a, "/api/peers").await, idb)["held"], true);
    let r = client()
        .delete(a.url(&format!("/api/peers/{}", idb.to_hex())))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    pin(&a, idb, "B", None);
    assert_eq!(peer_entry(&get(&a, "/api/peers").await, idb)["held"], false);
    let (status, v) = connect_peer(&a, &idb.to_hex(), json!({ "addr": b_addr })).await;
    assert_eq!(status, 200, "{v}");
    a.stop().await;
    b.stop().await;
}

/// **SINGLE-HOST** E5: a settings change that keeps networking on (Sharing, which restarts the
/// endpoint) keeps a hold: A still shows B held and the new endpoint still refuses it. Turning
/// networking off and on ends it, as documented.
#[tokio::test(flavor = "multi_thread")]
async fn a_sharing_change_keeps_a_hold_and_networking_off_ends_it_single_host() {
    let (a, _da) = empty_server().await;
    let (b, _db) = empty_server().await;
    enable_network(&a, false).await;
    enable_network(&b, true).await;
    let (ida, idb) = (a.state.identity.id, b.state.identity.id);
    pin(&a, idb, "B", None);
    pin(&b, ida, "A", None);
    let (status, _) = post(&a, &format!("/api/peers/{}/disconnect", idb.to_hex()), json!({})).await;
    assert_eq!(status, 200);
    for serve_peers in [true, false] {
        let a_addr = listen_addr(&enable_network(&a, serve_peers).await);
        let pb = peer_entry(&get(&a, "/api/peers").await, idb);
        assert_eq!(pb["held"], true, "after Sharing {serve_peers}: {pb}");
        let (status, v) = connect_peer(&b, &ida.to_hex(), json!({ "addr": a_addr })).await;
        assert_eq!((status, v["error"]["code"].as_str()), (403, Some("peer_held")), "{v}");
    }
    let (status, v) = put(&a, "/api/network", json!({ "enabled": false })).await;
    assert_eq!(status, 200, "{v}");
    enable_network(&a, false).await;
    assert_eq!(peer_entry(&get(&a, "/api/peers").await, idb)["held"], false);
    a.stop().await;
    b.stop().await;
}

/// **SINGLE-HOST** E5 + ADR-0003 D7: a hold stops work and connections, never revocations. A holds
/// B, then A revokes C and B revokes D (both share a group with A and B on each side). B's next dial
/// is still refused (`peer_held`, never connected), but on it each side receives and applies the
/// other's revocation.
#[tokio::test(flavor = "multi_thread")]
async fn revocations_cross_a_hold_both_ways_single_host() {
    let (a, _da) = empty_server().await;
    let (b, _db) = empty_server().await;
    let a_addr = listen_addr(&enable_network(&a, true).await);
    enable_network(&b, true).await;
    let (ida, idb) = (a.state.identity.id, b.state.identity.id);
    let (idc, idd) = (
        DeviceIdentity::generate().unwrap().id,
        DeviceIdentity::generate().unwrap().id,
    );
    pin(&a, idb, "B", None);
    pin(&b, ida, "A", None);
    for (srv, other) in [(&a, idb), (&b, ida)] {
        pin(srv, idc, "C", None);
        pin(srv, idd, "D", None);
        put_group(srv, "home", &[other, idc, idd], false, false).await;
    }
    let (status, _) = post(&a, &format!("/api/peers/{}/disconnect", idb.to_hex()), json!({})).await;
    assert_eq!(status, 200);
    let (status, v) = post(&a, &format!("/api/peers/{}/revoke", idc.to_hex()), json!({})).await;
    assert_eq!(status, 200, "{v}");
    let (status, v) = post(&b, &format!("/api/peers/{}/revoke", idd.to_hex()), json!({})).await;
    assert_eq!(status, 200, "{v}");
    let b_connects_before = audit_about(&b, "peer_connected", ida).len();

    let (status, v) = connect_peer(&b, &ida.to_hex(), json!({ "addr": a_addr })).await;
    assert_eq!((status, v["error"]["code"].as_str()), (403, Some("peer_held")), "{v}");
    wait_for("B applies A's revocation of C", || async {
        (b.state.peers.is_revoked(&idc) && !b.state.peers.is_pinned(&idc)).then_some(())
    })
    .await;
    wait_for("A applies B's revocation of D", || async {
        (a.state.peers.is_revoked(&idd) && !a.state.peers.is_pinned(&idd)).then_some(())
    })
    .await;
    assert!(audit_about(&a, "peer_revoked", idd)
        .iter()
        .any(|e| e.fields["source"] == "peer"));
    assert_eq!(
        peer_entry(&get(&a, "/api/peers").await, idb)["held"],
        true,
        "still held"
    );
    assert_eq!(peer_entry(&get(&b, "/api/peers").await, ida)["connected"], false);
    assert_eq!(
        audit_about(&b, "peer_connected", ida).len(),
        b_connects_before,
        "B never got in"
    );
    a.stop().await;
    b.stop().await;
}

/// **SINGLE-HOST**: a refusal only the device's own key can send (here `peer_held`) ends the dial.
/// A stale pinned address tried after it would fail later with "no answer" and become the
/// device's `last_error`, hiding why it is refused. A's Node is started by hand (networking stays
/// off in A's settings), so no sync loop races this dial.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_refusal_ends_the_dial_single_host() {
    let (a, _da) = empty_server().await;
    let (b, _db) = empty_server().await;
    let b_addr: std::net::SocketAddr = listen_addr(&enable_network(&b, true).await).parse().unwrap();
    let stale = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let (ida, idb) = (a.state.identity.id, b.state.identity.id);
    pin(&a, idb, "B", Some(stale.local_addr().unwrap().to_string()));
    pin(&b, ida, "A", None);
    let (status, _) = post(&b, &format!("/api/peers/{}/disconnect", ida.to_hex()), json!({})).await;
    assert_eq!(status, 200);
    let na = ucompute_network::transport::Node::start(
        NetworkConfig {
            enabled: true,
            ..cfg().network
        },
        a.state.identity.clone(),
        a.state.peers.clone(),
        a.state.policy(),
        a.state.audit.clone(),
    )
    .await
    .unwrap();

    let e = a.state.peer_connection_via(&na, idb, Some(b_addr)).await.unwrap_err();
    assert_eq!(e.code, ucompute_core::ErrorCode::PeerHeld, "{e}");
    assert_eq!(
        na.dial_failure(&idb).map(|f| (f.addr, f.code)),
        Some((b_addr, ucompute_core::ErrorCode::PeerHeld))
    );
    let mut buf = [0u8; 2048];
    assert!(
        tokio::time::timeout(Duration::from_millis(300), stale.recv_from(&mut buf))
            .await
            .is_err(),
        "the stale pinned address was not dialled"
    );
    na.shutdown().await;
    a.stop().await;
    b.stop().await;
}

/// The peer list never depends on the model store: an unreadable manifest only costs peer model
/// ids (catalog or null), never `/api/peers`, which Devices, Chat and pairing all read.
#[tokio::test(flavor = "multi_thread")]
async fn an_unreadable_manifest_does_not_fail_the_peer_list() {
    let (a, _da) = empty_server().await;
    let dir = a.state.paths.model_manifests();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("broken.json"), b"{ not json").unwrap();
    assert!(a.state.store.list().is_err(), "the store itself refuses it");
    pin(&a, ucompute_identity::DeviceIdentity::generate().unwrap().id, "B", None);
    let v = get(&a, "/api/peers").await;
    assert_eq!(v["peers"].as_array().unwrap().len(), 1, "{v}");
    a.stop().await;
}

/// A request to a paired device reads `connecting` (not started, no request id) while this device
/// is still dialling it: it takes no slot or load here, so it is never `queued` ("Waiting for a
/// slot") or `loading`.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_to_a_device_being_dialled_is_connecting_single_host() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = cfg();
    config.network.idle_timeout_ms = 1000;
    let a = Server::start(app(config, dir.path().to_path_buf(), HardwareReport::default())).await;
    // Nothing answers there: a handshake waits about three initial probe timeouts (~3 s).
    let silent = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let x = DeviceIdentity::generate().unwrap().id;
    pin(&a, x, "X", Some(silent.local_addr().unwrap().to_string()));
    enable_network(&a, false).await;
    let (status, v) = put(
        &a,
        "/api/privacy",
        json!({ "mode": "trusted_group", "acknowledged": true }),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    put_group(&a, "home", &[x], true, false).await;
    let req = tokio::spawn(
        client()
            .post(a.url("/v1/chat/completions"))
            .json(&chat_body(
                MODEL_ID,
                4,
                json!({ "target": format!("peer:{}", x.to_hex()) }),
            ))
            .send(),
    );
    let job = wait_for("the request's row", || async {
        get(&a, "/api/jobs").await["active"]
            .as_array()
            .unwrap()
            .first()
            .cloned()
    })
    .await;
    assert_eq!(
        (
            job["state"].as_str(),
            job["target"].clone(),
            &job["started_at_utc"],
            &job["request_id"]
        ),
        (
            Some("connecting"),
            json!(format!("peer:{}", x.to_hex())),
            &Value::Null,
            &Value::Null
        ),
        "{job}"
    );
    let r = req.await.unwrap().unwrap();
    assert_eq!(r.status(), 502, "nothing answered: peer_lost");
    let recent = get(&a, "/api/jobs").await["recent"][0].clone();
    assert_eq!(
        (recent["state"].as_str(), recent["error_code"].as_str()),
        (Some("error"), Some("peer_lost")),
        "{recent}"
    );
    a.stop().await;
}

// ------------------------------------------- D-026: local work first on CPU engines

/// [`serving_app`] whose probe reports every GPU backend unusable, so the engine runs on the CPU
/// (`device_actual == "cpu"`), as on a CPU-only host. The engine, model and inference are real.
fn cpu_serving_app(home: &std::path::Path, dir: &std::path::Path, config: ApiConfig) -> Arc<AppState> {
    let real = Paths::at(home);
    let backend = ucompute_runtime::backend_by_name("llama.cpp", engine_dir(&real), real.clone()).unwrap();
    let mut hw = ucompute_hardware::probe().unwrap();
    for b in &mut hw.backends {
        if b.name != "cpu" {
            b.available = false;
        }
    }
    let manager =
        ucompute_runtime::manager::ModelManager::new(real.clone(), ucompute_artifacts::Store::new(real), backend, hw);
    app_with(config, Paths::at(dir), Arc::new(manager))
}

/// The model these tests generate with: qwen2.5-0.5b, long enough on the CPU (several seconds for
/// a few hundred tokens) that "while a local generation runs" is a real interval.
const LP_MODEL: &str = CHAT_MODEL_ID;
const LP_PROMPT: &str = "Write a long story about a dragon who learns to bake bread.";
const BUSY: &str = "busy with its own work";

/// A and B paired (SINGLE-HOST), B serving with two engine slots (one for peers), A in trusted
/// mode; B's local priority set to `mode` through `PUT /api/network` (left at the default when
/// None). `cpu`: B's engine runs on the CPU, else on what the probe offers (Metal here).
async fn priority_pair(
    home: &std::path::Path,
    mode: Option<&str>,
    cpu: bool,
) -> (Server, Server, PeerId, [tempfile::TempDir; 2]) {
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = Server::start(app(cfg(), da.path().to_path_buf(), HardwareReport::default())).await;
    let b_app = if cpu {
        cpu_serving_app(home, db.path(), serving_cfg())
    } else {
        serving_app(home, db.path(), serving_cfg())
    };
    let b = Server::start(b_app).await;
    let net_b = enable_network(&b, true).await;
    assert_eq!(net_b["local_priority"], "refuse_new", "the default: {net_b}");
    if let Some(mode) = mode {
        let (status, v) = put(&b, "/api/network", json!({ "local_priority": mode })).await;
        assert_eq!((status, v["local_priority"].as_str()), (200, Some(mode)), "{v}");
        assert_eq!(
            v["listen_addr"], net_b["listen_addr"],
            "a priority change does not restart the endpoint"
        );
    }
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
    // Load B's engine with a local request and wait out the window it opens.
    let (status, v) = post(
        &b,
        "/v1/completions",
        json!({ "model": LP_MODEL, "prompt": LP_PROMPT, "max_tokens": 4, "temperature": 0 }),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let device = b.state.manager.status().await.unwrap().device_actual;
    eprintln!("SINGLE-HOST D-026: B's engine runs on {device:?} (local_priority {mode:?})");
    tokio::time::sleep(ucompute_network::local_priority::WINDOW + Duration::from_millis(300)).await;
    (a, b, idb, [da, db])
}

fn lp_body(max_tokens: u32, stream: bool, target: Option<PeerId>) -> Value {
    let mut v =
        json!({ "model": LP_MODEL, "prompt": LP_PROMPT, "max_tokens": max_tokens, "temperature": 0, "stream": stream });
    if let Some(id) = target {
        v["target"] = json!(format!("peer:{}", id.to_hex()));
    }
    v
}

/// A streaming generation under way: returns once its first token arrived; the task yields the
/// whole SSE body and the instant it ended, `ended` is set at that instant.
struct Running {
    task: tokio::task::JoinHandle<(String, Instant)>,
    ended: Arc<std::sync::atomic::AtomicBool>,
}

async fn start_running(url: String, body: Value) -> Running {
    let mut r = client().post(url).json(&body).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let mut text = String::new();
    while !text.contains("\"choices\"") {
        let c = r
            .chunk()
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("stream ended before its first token: {text}"));
        text.push_str(&String::from_utf8_lossy(&c));
    }
    let ended = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = ended.clone();
    let task = tokio::spawn(async move {
        while let Some(c) = r.chunk().await.unwrap() {
            text.push_str(&String::from_utf8_lossy(&c));
        }
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
        (text, Instant::now())
    });
    Running { task, ended }
}

fn assert_busy(status: u16, v: &Value) {
    assert_eq!((status, v["error"]["code"].as_str()), (429, Some("overloaded")), "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains(BUSY), "{v}");
}

/// D-026 B, the default, on a real CPU engine (SINGLE-HOST): a peer generation already running
/// when local work starts finishes; a new one is refused (typed, retryable `overloaded`) while a
/// local generation runs and within 2 s after it ends, and accepted after that.
#[tokio::test(flavor = "multi_thread")]
async fn local_priority_refuse_new_on_a_cpu_engine_single_host() {
    let Some(home) = fixture_home() else { return };
    if !has_model(&home, LP_MODEL) {
        assert!(
            std::env::var_os("UCOMPUTE_REQUIRE_FIXTURES").is_none_or(|v| v != "1"),
            "{LP_MODEL} missing"
        );
        return eprintln!("SKIP: {LP_MODEL} missing");
    }
    let _engine = ENGINE_TESTS.lock().await;
    let (a, b, idb, _dirs) = priority_pair(&home, None, true).await;
    assert_eq!(b.state.manager.status().await.unwrap().device_actual, "cpu");

    // A running peer generation finishes although local work starts beside it.
    let peer = start_running(a.url("/v1/completions"), lp_body(64, true, Some(idb))).await;
    let (status, v) = post(&b, "/v1/completions", lp_body(8, false, None)).await;
    assert_eq!(status, 200, "local requests are never refused: {v}");
    let (text, _) = peer.task.await.unwrap();
    assert!(
        text.contains("\"finish_reason\":\"") && !text.contains("\"error\""),
        "the peer generation finished: {text}"
    );
    tokio::time::sleep(ucompute_network::local_priority::WINDOW + Duration::from_millis(300)).await;

    // During a local generation: refused.
    let local = start_running(b.url("/v1/completions"), lp_body(400, true, None)).await;
    let t0 = Instant::now();
    let (status, v) = post(&a, "/v1/completions", lp_body(8, false, Some(idb))).await;
    let during = !local.ended.load(std::sync::atomic::Ordering::SeqCst);
    eprintln!(
        "SINGLE-HOST D-026 B: peer request during local work: {status} in {:?} (local still running: {during})",
        t0.elapsed()
    );
    assert_busy(status, &v);
    assert!(during, "the refusal came while the local generation ran");

    // Within 2 s after it ended: refused.
    let (body, end) = local.task.await.unwrap();
    assert!(body.contains("[DONE]"), "{body}");
    let (status, v) = post(&a, "/v1/completions", lp_body(8, false, Some(idb))).await;
    let after = end.elapsed();
    eprintln!("SINGLE-HOST D-026 B: peer request {after:?} after local work ended: {status}");
    assert!(after < ucompute_network::local_priority::WINDOW, "{after:?}");
    assert_busy(status, &v);

    // After the window: accepted.
    tokio::time::sleep(
        (ucompute_network::local_priority::WINDOW + Duration::from_millis(300)).saturating_sub(end.elapsed()),
    )
    .await;
    let (status, v) = post(&a, "/v1/completions", lp_body(8, false, Some(idb))).await;
    assert_eq!(status, 200, "after the window: {v}");
    assert!(!v["choices"][0]["text"].as_str().unwrap().is_empty(), "{v}");
    a.stop().await;
    b.stop().await;
}

/// D-026 A, opt-in, on a real CPU engine (SINGLE-HOST): local work starting cancels the running
/// peer generation, which ends with the typed `overloaded`; the local request completes.
#[tokio::test(flavor = "multi_thread")]
async fn local_priority_preempt_cancels_a_running_peer_generation_on_a_cpu_engine_single_host() {
    let Some(home) = fixture_home() else { return };
    if !has_model(&home, LP_MODEL) {
        assert!(
            std::env::var_os("UCOMPUTE_REQUIRE_FIXTURES").is_none_or(|v| v != "1"),
            "{LP_MODEL} missing"
        );
        return eprintln!("SKIP: {LP_MODEL} missing");
    }
    let _engine = ENGINE_TESTS.lock().await;
    let (a, b, idb, _dirs) = priority_pair(&home, Some("preempt"), true).await;
    assert_eq!(b.state.manager.status().await.unwrap().device_actual, "cpu");
    let peer = start_running(a.url("/v1/completions"), lp_body(400, true, Some(idb))).await;
    let (status, v) = post(&b, "/v1/completions", lp_body(8, false, None)).await;
    assert_eq!(status, 200, "the local request: {v}");
    let (text, _) = peer.task.await.unwrap();
    eprintln!(
        "SINGLE-HOST D-026 A: the peer stream ended with: {}",
        text.lines().rev().find(|l| l.contains("error")).unwrap_or("(no error)")
    );
    assert!(
        text.contains("\"code\":\"overloaded\"") && text.contains(BUSY),
        "{text}"
    );
    assert!(
        !text.contains("\"finish_reason\":\""),
        "cancelled, not finished: {text}"
    );
    // B's own row for the served request says so too.
    let jobs = get(&b, "/api/jobs").await;
    let served = jobs["recent"]
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["kind"] == "served_for_peer")
        .unwrap();
    assert_eq!(served["error_code"], "overloaded", "{served}");
    a.stop().await;
    b.stop().await;
}

/// D-026 C on a real CPU engine (SINGLE-HOST): no rule, a peer generation is accepted during local work.
#[tokio::test(flavor = "multi_thread")]
async fn local_priority_none_accepts_peer_work_during_local_work_on_a_cpu_engine_single_host() {
    let Some(home) = fixture_home() else { return };
    if !has_model(&home, LP_MODEL) {
        assert!(
            std::env::var_os("UCOMPUTE_REQUIRE_FIXTURES").is_none_or(|v| v != "1"),
            "{LP_MODEL} missing"
        );
        return eprintln!("SKIP: {LP_MODEL} missing");
    }
    let _engine = ENGINE_TESTS.lock().await;
    let (a, b, idb, _dirs) = priority_pair(&home, Some("none"), true).await;
    assert_eq!(b.state.manager.status().await.unwrap().device_actual, "cpu");
    let local = start_running(b.url("/v1/completions"), lp_body(400, true, None)).await;
    let (status, v) = post(&a, "/v1/completions", lp_body(8, false, Some(idb))).await;
    assert_eq!(status, 200, "{v}");
    let (body, _) = local.task.await.unwrap();
    assert!(body.contains("\"finish_reason\":\""), "{body}");
    a.stop().await;
    b.stop().await;
}

/// D-026 leaves GPU engines alone (SINGLE-HOST, Metal on this Mac): with the default `refuse_new`,
/// a peer generation is accepted while a local generation runs. Skips loudly on a host whose
/// engine runs on the CPU.
#[tokio::test(flavor = "multi_thread")]
async fn local_priority_leaves_a_gpu_engine_alone_single_host() {
    let Some(home) = fixture_home() else { return };
    if !has_model(&home, LP_MODEL) {
        assert!(
            std::env::var_os("UCOMPUTE_REQUIRE_FIXTURES").is_none_or(|v| v != "1"),
            "{LP_MODEL} missing"
        );
        return eprintln!("SKIP: {LP_MODEL} missing");
    }
    let _engine = ENGINE_TESTS.lock().await;
    let (a, b, idb, _dirs) = priority_pair(&home, None, false).await;
    let device = b.state.manager.status().await.unwrap().device_actual;
    if device == "cpu" {
        eprintln!("SKIP: no GPU engine on this host");
        a.stop().await;
        b.stop().await;
        return;
    }
    let local = start_running(b.url("/v1/completions"), lp_body(400, true, None)).await;
    let (status, v) = post(&a, "/v1/completions", lp_body(8, false, Some(idb))).await;
    let during = !local.ended.load(std::sync::atomic::Ordering::SeqCst);
    eprintln!(
        "SINGLE-HOST D-026 on {device}: peer request during local work: {status} (local still running: {during})"
    );
    assert_eq!(status, 200, "{v}");
    assert!(during, "accepted while the local generation ran");
    local.task.await.unwrap();
    a.stop().await;
    b.stop().await;
}

/// D-026 setting, no engine: `refuse_new` when `config.json` has none; unknown values refused with
/// `invalid_request`; a change is persisted, applied at once without restarting the endpoint, and
/// read back after a daemon restart (in process, same `UCOMPUTE_HOME`); a file written by another
/// process applies at the next read.
#[tokio::test(flavor = "multi_thread")]
async fn local_priority_setting_validates_persists_and_survives_a_restart() {
    use ucompute_network::local_priority::LocalPriority;
    let dir = tempfile::tempdir().unwrap();
    let srv = Server::start(app(cfg(), dir.path().to_path_buf(), HardwareReport::default())).await;
    assert_eq!(get(&srv, "/api/network").await["local_priority"], "refuse_new");
    assert_eq!(srv.state.local_work.mode(), LocalPriority::RefuseNew);
    let on = enable_network(&srv, false).await;
    for bad in [json!("always"), json!(""), json!(1), json!(null)] {
        let (status, v) = put(&srv, "/api/network", json!({ "local_priority": bad })).await;
        if bad.is_null() {
            assert_eq!(status, 200, "null leaves it unchanged: {v}");
            continue;
        }
        assert_eq!(
            (status, v["error"]["code"].as_str()),
            (400, Some("invalid_request")),
            "{bad}: {v}"
        );
    }
    let (status, v) = put(&srv, "/api/network", json!({ "local_priority": "preempt" })).await;
    assert_eq!((status, v["local_priority"].as_str()), (200, Some("preempt")), "{v}");
    assert_eq!(v["listen_addr"], on["listen_addr"], "not restarted: {v}");
    assert_eq!(v["enabled"], true);
    assert_eq!(srv.state.local_work.mode(), LocalPriority::Preempt);
    assert!(audit_kinds(&get(&srv, "/api/audit").await).contains(&"local_priority_changed".to_string()));
    let file: Value = serde_json::from_slice(&std::fs::read(dir.path().join("config.json")).unwrap()).unwrap();
    assert_eq!(file["local_priority"], "preempt", "{file}");
    srv.stop().await;

    let srv = Server::start(app(cfg(), dir.path().to_path_buf(), HardwareReport::default())).await;
    assert_eq!(
        srv.state.local_work.mode(),
        LocalPriority::Preempt,
        "read back at start"
    );
    assert_eq!(get(&srv, "/api/network").await["local_priority"], "preempt");
    // Another process (the file) sets C; the next read applies it.
    let mut s = Settings::load(&Paths::at(dir.path())).unwrap().unwrap();
    s.local_priority = LocalPriority::None;
    std::thread::sleep(Duration::from_millis(20)); // a new mtime
    s.save(&Paths::at(dir.path())).unwrap();
    assert_eq!(get(&srv, "/api/network").await["local_priority"], "none");
    assert_eq!(srv.state.local_work.mode(), LocalPriority::None);
    srv.stop().await;
}
