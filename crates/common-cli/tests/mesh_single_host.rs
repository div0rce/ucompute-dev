//! **SINGLE-HOST**: two `common` installations (separate temp `COMMON_HOME`s, distinct identities,
//! different ports) on this one Mac pair, trust each other, run the golden prompt remotely and
//! revoke. Nothing here satisfies a two-machine criterion. The serving side runs the real pinned
//! llama-server on the stories15M fixture (skips loudly when absent; `COMMON_REQUIRE_FIXTURES=1`
//! fails instead). Nothing is mocked.
//!
//! Sequence (and why): `peers pair` / `peers join` refuse while a daemon runs on their home, so
//! trust is set up first and the two daemons start afterwards. With the daemons up, `peers revoke`,
//! `peers rm`, `groups` and `privacy` go through the daemon's API (`serve.json`), and the test
//! checks the daemons themselves: the revoked session closes within 5 s, the revoked key is refused
//! in the handshake, and a later daemon write keeps the revocation.
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const MODEL: &str = "stories15M-q4_0";
const BLOB: &str = "66967fbece6dbe97886593fdbb73589584927e29119ec31f08090732d1861739";
const PROMPT: &str = "Once upon a time, a little rabbit";
/// Greedy 32-token continuation of PROMPT with stories15M-q4_0 on llama.cpp b11609 (CPU and Metal).
const GOLDEN: &str =
    " named Benny was hopping in a field. He saw a big, red radish and he wanted to eat it. But the radish was too big";

fn real_home() -> PathBuf {
    std::env::var_os("COMMON_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.common"))
}

/// The real engine and fixture, or None after printing why (a failure with COMMON_REQUIRE_FIXTURES=1).
fn fixtures() -> Option<PathBuf> {
    let home = real_home();
    let engine = home.join("backends/llama.cpp/b11609/llama-server");
    let blob = home.join(format!("models/blobs/sha256-{BLOB}"));
    let manifest = home.join(format!("models/manifests/{MODEL}.json"));
    let missing = [&engine, &blob, &manifest].into_iter().find(|p| !p.is_file());
    let Some(missing) = missing else { return Some(home) };
    assert!(
        std::env::var_os("COMMON_REQUIRE_FIXTURES").is_none_or(|v| v != "1"),
        "COMMON_REQUIRE_FIXTURES=1: missing {}",
        missing.display()
    );
    eprintln!("SKIP: missing asset {}", missing.display());
    None
}

/// A fresh COMMON_HOME whose engine archive, engine dir and fixture blob are symlinks into the
/// real one (the manifest is a copy, so the store is writable); identity, peers, audit, runs are its own.
struct Home {
    root: PathBuf,
}

impl Home {
    fn new(label: &str, real: &Path) -> Home {
        let root = std::env::temp_dir().join(format!("common-mesh-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let backends = root.join("backends/llama.cpp");
        std::fs::create_dir_all(&backends).unwrap();
        for e in std::fs::read_dir(real.join("backends/llama.cpp")).unwrap() {
            let e = e.unwrap();
            std::os::unix::fs::symlink(e.path(), backends.join(e.file_name())).unwrap();
        }
        std::fs::create_dir_all(root.join("models/blobs")).unwrap();
        std::fs::create_dir_all(root.join("models/manifests")).unwrap();
        let blob = format!("models/blobs/sha256-{BLOB}");
        std::os::unix::fs::symlink(real.join(&blob), root.join(&blob)).unwrap();
        let manifest = format!("models/manifests/{MODEL}.json");
        std::fs::copy(real.join(&manifest), root.join(&manifest)).unwrap();
        Home { root }
    }
    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_common"));
        c.args(args)
            .env("COMMON_HOME", &self.root)
            .env("COMMON_MDNS", "0")
            .env_remove("RUST_LOG")
            .env_remove("COMMON_API_KEY")
            .stdin(Stdio::null());
        c
    }
    fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().expect("spawn common")
    }
    fn ok(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert!(
            o.status.success(),
            "common {args:?} failed: {}\n{}",
            text(&o.stderr),
            text(&o.stdout)
        );
        text(&o.stdout)
    }
    fn fails_with(&self, args: &[&str], code: &str) -> String {
        let o = self.run(args);
        let err = text(&o.stderr);
        assert_eq!(o.status.code(), Some(1), "common {args:?}: {err}");
        assert!(err.contains(&format!("error[{code}]")), "common {args:?}: {err}");
        err
    }
    fn spawn(&self, args: &[&str]) -> (Child, mpsc::Receiver<String>) {
        let mut child = self
            .cmd(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn common");
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        (child, rx)
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn json(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or_else(|e| panic!("not JSON ({e}): {s}"))
}

/// Next stdout line satisfying `pred`, within `within`.
fn wait_line(rx: &mpsc::Receiver<String>, within: Duration, pred: impl Fn(&str) -> bool) -> String {
    let deadline = Instant::now() + within;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let line = rx.recv_timeout(left).expect("expected stdout line before the deadline");
        if pred(&line) {
            return line;
        }
    }
}

fn free_udp_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn free_tcp_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn wait_http(port: u16, child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        if let Some(st) = child.try_wait().unwrap() {
            let mut err = String::new();
            child.stderr.take().unwrap().read_to_string(&mut err).unwrap();
            panic!("daemon exited early ({st}): {err}");
        }
        assert!(Instant::now() < deadline, "daemon did not listen on {port} within 60 s");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// One HTTP/1.1 request to a daemon's loopback API: (status, JSON body or Null).
fn http(port: u16, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(120))).unwrap();
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    let (head, body) = resp.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, serde_json::from_str(body).unwrap_or(Value::Null))
}

