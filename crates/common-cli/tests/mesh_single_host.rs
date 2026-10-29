//! **SINGLE-HOST**: two `common` installations (separate temp `COMMON_HOME`s, distinct identities,
//! different ports) on this one Mac pair, trust each other, run the golden prompt remotely and
//! revoke. Nothing here satisfies a two-machine criterion. The serving side runs the real pinned
//! llama-server on the stories15M fixture (skips loudly when absent; `COMMON_REQUIRE_FIXTURES=1`
//! fails instead). Nothing is mocked.
//!
//! Sequence (and why): `peers pair` / `peers join` refuse while a daemon runs on their home (it
//! runs this device's peer endpoint), so trust is set up first and the two daemons start
//! afterwards. With the daemons up, `peers revoke`, `peers rm`, `groups` and `privacy` go through
//! the daemon's API (`serve.json`), and the test checks the daemons themselves: the revoked session
//! closes within 5 s, the revoked key is refused in the handshake, and a later daemon write keeps
//! the revocation.
// Unix only: symlinked homes and signals (libc::kill, waitpid).
#![cfg(unix)]
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
        c.args(args);
        // A daemon must not outlive the test binary: a failed assertion skips `terminate`, and a
        // dropped `Child` is not killed, which left daemons and their engines running for hours.
        if args.first() == Some(&"serve") {
            c.arg("--exit-with-parent");
        }
        c.env("COMMON_HOME", &self.root)
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
    /// `ok` at debug level (`-vv`), appending its stdout and stderr to `log`.
    fn ok_vv(&self, args: &[&str], log: &mut String) -> String {
        let o = self.run(&[args, &["-vv"]].concat());
        let (out, err) = (text(&o.stdout), text(&o.stderr));
        assert!(o.status.success(), "common {args:?} failed: {err}\n{out}");
        log.push_str(&err);
        log.push_str(&out);
        out
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
            if let Some(mut e) = child.stderr.take() {
                e.read_to_string(&mut err).unwrap();
            }
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

/// Regular files under `dir`, symlinks not followed (counted in `links`).
fn files_under(dir: &Path, out: &mut Vec<PathBuf>, links: &mut usize) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let (p, t) = (e.path(), e.file_type().unwrap());
        if t.is_symlink() {
            *links += 1;
        } else if t.is_dir() {
            files_under(&p, out, links);
        } else {
            out.push(p);
        }
    }
}

/// What [`assert_key_absent`] searched: files, their bytes, symlinks not followed.
struct Scanned {
    /// Relative to the home.
    files: Vec<String>,
    bytes: usize,
    links: usize,
    /// Number of seed encodings searched for.
    needles: usize,
}

/// ring's Ed25519 PKCS#8 v2 document after its 2-byte SEQUENCE header: version 1, id-Ed25519,
/// OCTET STRING wrapping the 32-byte seed (bytes 16..48), then the public key (its [1] tag differs
/// across ring versions).
const PKCS8_AFTER_LEN: [u8; 14] = [
    0x02, 0x01, 0x01, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
];

/// The forms the 32-byte Ed25519 seed could take in a log, file or bundle. Each also catches the
/// PKCS#8 document (or seed||public key) printed the same way, since the seed sits inside it.
/// Raw bytes; lower and upper hex; the comma-joined decimals that `{:?}`, `{:#?}` and serde_json
/// print for a byte slice (matched after whitespace is dropped, see [`contains_key`]); base64 and
/// base64url with the seed starting at byte 0, 1 or 2 of a 3-byte group of a larger buffer: the
/// 30 seed bytes from offset 0, 1 or 2 are whole groups, so their 40 chars do not depend on
/// neighbouring bytes or padding.
// ponytail: not covered are `{:x?}` (unpadded hex list), per-byte `0x..` and other ad-hoc
// formats; add a needle when a code path prints bytes that way.
fn seed_needles(seed: &[u8]) -> Vec<Vec<u8>> {
    assert_eq!(seed.len(), 32);
    let h = hex::encode(seed);
    let decimal: Vec<String> = seed.iter().map(u8::to_string).collect();
    let mut n = vec![
        seed.to_vec(),
        h.to_uppercase().into_bytes(),
        h.into_bytes(),
        decimal.join(",").into_bytes(),
    ];
    for i in 0..3 {
        let b = b64(&seed[i..i + 30]);
        n.push(b.replace('+', "-").replace('/', "_").into_bytes());
        n.push(b.into_bytes());
    }
    n
}

