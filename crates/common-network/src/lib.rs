//! Peer networking (ADR-0003).
//!
//! Invariants (tested): the only socket this crate owns is the quinn `Endpoint` (no plaintext
//! path; `tests/invariants.rs::no_plaintext_mode_exists` greps `src/` for any TCP or raw UDP
//! socket type); TLS 1.3 only; raw public keys both ways; the workload endpoint accepts only
//! pinned, non-revoked peers inside the TLS handshake; the pairing endpoint exists only during a
//! pairing window; nothing listens unless `NetworkConfig.enabled`.
//!
//! # Validating a packet capture (ADR-0003 D3, F-02-03 acceptance)
//!
//! QUIC has no cleartext mode, but "no plaintext" is demonstrated, not assumed:
//! 1. Run a remote inference whose prompt and expected completion contain unique canaries
//!    (`CANARY-<random>`), capturing every UDP datagram on the path (`tcpdump -i lo0 -w` for the
//!    SINGLE-HOST test, the LAN interface for the two-machine session; `/dev/bpf*` needs root).
//! 2. Feed the captured datagram payloads and the canaries to [`scan_for_plaintext`]: it must
//!    return no indices. It searches UTF-8 and UTF-16LE, so neither encoding leaks.
//! 3. Positive control in the same test: the identical scan over a capture of the loopback
//!    `llama-server` HTTP hop **must** find the canaries, proving the search works.
//! 4. QUIC Initial packets are only obfuscated: they carry the ClientHello (ALPN `common/1`, server
//!    name `common`, no secrets). The scan covers them too.
//!
//! Debug-only `rustls::KeyLogFile` + Wireshark may show the ciphertext did carry the canaries;
//! never in release builds.

pub mod discovery;
pub mod health;
pub mod pairing;
pub mod proto;
pub mod remote;
pub mod tls;
pub mod transport;

use serde::{Deserialize, Serialize};

pub const ALPN_WORKLOAD: &[u8] = b"common/1";
pub const ALPN_PAIRING: &[u8] = b"common-pair/1";
pub const PROTOCOL_MAJOR: u32 = 1;
pub const PROTOCOL_MINOR: u32 = 1;
/// From this minor a device that holds its peer (E5, `transport::Node::hold`) answers the peer's
/// Capabilities with `ErrorMsg{code: "peer_held"}` and the two exchange revocations (ADR-0003 D7)
/// before the holder closes with `CLOSE_HELD`; with an older peer it closes at once.
pub const HELD_EXCHANGE_MINOR: u32 = 1;
/// Hard bound on one control frame.
pub const MAX_CONTROL_FRAME: usize = 1 << 20;
/// Bound on the Capabilities control frame: signed record plus signature (F-02-04).
pub const MAX_CAPABILITIES_BYTES: usize = 64 << 10;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NetworkConfig {
    /// Nothing listens unless true (default false).
    #[serde(default)]
    pub enabled: bool,
    /// UDP port of the workload endpoint.
    #[serde(default = "default_port")]
    pub listen_port: u16,
    /// Bind address; default 0.0.0.0 only when enabled (LAN), tests use 127.0.0.1.
    #[serde(default = "default_bind")]
    pub bind_addr: String,
    #[serde(default = "default_true")]
    pub mdns: bool,
    #[serde(default = "default_keep_alive_ms")]
    pub keep_alive_ms: u64,
    #[serde(default = "default_idle_ms")]
    pub idle_timeout_ms: u64,
    #[serde(default = "default_pair_window_s")]
    pub pairing_window_secs: u64,
}
fn default_port() -> u16 {
    8193
}
fn default_bind() -> String {
    "0.0.0.0".into()
}
fn default_true() -> bool {
    true
}
/// Keep-alive 1 s / idle 4 s: a dead peer is detected within 5 s worst case (see `health`).
fn default_keep_alive_ms() -> u64 {
    1000
}
fn default_idle_ms() -> u64 {
    4000
}
fn default_pair_window_s() -> u64 {
    120
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            listen_port: 8193,
            bind_addr: default_bind(),
            mdns: true,
            keep_alive_ms: default_keep_alive_ms(),
            idle_timeout_ms: default_idle_ms(),
            pairing_window_secs: 120,
        }
    }
}

/// Indices of the datagrams that contain any canary, as UTF-8 or UTF-16LE. Empty means clean.
/// Used by the relay/packet-capture tests (see the crate docs).
pub fn scan_for_plaintext(datagrams: &[Vec<u8>], canaries: &[&str]) -> Vec<usize> {
    let needles: Vec<Vec<u8>> = canaries
        .iter()
        .flat_map(|c| {
            let utf16le: Vec<u8> = c.encode_utf16().flat_map(u16::to_le_bytes).collect();
            [c.as_bytes().to_vec(), utf16le]
        })
        .filter(|n| !n.is_empty())
        .collect();
    datagrams
        .iter()
        .enumerate()
        .filter(|(_, d)| needles.iter().any(|n| d.windows(n.len()).any(|w| w == n.as_slice())))
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_default_is_off_and_scan_finds_both_encodings() {
        assert!(!NetworkConfig::default().enabled);
        let canary = "CANARY-7f3a";
        let utf16: Vec<u8> = canary.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let datagrams = vec![
            b"\x00\x01ciphertext-looking bytes".to_vec(),
            format!("GET /?q={canary} HTTP/1.1").into_bytes(),
            [vec![0xC0], utf16, vec![0xFF]].concat(),
            canary.as_bytes()[..5].to_vec(),
        ];
        assert_eq!(scan_for_plaintext(&datagrams, &[canary]), vec![1, 2]);
        assert!(scan_for_plaintext(&datagrams, &["absent"]).is_empty());
        assert!(scan_for_plaintext(&datagrams, &[""]).is_empty());
    }
}