/// `connected` of `id` in the daemon's `GET /api/peers`; None when it is not pinned there.
fn connected(port: u16, id: &str) -> Option<bool> {
    let (st, v) = http(port, "GET", "/api/peers", None);
    assert_eq!(st, 200, "{v}");
    v["peers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == id)
        .map(|p| p["connected"].as_bool().unwrap())
}

fn wait_until(within: Duration, what: &str, cond: impl Fn() -> bool) -> Duration {
    let start = Instant::now();
    while !cond() {
        assert!(start.elapsed() < within, "{what}: not within {within:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
    start.elapsed()
}

/// Base64 (standard alphabet, unpadded, so it is a substring of the padded form too).
fn b64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    b.chunks(3)
        .flat_map(|c| {
            let n = c
                .iter()
                .enumerate()
                .fold(0u32, |n, (i, &x)| n | u32::from(x) << (16 - 8 * i));
            (0..=c.len()).map(move |i| T[(n >> (18 - 6 * i) & 63) as usize] as char)
        })
        .collect()
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            files_under(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// F-06-01 grep: the device key (its PKCS#8 file and the 32-byte seed inside) as raw bytes, hex,
/// upper hex, base64 or base64url appears in no file under `logs/` (audit.jsonl, engine logs), in
/// no `diag*.json` bundle and not in `output` (the daemon's stdout and stderr).
fn assert_key_absent(home: &Home, output: &str) -> usize {
    assert_eq!(
        (b64(b"Man"), b64(b"Ma"), b64(b"M")),
        ("TWFu".into(), "TWE".into(), "TQ".into())
    );
    let pkcs8 = std::fs::read(home.root.join("identity/device.key")).unwrap();
    // ring's Ed25519 PKCS#8 v2 document: SEQUENCE header, version 1, id-Ed25519, OCTET STRING
    // wrapping the 32-byte seed, then the public key (its [1] tag differs across ring versions).
    let after_len = [
        0x02, 0x01, 0x01, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
    ];
    assert_eq!(
        (pkcs8[0], &pkcs8[2..16]),
        (0x30, &after_len[..]),
        "unexpected device.key layout"
    );
    let needles: Vec<Vec<u8>> = [&pkcs8[..], &pkcs8[16..48]]
        .into_iter()
        .flat_map(|k| {
            let (h, b) = (hex::encode(k), b64(k));
            [
                k.to_vec(),
                h.to_uppercase().into_bytes(),
                h.into_bytes(),
                b.replace('+', "-").replace('/', "_").into_bytes(),
                b.into_bytes(),
            ]
        })
        .collect();
    let mut files = vec![];
    files_under(&home.root.join("logs"), &mut files);
    files.extend(
        std::fs::read_dir(&home.root)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("diag")),
    );
    assert!(files.iter().any(|f| f.ends_with("audit.jsonl")), "{files:?}");
    assert!(files.iter().any(|f| f.to_string_lossy().contains("diag")), "{files:?}");
    let mut hay: Vec<(String, Vec<u8>)> = files
        .iter()
        .map(|f| (f.display().to_string(), std::fs::read(f).unwrap()))
        .collect();
    hay.push(("daemon output".into(), output.as_bytes().to_vec()));
    for (name, h) in &hay {
        for n in &needles {
            assert!(
                !h.windows(n.len()).any(|w| w == &n[..]),
                "device key material in {name}"
            );
        }
    }
    hay.len()
}

fn terminate(mut child: Child, what: &str) -> String {
    assert!(Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap()
        .success());
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("{what} did not exit within 20 s of SIGTERM");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut err = String::new();
    if let Some(mut e) = child.stderr.take() {
        e.read_to_string(&mut err).unwrap();
    }
    assert!(status.success(), "{what} exit on SIGTERM: {status}\n{err}");
    err
}

#[test]
fn two_installs_pair_run_remote_and_revoke_single_host() {
    let Some(real) = fixtures() else { return };
    let a = Home::new("a", &real);
    let b = Home::new("b", &real);
    let (pa, pb) = (free_udp_port().to_string(), free_udp_port().to_string());
    let net_a = ["--listen-port", &pa, "--listen-addr", "127.0.0.1", "--no-mdns"];
    let net_b = ["--listen-port", &pb, "--listen-addr", "127.0.0.1", "--no-mdns"];

    // Default state: private, nothing paired, no groups.
    assert!(a
        .ok(&["privacy"])
        .contains("privacy local_only (network off, serving peers off)"));
    assert!(a.ok(&["peers", "list"]).contains("no paired devices"));
    assert_eq!(json(&a.ok(&["groups", "list", "--json"])), Value::Array(vec![]));

    // A opens a pairing window; its code, id and pairing address are read from stdout.
    let (pair, lines) = a.spawn(&[&["peers", "pair", "--name", "alpha"][..], &net_a].concat());
    let t = Duration::from_secs(30);
    let code = wait_line(&lines, t, |l| l.starts_with("code: "))["code: ".len()..].to_string();
    assert!(code.len() == 6 && code.bytes().all(|c| c.is_ascii_digit()), "{code}");
    let id_a = wait_line(&lines, t, |l| l.starts_with("peer-id: "))["peer-id: ".len()..].replace(' ', "");
    assert_eq!(id_a.len(), 64, "8 groups of 8 hex");
    let pair_addr = wait_line(&lines, t, |l| l.starts_with("pair-address: "))["pair-address: ".len()..].to_string();
    assert!(pair_addr.starts_with("127.0.0.1:"), "{pair_addr}");
    let qr = wait_line(&lines, t, |l| l.starts_with("qr: "));
    assert!(qr.contains(&format!("common-pair:v1:{id_a}:{pair_addr}:")), "{qr}");

    // Wrong code: refused with pairing_failed, nothing pinned anywhere; the window stays open.
    let wrong = format!("{:06}", (code.parse::<u32>().unwrap() + 1) % 1_000_000);
    b.fails_with(
        &[&["peers", "join", &pair_addr, &wrong, "--name", "beta"][..], &net_b].concat(),
        "pairing_failed",
    );
    assert!(b.ok(&["peers", "list"]).contains("no paired devices"));
    assert!(a.ok(&["peers", "list"]).contains("no paired devices"));
    // A malformed code is refused before any packet.
    b.fails_with(
        &["peers", "join", &pair_addr, "12", "--name", "beta"],
        "invalid_request",
    );

    // Right code: both sides pin each other with the other's workload address.
    let joined = b.ok(&[&["peers", "join", &pair_addr, &code, "--name", "beta"][..], &net_b].concat());
    assert!(
        joined.contains("paired alpha") && joined.contains(&format!("127.0.0.1:{pa}")),
        "{joined}"
    );
    let paired = wait_line(&lines, t, |l| l.starts_with("paired "));
    assert!(
        paired.contains("paired beta") && paired.contains(&format!("127.0.0.1:{pb}")),
        "{paired}"
    );
    let st = pair.wait_with_output().unwrap();
    assert!(st.status.success(), "{}", text(&st.stderr));
    let a_peers = json(&a.ok(&["peers", "list", "--json"]));
    let id_b = a_peers[0]["id"].as_str().unwrap().to_string();
    assert_eq!(a_peers[0]["name"], "beta");
    assert_eq!(a_peers[0]["last_addr"], format!("127.0.0.1:{pb}"));
    let b_peers = json(&b.ok(&["peers", "list", "--json"]));
    assert_eq!(b_peers[0]["id"], id_a);
    assert_eq!(b_peers[0]["name"], "alpha");
    let label_a = b_peers[0]["label"].as_str().unwrap().to_string();
    assert_eq!(label_a.len(), 14);

    // Groups: both allow remote inference for the other (default deny until then).
    a.ok(&["groups", "create", "home", "--allow-remote"]);
    a.ok(&["groups", "add", "home", &id_b]);
    b.ok(&["groups", "create", "home", "--allow-remote"]);
    b.ok(&["groups", "add", "home", &label_a]);
    let g = json(&a.ok(&["groups", "policy", "home", "--allow-pooled=false", "--json"]));
    assert_eq!(
        (
            g["policy"]["allow_remote_inference"].as_bool(),
            g["policy"]["allow_pooled_inference"].as_bool()
        ),
        (Some(true), Some(false))
    );
    assert_eq!(json(&a.ok(&["groups", "list", "--json"]))[0]["members"][0], id_b);
    a.fails_with(&["groups", "add", "home", "ffff-0000-1234"], "peer_unauthorized");

    // Privacy: local-only refuses --peer before anything is opened; leaving it needs --yes off a TTY.
    let err = a.fails_with(&["run", MODEL, "-p", PROMPT, "--peer", &id_b], "policy_denied");
    assert!(err.contains("local execution"), "{err}");
    b.fails_with(&["privacy", "trusted-devices"], "invalid_request");
    let s = json(&b.ok(&["privacy", "trusted-devices", "--yes", "--json"]));
    assert_eq!(
        (s["privacy"].as_str(), s["network_enabled"].as_bool()),
        (Some("trusted_group"), Some(true))
    );
    let cfg: Value = serde_json::from_slice(&std::fs::read(b.root.join("config.json")).unwrap()).unwrap();
    assert_eq!(cfg["privacy"], "trusted_group");

    // Two daemons, A serving peers. SINGLE-HOST: both on 127.0.0.1 of this Mac.
    let (api_a, api_b) = (free_tcp_port(), free_tcp_port());
    let (mut da, la) = a.spawn(
        &[
            &["serve", "--port", &api_a.to_string(), "--network", "--serve-peers"][..],
            &net_a,
        ]
        .concat(),
    );
    let (mut db, lb) = b.spawn(&[&["serve", "--port", &api_b.to_string(), "--network"][..], &net_b].concat());
    wait_http(api_a, &mut da);
    wait_http(api_b, &mut db);

    // Golden text through A's real engine, requested from B.
    let o = b.run(&[
        "run", MODEL, "-p", PROMPT, "-n", "32", "--temp", "0", "--seed", "42", "--peer", &id_a, "--json",
    ]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let v = json(&text(&o.stdout));
    assert_eq!(v["completion"], GOLDEN);
    assert_eq!(v["scope"], "remote_single_peer");
    assert_eq!(v["remote_peer_id"], id_a);
    assert_eq!(
        v["physical_device_count"], 1,
        "one Mac; the peer id is the evidence, not a device count"
    );
    assert_eq!(v["model_sha256"], BLOB);
    assert_eq!(v["completion_token_ids"].as_array().map(Vec::len), Some(32));
    assert_eq!(
        (v["prompt_tokens"].as_u64(), v["completion_tokens"].as_u64()),
        (Some(10), Some(32))
    );
    assert!(v["device_actual"].as_str().unwrap().starts_with("peer:"), "{v}");
    assert!(
        v.get("prompt").is_none(),
        "prompt text is not stored without --save-prompt"
    );
    let _ = std::fs::remove_dir_all(Path::new(v["manifest_path"].as_str().unwrap()).parent().unwrap());
    // Text mode names the target in the summary line.
    let o = b.run(&[
        "run", MODEL, "-p", PROMPT, "-n", "8", "--temp", "0", "--seed", "42", "--peer", &label_a,
    ]);
    let err = text(&o.stderr);
    assert!(o.status.success(), "{err}");
    assert!(text(&o.stdout).starts_with(" named Benny"), "{}", text(&o.stdout));
    assert!(err.contains(&format!("target=peer:{id_a}")), "{err}");

    // A audited the request (ids and counts), never the text; the bundle is redacted too.
    let audit = json(&a.ok(&["audit", "--n", "20", "--json"]));
    let kinds: Vec<&str> = audit
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert!(
        kinds.contains(&"pairing_succeeded") && kinds.contains(&"remote_request"),
        "{kinds:?}"
    );
    let raw = std::fs::read_to_string(a.root.join("logs/audit.jsonl")).unwrap();
    assert!(
        !raw.contains(PROMPT) && !raw.contains("Benny"),
        "text leaked into audit.jsonl"
    );
    let out = a.root.join("diag.json");
    a.ok(&["diagnostics", "--out", out.to_str().unwrap()]);
    let bundle = std::fs::read_to_string(&out).unwrap();
    assert_eq!(json(&bundle)["schema_version"], 1);
    assert!(
        !bundle.contains(PROMPT) && !bundle.contains("Benny"),
        "text leaked into the bundle"
    );
    assert!(!bundle.contains(&id_a), "64-hex ids are masked in the bundle");

    // While a daemon owns the trust store, CLI pairing (its own endpoint and pin) is refused.
    let err = a.fails_with(&["peers", "pair"], "invalid_request");
    assert!(err.contains("owns this device's trust store"), "{err}");

    // Daemon B opens a session to daemon A (the API's peer target); daemon A holds the other end.
    let completion = |target: &str| serde_json::json!({ "model": MODEL, "prompt": PROMPT, "max_tokens": 4, "temperature": 0, "target": format!("peer:{target}") });
    let (st, v) = http(api_b, "POST", "/v1/completions", Some(completion(&id_a)));
    assert_eq!(st, 200, "{v}");
    assert_eq!(connected(api_b, &id_a), Some(true));
    assert_eq!(connected(api_a, &id_b), Some(true));

    // Revocation with both daemons running (F-06-05). The CLI goes through daemon B (serve.json),
    // which closes A's session at once: daemon A sees it closed within 5 s.
    let r = json(&b.ok(&["peers", "revoke", &label_a, "--json"]));
    assert_eq!(r["peer_id"], id_a);
    assert_eq!(r["signature"].as_str().map(str::len), Some(128));
    assert_eq!(r["daemon_applied"], true);
    let closed = wait_until(Duration::from_secs(5), "A's session with daemon B closed", || {
        connected(api_a, &id_b) == Some(false)
    });
    eprintln!("SINGLE-HOST: revoked session seen closed by the other daemon after {closed:?}");
    assert_eq!(connected(api_b, &id_a), None, "daemon B unpinned A in memory");
    // A, which still trusts B, reconnects: daemon B refuses A's key inside the handshake. B does
    // not serve peers, so a handshake that got through would end in policy_denied instead.
    let s = json(&a.ok(&["privacy", "trusted-devices", "--yes", "--json"]));
    assert_eq!(s["privacy"], "trusted_group");
    assert_eq!(
        http(api_a, "GET", "/api/privacy", None).1["mode"],
        "trusted_group",
        "via daemon A"
    );
    a.fails_with(&["run", MODEL, "-p", PROMPT, "--peer", &id_b], "peer_revoked");
    let (st, v) = http(api_a, "POST", "/v1/completions", Some(completion(&id_b)));
    assert_eq!((st, v["error"]["code"].as_str()), (403, Some("peer_revoked")), "{v}");
    // A later daemon write (a CLI group edit through daemon B) keeps the revocation on disk.
    b.ok(&["groups", "create", "later"]);
    let (_, g) = http(api_b, "GET", "/api/groups", None);
    assert!(
        g["groups"].as_array().unwrap().iter().any(|g| g["name"] == "later"),
        "daemon B applied the CLI's group edit: {g}"
    );
    let disk: Value = serde_json::from_slice(&std::fs::read(b.root.join("identity/peers.json")).unwrap()).unwrap();
    assert_eq!(disk["peers"], Value::Array(vec![]), "{disk}");
    assert!(
        disk["revocations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["peer_id"] == id_a),
        "{disk}"
    );
    b.fails_with(&["run", MODEL, "-p", PROMPT, "--peer", &id_a], "peer_revoked");
    b.fails_with(&["run", MODEL, "-p", PROMPT, "--peer", &label_a], "peer_revoked");
    assert!(b.ok(&["peers", "list"]).contains("no paired devices"));
    // Unknown peers are peer_unauthorized; `rm` forgets without a revocation record.
    b.fails_with(
        &["run", MODEL, "-p", PROMPT, "--peer", "0000-1111-2222"],
        "peer_unauthorized",
    );
    let rm = json(&a.ok(&["peers", "rm", &id_b, "--json"]));
    assert_eq!(rm["daemon_applied"], true);
    assert_eq!(connected(api_a, &id_b), None, "daemon A forgot B");
    assert!(a.ok(&["peers", "list"]).contains("no paired devices"));

    // SIGTERM stops both daemons cleanly (engine unloaded, endpoint closed, serve.json removed).
    let out_a = terminate(da, "daemon A") + &la.try_iter().collect::<Vec<_>>().join("\n");
    let out_b = terminate(db, "daemon B") + &lb.try_iter().collect::<Vec<_>>().join("\n");
    assert!(!a.root.join("serve.json").exists() && !b.root.join("serve.json").exists());

    // F-06-01: after real pairing, remote runs and daemon sessions, neither device key is in any
    // log, audit trail, diagnostics bundle or daemon output.
    for h in [&a, &b] {
        let out = h.root.join("diag-final.json");
        h.ok(&["diagnostics", "--out", out.to_str().unwrap()]);
    }
    let checked = assert_key_absent(&a, &out_a) + assert_key_absent(&b, &out_b);
    eprintln!("SINGLE-HOST: device keys absent from {checked} logs, bundles and outputs");
}
