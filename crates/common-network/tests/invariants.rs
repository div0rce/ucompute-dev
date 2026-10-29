//! Security invariants that are read off the source, not promised (THREAT_MODEL SD-03).
use std::path::Path;

/// The only network socket this crate owns is the quinn `Endpoint`. No TCP or raw UDP socket type
/// may appear anywhere in `src/` (comments included), outside the allow-list: `rpc.rs`, the
/// pooled-inference bridge and stage connect (WP-04, ADR-0005 D4), whose every bind and connect is
/// `Ipv4Addr::LOCALHOST` (checked below).
#[test]
fn no_plaintext_mode_exists() {
    const FORBIDDEN: &[&str] = &[
        "TcpListener",
        "TcpStream",
        "UdpSocket",
        "std::net::Udp",
        "socket2",
        "mio::net",
    ];
    const ALLOWED_MODULES: &[&str] = &["rpc.rs"];
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&src).unwrap() {
        let p = entry.unwrap().path();
        if p.extension().is_some_and(|e| e == "rs") {
            files.push(p);
        }
    }
    assert!(files.len() >= 7, "expected the crate's modules under {}", src.display());
    let mut hits = Vec::new();
    for f in &files {
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        if ALLOWED_MODULES.contains(&name.as_str()) {
            continue;
        }
        let text = std::fs::read_to_string(f).unwrap();
        for (n, line) in text.lines().enumerate() {
            if FORBIDDEN.iter().any(|t| line.contains(t)) {
                hits.push(format!("{name}:{}: {}", n + 1, line.trim()));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "plaintext-capable socket types in src/:\n{}",
        hits.join("\n")
    );
    // rpc.rs: TCP on loopback only. Every bind and connect names Ipv4Addr::LOCALHOST on its own
    // line, there is at least one of each, and no other address or socket type appears.
    let rpc = std::fs::read_to_string(src.join("rpc.rs")).unwrap();
    let code: Vec<&str> = rpc.lines().filter(|l| !l.trim_start().starts_with("//")).collect();
    for call in ["TcpListener::bind(", "TcpStream::connect("] {
        let uses: Vec<&&str> = code.iter().filter(|l| l.contains(call)).collect();
        assert!(!uses.is_empty(), "rpc.rs has no {call}");
        for l in uses {
            assert!(
                l.contains(&format!("{call}(Ipv4Addr::LOCALHOST, ")),
                "not loopback: {}",
                l.trim()
            );
        }
    }
    for banned in [
        "UNSPECIFIED",
        "0.0.0.0",
        "Ipv6Addr",
        "::bind",
        "connect(&",
        "UdpSocket",
        "SocketAddr::new",
        "to_socket_addrs",
    ] {
        let hits: Vec<_> = code
            .iter()
            .filter(|l| l.contains(banned) && !l.contains("TcpListener::bind((Ipv4Addr::LOCALHOST, "))
            .collect();
        assert!(hits.is_empty(), "rpc.rs: {banned}: {hits:?}");
    }
    // Endpoints are built only where ADR-0003 names them: the workload endpoint (transport.rs),
    // the pairing endpoint (pairing.rs) and the spike tests inside tls.rs.
    let transport = std::fs::read_to_string(src.join("transport.rs")).unwrap();
    assert!(transport.contains("quinn::Endpoint::server("));
    for f in &files {
        let name = f.file_name().unwrap();
        if name != "transport.rs" && name != "tls.rs" && name != "pairing.rs" {
            let text = std::fs::read_to_string(f).unwrap();
            let code: String = text
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                !code.contains("Endpoint::server(") && !code.contains("Endpoint::client("),
                "{}",
                f.display()
            );
        }
    }
}

/// Private keys and prompts never reach logs: no logging macro line in `src/` mentions key
/// material or prompts, and the identity's Debug is redacted.
#[test]
fn private_keys_never_formatted() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for entry in std::fs::read_dir(&src).unwrap() {
        let p = entry.unwrap().path();
        if p.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let text = std::fs::read_to_string(&p).unwrap();
        for (n, line) in text.lines().enumerate() {
            let l = line.trim_start();
            if l.starts_with("//") {
                continue;
            }
            let logs = l.contains("tracing::") || l.contains("println!") || l.contains("eprintln!");
            assert!(
                !(logs && (l.contains("pkcs8") || l.contains("prompt") || l.contains("secret"))),
                "{}:{}: {}",
                p.display(),
                n + 1,
                line
            );
        }
    }
    let id = common_identity::DeviceIdentity::generate().unwrap();
    let dbg = format!("{id:?}");
    assert!(dbg.contains("redacted"), "{dbg}");
    assert!(!dbg.contains(&hex::encode(id.pkcs8_der())));
}
