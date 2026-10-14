//! Peer networking (ADR-0003). STUB — implementation pending.
//!
//! Invariants (tested): the only socket this crate owns is the quinn `Endpoint` (no plaintext
//! path); TLS 1.3 only; raw public keys both ways; the workload endpoint accepts only pinned,
//! non-revoked peers inside the TLS handshake; the pairing endpoint exists only during a pairing
//! window; nothing listens unless `NetworkConfig.enabled`.

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
pub const PROTOCOL_MINOR: u32 = 0;
/// Hard bound on one control frame.
pub const MAX_CONTROL_FRAME: usize = 1 << 20;
/// Bound on the Capabilities record (F-02-04).
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
fn default_keep_alive_ms() -> u64 {
    2000
}
fn default_idle_ms() -> u64 {
    6000
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
            keep_alive_ms: 2000,
            idle_timeout_ms: 6000,
            pairing_window_secs: 120,
        }
    }
}