/// Whether any needle occurs in `hay` as is or with all ASCII whitespace removed (so `{:?}`'s
/// `", "`, pretty JSON and line-wrapped base64 or hex match too; extra hits only fail safe).
fn contains_key(hay: &[u8], needles: &[Vec<u8>]) -> bool {
    let squeezed: Vec<u8> = hay.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
    [hay, &squeezed[..]]
        .iter()
        .any(|h| needles.iter().any(|n| h.windows(n.len()).any(|w| w == &n[..])))
}

/// F-06-01 grep: the device key (the 32-byte Ed25519 seed in its PKCS#8 file, in every form of
/// [`seed_needles`]) appears in no file under the home (`identity/device.key` itself excepted; the
/// home must hold `audit.jsonl` and a `diag*` bundle) and not in `output` (daemon and CLI stdout
/// and stderr). Symlinks are not followed: in these homes they point at the hash-pinned engine and
/// model outside the home.
fn assert_key_absent(home: &Home, output: &str) -> Scanned {
    let key_file = home.root.join("identity/device.key");
    let pkcs8 = std::fs::read(&key_file).unwrap();
    assert_eq!(
        (pkcs8[0], &pkcs8[2..16]),
        (0x30, &PKCS8_AFTER_LEN[..]),
        "unexpected device.key layout"
    );
    let needles = seed_needles(&pkcs8[16..48]);
    let (mut files, mut links) = (vec![], 0);
    files_under(&home.root, &mut files, &mut links);
    files.retain(|f| *f != key_file);
    assert!(files.iter().any(|f| f.ends_with("logs/audit.jsonl")), "{files:?}");
    assert!(
        files
            .iter()
            .any(|f| f.parent() == Some(&home.root) && f.file_name().unwrap().to_string_lossy().starts_with("diag")),
        "{files:?}"
    );
    let mut hay: Vec<(String, Vec<u8>)> = files
        .iter()
        .map(|f| (f.display().to_string(), std::fs::read(f).unwrap()))
        .collect();
    hay.push(("daemon output".into(), output.as_bytes().to_vec()));
    for (name, h) in &hay {
        assert!(!contains_key(h, &needles), "device key material in {name}");
    }
    Scanned {
        files: files
            .iter()
            .map(|f| f.strip_prefix(&home.root).unwrap().display().to_string())
            .collect(),
        bytes: hay.iter().map(|(_, h)| h.len()).sum(),
        links,
        needles: needles.len(),
    }
}

