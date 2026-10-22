//! The desktop app's golden API fixtures (`apps/desktop/src/test/fixtures/api/*.json`, written by
//! the UI side from the UI 2.0 plan §4, read here and never written) against real responses: every
//! key a fixture has must exist in the response with the same JSON type, recursively. Values are
//! ignored. A `null` in the fixture matches anything; a `null` in the response matches a fixture
//! scalar (a value not known right now) but never a fixture object or array, whose shape would then
//! go unchecked. A fixture array element must match some response element (rows differ by state),
//! so each test produces a real row for every kind of fixture row, and a fixture array the response
//! has no element for is a failure unless the test names it with the reason. Keys the response has
//! and a fixture lacks are printed, not failed. A mismatch is a contract drift between the daemon
//! and the app, settled between the two sides, never by editing the other's file.
//!
//! Status and jobs use the FAKE `LoadedModel` double in the slot; peers and audit run two daemons
//! in this process (**SINGLE-HOST**, loopback, no engine).

mod common;

use common::*;
use common_api::ApiConfig;
use common_hardware::HardwareReport;
use common_identity::{PeerId, PeerRecord};
use common_network::NetworkConfig;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn fixture(name: &str) -> Value {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../apps/desktop/src/test/fixtures/api/{name}.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Mismatches of `real` against `fixture` (see the module docs), as `path: why`; `extra` collects
/// response keys the fixture lacks; `empty` the fixture arrays the response had no element for.
fn shape(
    path: &str,
    fixture: &Value,
    real: &Value,
    out: &mut Vec<String>,
    extra: &mut Vec<String>,
    empty: &mut Vec<String>,
) {
    match (fixture, real) {
        (Value::Null, _) => {}
        (Value::Object(_) | Value::Array(_), Value::Null) => {
            out.push(format!("{path}: fixture has {}, response has null", kind(fixture)))
        }
        (_, Value::Null) => {}
        (Value::Object(f), Value::Object(r)) => {
            for (k, fv) in f {
                match r.get(k) {
                    Some(rv) => shape(&format!("{path}.{k}"), fv, rv, out, extra, empty),
                    None => out.push(format!("{path}.{k}: missing from the response")),
                }
            }
            extra.extend(r.keys().filter(|k| !f.contains_key(*k)).map(|k| format!("{path}.{k}")));
        }
        // Each fixture element must have the shape of some response element (rows differ by state);
        // of the matches, the one that left the fewest arrays unchecked counts. ponytail: a real
        // null still matches a fixture scalar, so a scalar's type drift in a field that is null in
        // some real row can pass; pair rows by a discriminator if that ever hides a drift.
        (Value::Array(f), Value::Array(r)) if !r.is_empty() => {
            for (i, fv) in f.iter().enumerate() {
                let tries: Vec<(Vec<String>, Vec<String>)> = r
                    .iter()
                    .map(|rv| {
                        let (mut e, mut s) = (vec![], vec![]);
                        shape(&format!("{path}[{i}]"), fv, rv, &mut e, &mut vec![], &mut s);
                        (e, s)
                    })
                    .collect();
                match tries.iter().filter(|(e, _)| e.is_empty()).min_by_key(|(_, s)| s.len()) {
                    Some((_, s)) => empty.extend(s.iter().cloned()),
                    None => out.extend(
                        tries
                            .into_iter()
                            .map(|(e, _)| e)
                            .min_by_key(Vec::len)
                            .unwrap_or_default(),
                    ),
                }
            }
            let mut e = vec![];
            shape(
                &format!("{path}[0]"),
                &f.first().cloned().unwrap_or(Value::Null),
                &r[0],
                &mut e,
                extra,
                &mut vec![],
            );
        }
        (Value::Array(f), Value::Array(_)) => {
            if !f.is_empty() {
                empty.push(path.to_string());
            }
        }
        (f, r) if kind(f) == kind(r) => {}
        (f, r) => out.push(format!("{path}: fixture has {}, response has {}", kind(f), kind(r))),
    }
}

/// `allow_empty`: fixture array paths this test cannot give a real element, each with its reason
/// at the call site.
fn assert_shape(name: &str, fixture: &Value, real: &Value, allow_empty: &[&str]) {
    let (mut out, mut extra, mut empty) = (vec![], vec![], vec![]);
    shape("", fixture, real, &mut out, &mut extra, &mut empty);
    extra.sort();
    extra.dedup();
    if !extra.is_empty() {
        eprintln!("{name}: response keys the fixture does not have: {extra:?}");
    }
    assert!(
        out.is_empty(),
        "{name} drifted from the daemon:\n{}\nresponse: {real}",
        out.join("\n")
    );
    empty.sort();
    empty.dedup();
    empty.retain(|p| !allow_empty.contains(&p.as_str()));
    assert!(
        empty.is_empty(),
        "{name}: nothing in the response to compare these fixture arrays against (produce a real row): {empty:?}\nresponse: {real}"
    );
}

#[test]
fn the_shape_check_itself() {
    let f = json!({ "a": 1, "b": null, "c": { "d": "x" }, "e": [{ "f": true }, { "f": null, "g": 1 }] });
    let ok = json!({ "a": 2.5, "b": "anything", "c": { "d": "y", "z": 0 }, "e": [{ "f": false, "g": null }] });
    let (mut out, mut extra, mut empty) = (vec![], vec![], vec![]);
    shape("", &f, &ok, &mut out, &mut extra, &mut empty);
    assert!(out.is_empty(), "{out:?}");
    assert_eq!(extra, [".c.z", ".e[0].g"]);
    assert!(empty.is_empty());
    for bad in [
        json!({ "a": "1", "b": null, "c": { "d": "x" }, "e": [] }),
        json!({ "b": null, "a": 1, "c": {}, "e": [] }),
        json!({ "a": 1, "b": null, "c": { "d": "x" }, "e": [{ "f": 1, "g": 1 }] }),
        // A response null never stands in for a fixture object: its shape would go unchecked.
        json!({ "a": 1, "b": null, "c": null, "e": [{ "f": true }] }),
    ] {
        let mut out = vec![];
        shape("", &f, &bad, &mut out, &mut vec![], &mut vec![]);
        assert!(!out.is_empty(), "{bad}");
    }
    // A populated fixture row is never matched by a sparse real one (the old any-element hole): the
    // connected row must find a real row with an object there.
    let rows = json!([{ "caps": { "n": 1 } }, { "caps": null }]);
    let mut out = vec![];
    shape(
        "",
        &rows,
        &json!([{ "caps": null }]),
        &mut out,
        &mut vec![],
        &mut vec![],
    );
    assert_eq!(out, ["[0].caps: fixture has object, response has null"]);
    // A fixture array with nothing to compare against is reported, by path.
    let mut empty = vec![];
    shape(
        "",
        &json!({ "e": [1], "x": [] }),
        &json!({ "e": [], "x": [] }),
        &mut vec![],
        &mut vec![],
        &mut empty,
    );
    assert_eq!(empty, [".e"]);
}

async fn get(srv: &Server, path: &str) -> Value {
    let r = client().get(srv.url(path)).send().await.unwrap();
    assert_eq!(r.status(), 200, "{path}");
    r.json().await.unwrap()
}

async fn post(srv: &Server, path: &str, body: Value) -> (u16, Value) {
    let r = client().post(srv.url(path)).json(&body).send().await.unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(Value::Null))
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
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn status_matches_the_desktop_fixture_fake() {
    let dir = tempfile::tempdir().unwrap();
    let srv = Server::start(app(
        ApiConfig::default(),
        dir.path().to_path_buf(),
        HardwareReport::default(),
    ))
    .await;
    seed_slot(&srv.state, MODEL_ID, FakeModel::new(1, Duration::ZERO)).await;
    assert_shape("status_e2", &fixture("status_e2"), &get(&srv, "/api/status").await, &[]);
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jobs_match_the_desktop_fixture_fake() {
    let dir = tempfile::tempdir().unwrap();
    let srv = Server::start(app(
        ApiConfig::default(),
        dir.path().to_path_buf(),
        HardwareReport::default(),
    ))
    .await;
    seed_slot(&srv.state, MODEL_ID, FakeModel::new(40, Duration::from_millis(25))).await;
    let body = json!({ "model": MODEL_ID, "prompt": "a b", "max_tokens": 40, "stream": true });
    let done = client()
        .post(srv.url("/v1/completions"))
        .json(&json!({ "model": MODEL_ID, "prompt": "a b", "max_tokens": 2 }))
        .header("x-common-client-ref", "done-ref")
        .send()
        .await
        .unwrap();
    assert_eq!(done.status(), 200);
    let mut running = client()
        .post(srv.url("/v1/completions"))
        .header("x-common-client-ref", "running-ref")
        .json(&body)
        .send()
        .await
        .unwrap();
    running.chunk().await.unwrap();
    let v = get(&srv, "/api/jobs").await;
    assert_eq!(
        (
            v["active"].as_array().unwrap().len(),
            v["recent"].as_array().unwrap().len()
        ),
        (1, 1),
        "{v}"
    );
    assert_shape("jobs_e3", &fixture("jobs_e3"), &v, &[]);
    drop(running);
    srv.stop().await;
}

/// Loopback peer endpoint on an ephemeral port, no mDNS.
fn net_cfg() -> ApiConfig {
    ApiConfig {
        network: NetworkConfig {
            listen_port: 0,
            bind_addr: "127.0.0.1".into(),
            mdns: false,
            ..NetworkConfig::default()
        },
        ..ApiConfig::default()
    }
}

fn pin(srv: &Server, id: PeerId, name: &str) {
    srv.state
        .peers
        .pin(PeerRecord {
            id,
            name: name.into(),
            paired_at_utc: common_identity::utc_now(),
            last_seen_utc: None,
            last_addr: None,
        })
        .unwrap();
}

async fn listening(srv: &Server) -> String {
    let r = client()
        .put(srv.url("/api/network"))
        .json(&json!({ "enabled": true, "serve_peers": false }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    r.json::<Value>().await.unwrap()["listen_addr"]
        .as_str()
        .unwrap()
        .to_string()
}

/// A store manifest (no blob: only the cached-model list in Capabilities and id resolution read it).
fn manifest(id: &str, sha: &str) -> common_artifacts::ModelManifest {
    common_artifacts::ModelManifest {
        schema_version: common_artifacts::ModelManifest::SCHEMA_VERSION,
        id: id.into(),
        name: id.into(),
        file_name: format!("{id}.gguf"),
        sha256: sha.into(),
        bytes: 1,
        license: "test".into(),
        license_url: None,
        source_url: "test".into(),
        redistribution: "unknown".into(),
        imported_at_utc: common_identity::utc_now(),
        chat: false,
        tags: vec![],
    }
}

/// **SINGLE-HOST**: A connects to B (`peers_e1`: B connected, in a group, with two cached models,
/// one A knows and one it does not; a paired device whose dial failed; a revoked one), then
/// disconnects it, and B's redial is refused (`peers_e5`, A's and B's rows together: A holds B,
/// B was refused by A); the audit lines this produces (`peer_connected`, `peer_disconnected`
/// by_user, `peer_rejected` held, B's `peer_lost`) against `audit_e4`.
#[tokio::test(flavor = "multi_thread")]
async fn peers_and_audit_match_the_desktop_fixtures_single_host() {
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = Server::start(app(net_cfg(), da.path().to_path_buf(), HardwareReport::default())).await;
    let b = Server::start(app(net_cfg(), db.path().to_path_buf(), HardwareReport::default())).await;
    // Before networking starts: the cached-model list is read then.
    let only_b = "ab".repeat(32);
    for (srv, m) in [
        (&a, manifest(MODEL_ID, MODEL_SHA)),
        (&b, manifest(MODEL_ID, MODEL_SHA)),
        (&b, manifest("only-on-b", &only_b)),
    ] {
        srv.state.store.write_manifest(&m).unwrap();
    }
    let a_addr = listening(&a).await;
    let b_addr = listening(&b).await;
    let (ida, idb) = (a.state.identity.id, b.state.identity.id);
    pin(&a, idb, "B");
    pin(&b, ida, "A");
    let r = client()
        .put(a.url("/api/groups"))
        .json(&json!({ "name": "home", "members": [idb.to_hex()], "policy": { "allow_remote_inference": false, "allow_pooled_inference": false } }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    // Ghost never connects; its dial fails (nothing listens on that loopback port), so its row has
    // `last_error` like the fixture's Laptop.
    let ghost = common_identity::DeviceIdentity::generate().unwrap().id;
    pin(&a, ghost, "Ghost");
    let dead = std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let (status, v) = post(
        &a,
        &format!("/api/peers/{}/connect", ghost.to_hex()),
        json!({ "addr": dead.to_string() }),
    )
    .await;
    assert_ne!(status, 200, "{v}");
    let gone = common_identity::DeviceIdentity::generate().unwrap().id;
    pin(&a, gone, "Gone");
    let (status, v) = post(&a, &format!("/api/peers/{}/revoke", gone.to_hex()), json!({})).await;
    assert_eq!(status, 200, "{v}");
    let (status, v) = post(
        &a,
        &format!("/api/peers/{}/connect", idb.to_hex()),
        json!({ "addr": b_addr }),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let peers = get(&a, "/api/peers").await;
    assert_eq!(
        peers["peers"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["capabilities"].is_object())
            .count(),
        1,
        "B's populated row is compared: {peers}"
    );
    // `discovered` needs mDNS, which tests keep off (ponytail: its row shape is checked against the
    // fixture only by the UI side; add an injected record if it drifts). `gpus` comes from this
    // host's real probe, which a machine without a GPU leaves empty.
    assert_shape(
        "peers_e1",
        &fixture("peers_e1"),
        &peers,
        &[".discovered", ".peers[0].capabilities.hardware.gpus"],
    );

    let (status, v) = post(&a, &format!("/api/peers/{}/disconnect", idb.to_hex()), json!({})).await;
    assert_eq!(status, 200, "{v}");
    wait_for("B sees the close", || async {
        let v = get(&b, "/api/peers").await;
        (v["peers"][0]["connected"] == false).then_some(())
    })
    .await;
    let (status, v) = post(
        &b,
        &format!("/api/peers/{}/connect", ida.to_hex()),
        json!({ "addr": a_addr }),
    )
    .await;
    assert_eq!((status, v["error"]["code"].as_str()), (403, Some("peer_held")), "{v}");
    // Each fixture row against its real counterpart: the held one is A's row for B, the one the
    // other side held is B's row for A.
    let mut both = get(&a, "/api/peers").await;
    let b_rows = get(&b, "/api/peers").await["peers"].as_array().unwrap().clone();
    both["peers"].as_array_mut().unwrap().extend(b_rows);
    assert_shape("peers_e5", &fixture("peers_e5"), &both, &[]);

    let events = wait_for("B audits the loss", || async {
        let mut all = get(&a, "/api/audit?n=1000").await["events"].as_array().unwrap().clone();
        all.extend(get(&b, "/api/audit?n=1000").await["events"].as_array().unwrap().clone());
        all.iter().any(|e| e["kind"] == "peer_lost").then_some(all)
    })
    .await;
    // Per fixture event, the real events of the same kind as one array; for the kinds whose
    // reason selects the line's shape (`held`, `by_user`), of the same reason too.
    for f in fixture("audit_e4")["events"].as_array().unwrap() {
        let by_reason = matches!(f["kind"].as_str(), Some("peer_rejected" | "peer_disconnected"));
        let same: Vec<Value> = events
            .iter()
            .filter(|e| e["kind"] == f["kind"] && (!by_reason || e["fields"]["reason"] == f["fields"]["reason"]))
            .cloned()
            .collect();
        assert!(!same.is_empty(), "no {} event like {f} in {events:?}", f["kind"]);
        assert_shape(
            &format!("audit_e4 {}", f["kind"]),
            &json!([f]),
            &Value::Array(same),
            &[],
        );
    }
    a.stop().await;
    b.stop().await;
}
