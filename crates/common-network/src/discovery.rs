//! mDNS-SD discovery (ADR-0003 D4): `_common._udp.local.`, signed TXT, no secrets. STUB.
use common_core::Result;
use common_identity::{DeviceIdentity, PeerId};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiscoveredPeer {
    pub id: PeerId,
    pub addr: SocketAddr,
    pub pairing: bool,
    pub pair_port: Option<u16>,
    pub proto: String,
    /// True when pinned in the local store (never a trust decision by itself).
    pub trusted: bool,
    pub seen_utc: String,
}

pub struct Advertiser;
impl Advertiser {
    /// Advertise (signed TXT: v, id, pair, pairport, proto, ts, sig). Dropping stops the advert.
    pub fn start(identity: &DeviceIdentity, port: u16, pairing_port: Option<u16>) -> Result<Advertiser> {
        let _ = (identity, port, pairing_port);
        Err(common_core::CommonError::new(
            common_core::ErrorCode::NotImplemented,
            "discovery",
        ))
    }
}

pub struct Browser;
impl Browser {
    pub fn start() -> Result<Browser> {
        Err(common_core::CommonError::new(
            common_core::ErrorCode::NotImplemented,
            "discovery",
        ))
    }
    /// Peers seen in the last `max_age_secs` with valid signatures. Unsigned/expired records are dropped.
    pub fn peers(&self, max_age_secs: u64) -> Vec<DiscoveredPeer> {
        let _ = max_age_secs;
        vec![]
    }
}