/// SYNTHETIC key (a made-up seed, no daemon): the scan in [`assert_key_absent`] catches each form
/// it claims, including the key inside a larger buffer at every base64 alignment, and does not
/// fire on unrelated text. This is the positive control for the F-06-01 greps below.
#[test]
fn key_scan_catches_every_claimed_encoding_synthetic_key() {
    assert_eq!(
        (b64(b"Man"), b64(b"Ma"), b64(b"M")),
        ("TWFu".into(), "TWE".into(), "TQ".into())
    );
    let seed: Vec<u8> = (0..32u8).map(|i| i.wrapping_mul(197).wrapping_add(251)).collect();
    let public = [0xfeu8; 32];
    let mut doc = vec![0x30, 0x51];
    doc.extend(PKCS8_AFTER_LEN);
    doc.extend(&seed);
    doc.extend([0x81, 0x21, 0x00]);
    doc.extend(public);
    let pair = [&seed[..], &public[..]].concat();
    let url = |s: String| s.replace('+', "-").replace('/', "_");
    let mut leaks: Vec<Vec<u8>> = vec![
        doc.clone(),
        format!("key={seed:?}").into(),
        format!("{doc:#?}").into(),
        serde_json::to_vec(&doc).unwrap(),
        serde_json::to_vec_pretty(&seed).unwrap(),
        hex::encode(&doc).into(),
        hex::encode_upper(&pair).into(),
    ];
    // Base64 of doc, of seed||public and of the seed behind 1 and 2 other bytes (alignments 1, 0,
    // 1, 2), wrapped every 16 chars so each needle crosses a line break; base64url of each.
    for buf in [
        doc.clone(),
        pair.clone(),
        [&[1u8][..], &pair[..]].concat(),
        [&[1u8, 2][..], &seed[..]].concat(),
    ] {
        let b = b64(&buf);
        let wrapped: Vec<&str> = b
            .as_bytes()
            .chunks(16)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect();
        leaks.push(wrapped.join("\n").into());
        leaks.push(url(b).into());
    }
    let needles = seed_needles(&seed);
    assert_eq!(needles.len(), 10);
    for leak in &leaks {
        assert!(contains_key(leak, &needles), "missed: {}", text(leak));
    }
    let clean = format!("{public:?} {} {}", hex::encode(public), b64(&doc[..16]));
    assert!(!contains_key(clean.as_bytes(), &needles), "{clean}");
}

/// `remote_request` audit entries of `home` written by the sending side (`role: client`), or by
/// the serving side (no role) when `client` is false.
fn remote_entries(home: &Home, client: bool) -> Vec<Value> {
    let all = json(&home.ok(&["audit", "--n", "500", "--json"]));
    all.as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "remote_request" && (e["fields"]["role"] == "client") == client)
        .cloned()
        .collect()
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
    // A QR payload naming another device: A's key fails B's TLS verifier inside the handshake, so
    // nothing is pinned on either side and A's window stays open.
    let secret = qr.rsplit(':').next().unwrap();
    let other = "ab".repeat(32);
    let wrong_qr = format!("common-pair:v1:{other}:{pair_addr}:{secret}");
    let err = b.fails_with(
        &[&["peers", "join", &wrong_qr, "--name", "beta"][..], &net_b].concat(),
        "pairing_failed",
    );
    assert!(err.contains("not the one in the QR code"), "{err}");
    assert!(b.ok(&["peers", "list"]).contains("no paired devices"));
    assert!(a.ok(&["peers", "list"]).contains("no paired devices"));
    let raw = std::fs::read_to_string(b.root.join("logs/audit.jsonl")).unwrap();
    assert!(raw.contains("qr_peer_mismatch") && raw.contains(&other), "{raw}");
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
    // No daemon: store-only, connection state unknown (null), and the text says why.
    assert!(a.ok(&["peers", "list"]).contains("`common serve` is not running here"));
    let a_list = json(&a.ok(&["peers", "list", "--json"]));
    assert_eq!(
        (&a_list["daemon"], &a_list["discovered"]),
        (&Value::Null, &Value::Array(vec![]))
    );
    let a_peers = &a_list["peers"];
    assert_eq!(a_peers[0]["connected"], Value::Null);
    let id_b = a_peers[0]["id"].as_str().unwrap().to_string();
    assert_eq!(a_peers[0]["name"], "beta");
    assert_eq!(a_peers[0]["last_addr"], format!("127.0.0.1:{pb}"));
    let b_list = json(&b.ok(&["peers", "list", "--json"]));
    let b_peers = &b_list["peers"];
    assert!(
        b_list["this_device"]
            .as_str()
            .is_some_and(|me| me.len() == 64 && me != id_a),
        "{b_list}"
    );
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
    // F-06-10, sending side: B audited `sent` then `finished` (done, 32 tokens), ids and counts only.
    let sent = remote_entries(&b, true);
    let phases: Vec<(&str, &str)> = sent
        .iter()
        .map(|e| {
            (
                e["fields"]["phase"].as_str().unwrap(),
                e["fields"]["outcome"].as_str().unwrap_or("-"),
            )
        })
        .collect();
    assert_eq!(phases, [("sent", "-"), ("finished", "done")], "{sent:?}");
    for e in &sent {
        assert_eq!(
            (&e["peer_id"], &e["fields"]["model_id"]),
            (&Value::from(id_a.as_str()), &Value::from(MODEL))
        );
        assert_eq!(e["fields"]["peer"], label_a);
    }
    assert_eq!(sent[1]["fields"]["completion_tokens"], 32);
    let raw = std::fs::read_to_string(b.root.join("logs/audit.jsonl")).unwrap();
    assert!(
        !raw.contains(PROMPT) && !raw.contains("Benny"),
        "text leaked into B's audit"
    );
    // Both sides' entries join on the request id (A writes its entry once it has finished).
    // `finished` carries it; `sent` carries the same id or none (common-api decides when the id
    // is chosen relative to the `sent` entry).
    let id = sent[1]["fields"]["request_id"]
        .as_str()
        .expect("request id")
        .to_string();
    let first = &sent[0]["fields"]["request_id"];
    assert!(first.is_null() || *first == id.as_str(), "{sent:?}");
    wait_until(Duration::from_secs(5), "A audited the request B sent", || {
        remote_entries(&a, false)
            .iter()
            .any(|e| e["fields"]["request_id"] == id.as_str() && e["fields"]["outcome"] == "done")
    });
    // No run without its `sent` entry: with B's audit log unwritable the request is cancelled as
    // soon as it leaves, the run fails with `io` and prints no completion. (A may log the cancel.)
    let log = b.root.join("logs/audit.jsonl");
    let mode = |m: u32| std::fs::set_permissions(&log, std::os::unix::fs::PermissionsExt::from_mode(m)).unwrap();
    mode(0o400);
    let o = b.run(&["run", MODEL, "-p", PROMPT, "-n", "4", "--peer", &id_a]);
    mode(0o600);
    let err = text(&o.stderr);
    assert_eq!(o.status.code(), Some(1), "{err}");
    assert!(
        err.contains("error[io]") && err.contains("audit log not writable"),
        "{err}"
    );
    assert_eq!(text(&o.stdout), "", "no completion without an audit entry");
    assert_eq!(
        remote_entries(&b, true).len(),
        2,
        "nothing recorded for the refused run"
    );
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

    // While a daemon runs this device's peer endpoint, CLI pairing (a second endpoint) is refused.
    let err = a.fails_with(&["peers", "pair"], "invalid_request");
    assert!(err.contains("runs this device's peer endpoint"), "{err}");

    // Daemon B opens a session to daemon A (the API's peer target); daemon A holds the other end.
    let completion = |target: &str| serde_json::json!({ "model": MODEL, "prompt": PROMPT, "max_tokens": 4, "temperature": 0, "target": format!("peer:{target}") });
    let (st, v) = http(api_b, "POST", "/v1/completions", Some(completion(&id_a)));
    assert_eq!(st, 200, "{v}");
    assert_eq!(connected(api_b, &id_a), Some(true));
    assert_eq!(connected(api_a, &id_b), Some(true));
    // `peers list` merges the daemon's live view (mDNS is off here, so nothing is discovered).
    let live = json(&b.ok(&["peers", "list", "--json"]));
    assert_eq!(live["daemon"], format!("http://127.0.0.1:{api_b}"), "{live}");
    assert_eq!(
        (&live["peers"][0]["id"], &live["peers"][0]["connected"]),
        (&Value::from(id_a.as_str()), &Value::Bool(true))
    );
    assert_eq!(live["discovered"], Value::Array(vec![]));
    let table = b.ok(&["peers", "list"]);
    assert!(table.contains("CONNECTED") && !table.contains("not running"), "{table}");

    // E5: Disconnect in daemon B holds A for B's CLI too. `run --peer` (its own client Node) is
    // refused before it dials; daemon A, which holds nothing, would serve it. Connect ends the hold.
    let (st, v) = http(
        api_b,
        "POST",
        &format!("/api/peers/{id_a}/disconnect"),
        Some(serde_json::json!({})),
    );
    assert_eq!((st, v["id"].as_str()), (200, Some(id_a.as_str())), "{v}");
    let err = b.fails_with(
        &["run", MODEL, "-p", PROMPT, "-n", "4", "--peer", &id_a],
        "policy_denied",
    );
    assert!(err.contains("disconnected by you"), "{err}");
    let (st, v) = http(
        api_b,
        "POST",
        &format!("/api/peers/{id_a}/connect"),
        Some(serde_json::json!({})),
    );
    assert_eq!(st, 200, "{v}");
    assert_eq!(connected(api_b, &id_a), Some(true));

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
    // That refusal comes before B sent any control data, so A cannot tell it from one forged on
    // the path: it reads as a device that did not prove it is B, never as revoked (trust-3).
    let err = a.fails_with(&["run", MODEL, "-p", PROMPT, "--peer", &id_b], "peer_unauthorized");
    assert!(err.contains("did not prove"), "{err}");
    let (st, v) = http(api_a, "POST", "/v1/completions", Some(completion(&id_b)));
    assert_eq!(
        (st, v["error"]["code"].as_str()),
        (403, Some("peer_unauthorized")),
        "{v}"
    );
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
    for (h, out) in [(&a, &out_a), (&b, &out_b)] {
        let s = assert_key_absent(h, out);
        eprintln!(
            "SINGLE-HOST: device key absent ({} encodings of its seed, default log level) from {} files {:?} and the daemon output ({} bytes; {} symlinked pinned assets not followed)",
            s.needles, s.files.len(), s.files, s.bytes, s.links
        );
    }
}

/// F-06-01 at debug log level, with the paths that sign with the device key exercised: daemon A
/// (`common serve --network --serve-peers -vv`, pinned engine, real fixture) opens a pairing
/// window; install B (`-vv` CLI, no daemon) pairs with it over SPAKE2, then runs a remote request
/// on A (TLS 1.3 handshakes with both raw public keys, signed capabilities both ways, real
/// inference on A); A answers a local completion and revokes B through the daemon (a revocation
/// signed with A's key). Then both keys are searched (every form of [`seed_needles`]) in every
/// file under both homes, both diagnostics bundles and all daemon and CLI stdout and stderr.
/// SINGLE-HOST: one daemon and one CLI on this Mac; nothing here is a two-machine result.
#[test]
fn private_key_absent_from_real_logs_and_bundle_single_host() {
    let Some(real) = fixtures() else { return };
    let h = Home::new("leak", &real);
    let b = Home::new("leakb", &real);
    let (api, udp, udp_b) = (
        free_tcp_port(),
        free_udp_port().to_string(),
        free_udp_port().to_string(),
    );
    let net_b = ["--listen-port", &udp_b, "--listen-addr", "127.0.0.1", "--no-mdns"];
    let (mut d, lines) = h.spawn(&[
        "serve",
        "--port",
        &api.to_string(),
        "--network",
        "--serve-peers",
        "-vv",
        "--listen-port",
        &udp,
        "--listen-addr",
        "127.0.0.1",
        "--no-mdns",
    ]);
    // Debug logging is chatty: drain stderr so the daemon never blocks on a full pipe.
    let mut stderr = d.stderr.take().unwrap();
    let err_reader = std::thread::spawn(move || {
        let mut v = Vec::new();
        stderr.read_to_end(&mut v).unwrap();
        v
    });
    wait_http(api, &mut d);
    // Stdout and stderr of every `-vv` CLI run, A's and B's.
    let mut cli = String::new();

    let (st, net) = http(api, "GET", "/api/network", None);
    assert_eq!((st, net["enabled"].as_bool()), (200, Some(true)), "{net}");
    assert_eq!(http(api, "GET", "/api/peers", None).0, 200);
    let (st, pair) = http(
        api,
        "POST",
        "/api/pair/start",
        Some(serde_json::json!({ "name": "leak" })),
    );
    assert_eq!(st, 200, "{pair}");
    let qr = pair["qr_payload"].as_str().unwrap_or_default();
    assert!(qr.starts_with("common-pair:v1:"), "{pair}");
    let id_a = qr.split(':').nth(2).unwrap().to_string();

    // B pairs with the daemon's window (SPAKE2 inside TLS with A's device key).
    let (addr, code) = (pair["pair_addr"].as_str().unwrap(), pair["code"].as_str().unwrap());
    let joined = b.ok_vv(
        &[&["peers", "join", addr, code, "--name", "leakb"][..], &net_b].concat(),
        &mut cli,
    );
    assert!(joined.contains("paired leak"), "{joined}");
    wait_until(Duration::from_secs(10), "daemon A finished pairing", || {
        !http(api, "GET", "/api/pair/status", None).1["result"]["peer"].is_null()
    });
    let id_b = json(&b.ok(&["peers", "list", "--json"]))["this_device"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(connected(api, &id_b), Some(false), "daemon A pinned B");

    // Policy on both sides (default deny), then B runs the prompt on A: one handshake each way of
    // signing (TLS CertificateVerify, capabilities), real inference on A.
    h.ok_vv(&["groups", "create", "home", "--allow-remote"], &mut cli);
    h.ok_vv(&["groups", "add", "home", &id_b], &mut cli);
    b.ok_vv(&["groups", "create", "home", "--allow-remote"], &mut cli);
    b.ok_vv(&["groups", "add", "home", &id_a], &mut cli);
    b.ok_vv(&["privacy", "trusted-devices", "--yes"], &mut cli);
    let remote = b.ok_vv(
        &[
            "run", MODEL, "-p", PROMPT, "-n", "8", "--temp", "0", "--seed", "42", "--peer", &id_a,
        ],
        &mut cli,
    );
    assert!(remote.starts_with(" named Benny"), "{remote}");

    let (st, v) = http(
        api,
        "POST",
        "/v1/completions",
        Some(serde_json::json!({ "model": MODEL, "prompt": PROMPT, "max_tokens": 8, "temperature": 0 })),
    );
    assert_eq!(st, 200, "{v}");
    assert!(
        v["choices"][0]["text"]
            .as_str()
            .is_some_and(|t| t.starts_with(" named Benny")),
        "{v}"
    );
    // Daemon A signs the revocation of B with its device key.
    let r = json(&h.ok_vv(&["peers", "revoke", &id_b, "--json"], &mut cli));
    assert_eq!(
        (&r["daemon_applied"], r["signature"].as_str().map(str::len)),
        (&Value::Bool(true), Some(128)),
        "{r}"
    );
    // A writes its `remote_request` entry once the request has finished.
    wait_until(
        Duration::from_secs(5),
        "A audited pairing, the remote request and the revoke",
        || {
            let (st, audit) = http(api, "GET", "/api/audit", None);
            assert_eq!(st, 200, "{audit}");
            let kinds: Vec<&str> = audit["events"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|e| e["kind"].as_str())
                .collect();
            ["pairing_succeeded", "remote_request", "peer_revoked"]
                .iter()
                .all(|k| kinds.contains(k))
        },
    );

    let stdout = terminate(d, "daemon") + &lines.try_iter().collect::<Vec<_>>().join("\n");
    let stderr = String::from_utf8_lossy(&err_reader.join().unwrap()).into_owned();
    // rustls's debug line names the ALPN of each handshake the daemon served: the pairing one and
    // the workload one were both logged at debug level, inside the output scanned below.
    for alpn in ["common-pair/1", "common/1"] {
        assert!(
            stderr.contains(&format!("ProtocolName({})", hex::encode(alpn))),
            "daemon logged no {alpn} handshake at debug"
        );
    }
    for (who, out) in [("daemon", &stderr), ("CLI", &cli)] {
        assert!(
            out.contains("DEBUG"),
            "{who} -vv logs at debug: {}",
            &out[..out.len().min(500)]
        );
    }
    for (home, name) in [(&h, "diag-leak.json"), (&b, "diag-leakb.json")] {
        let bundle = home.root.join(name);
        home.ok(&["diagnostics", "--out", bundle.to_str().unwrap()]);
        assert_eq!(json(&std::fs::read_to_string(&bundle).unwrap())["schema_version"], 1);
    }

    let all = format!("{stdout}\n{stderr}\n{cli}");
    for (home, who) in [(&h, "A (daemon)"), (&b, "B (CLI)")] {
        let s = assert_key_absent(home, &all);
        eprintln!(
            "SINGLE-HOST: device key of {who}: 0 hits for {} encodings of its seed in {} files under its COMMON_HOME {:?} and {} bytes of -vv daemon+CLI stdout/stderr (pairing, handshakes, remote request, revocation); {} bytes scanned in all; {} symlinked pinned assets (engine, model) not followed",
            s.needles,
            s.files.len(),
            s.files,
            all.len(),
            s.bytes,
            s.links
        );
    }
}

/// WP-04 C6, SINGLE-HOST: `common run --split` goes through the running daemon, prints the
/// daemon's pooling disclosure and sends nothing until the user answers yes. A no ends it with
/// `policy_denied`; a yes reaches the licence gate (stories15M's licence is unknown, B-011, and
/// nothing lifts the gate here), still before any connection. The peer is pinned at an address
/// nothing listens on, in a group that allows pooling.
#[test]
fn run_split_shows_the_disclosure_and_needs_a_yes_single_host() {
    let Some(real) = fixtures() else { return };
    let h = Home::new("split", &real);
    let peer = common_identity::DeviceIdentity::generate().unwrap().id;
    let store = common_identity::PeerStore::open(&common_core::Paths::at(&h.root)).unwrap();
    store
        .pin(common_identity::PeerRecord {
            id: peer,
            name: "stage".into(),
            paired_at_utc: common_identity::utc_now(),
            last_seen_utc: None,
            last_addr: Some("127.0.0.1:1".into()),
        })
        .unwrap();
    store
        .upsert_group(common_identity::Group {
            name: "pool".into(),
            members: vec![peer],
            policy: common_identity::GroupPolicy {
                allow_remote_inference: false,
                allow_pooled_inference: true,
            },
        })
        .unwrap();
    h.ok(&["privacy", "trusted-devices", "--yes"]);
    // no daemon: refused, nothing opened
    let args = [
        "run",
        MODEL,
        "-p",
        PROMPT,
        "-n",
        "4",
        "--split",
        &peer.to_hex(),
        "--split-at",
        "3",
    ];
    let err = h.fails_with(&args, "policy_denied");
    assert!(err.contains("`common serve`"), "{err}");
    let (api, pa) = (free_tcp_port(), free_udp_port().to_string());
    let (mut d, _lines) = h.spawn(&[
        "serve",
        "--port",
        &api.to_string(),
        "--network",
        "--listen-port",
        &pa,
        "--listen-addr",
        "127.0.0.1",
        "--no-mdns",
    ]);
    wait_http(api, &mut d);
    let answer = |reply: &str| {
        let mut c = h
            .cmd(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        c.stdin.take().unwrap().write_all(reply.as_bytes()).unwrap();
        let o = c.wait_with_output().unwrap();
        assert_eq!(o.status.code(), Some(1));
        text(&o.stderr)
    };
    let no = answer("n\n");
    assert!(no.contains("Pooling splits one model between this device and"), "{no}");
    assert!(
        no.contains("error[policy_denied]") && no.contains("not accepted"),
        "{no}"
    );
    let yes = answer("y\n");
    assert!(yes.contains("Pooling splits one model"), "{yes}");
    assert!(
        yes.contains("error[policy_denied]") && yes.contains("licence (unknown)"),
        "{yes}"
    );
    let audit = std::fs::read_to_string(h.root.join("logs/audit.jsonl")).unwrap_or_default();
    assert!(
        !audit.contains("session_open") && !audit.contains("peer_connected"),
        "{audit}"
    );
    terminate(d, "daemon");
}
