//! What H's RPC bridge measures on every tunnel it pumps (WP-04 C5; ADR-0005 D4 and D6), with no
//! I/O of its own: the pump calls [`Tunnel::on_chunk`] for each chunk before forwarding it, and the
//! 1 s tick calls [`Tunnel::silence`].
//!
//! - **Counters** ([`Session`]): plaintext RPC bytes each way and direction switches, summed over
//!   the session's tunnels. These are the numbers P1 is scored on (lab `SCORING-INPUT.md`): a
//!   switch is counted per tunnel, when a chunk goes the other way from that tunnel's previous
//!   chunk, so a request and its reply count 2.
//! - **Byte cap**: a direction's bytes since the current load began ([`Session::new_load`]) above
//!   the cap trip the session. Per load, not per session: the runtime starts a split engine a
//!   second time after a lost port race (`couldn't bind`), and that start uploads R's weights again.
//! - **Silence watchdog `T_rpc`**: armed when H sends a command that makes R compute or read back
//!   (`GRAPH_COMPUTE`, `GRAPH_RECOMPUTE`, `GET_TENSOR`, `GET_TENSOR_2D`), disarmed by any byte from
//!   R. The weight upload (`SET_TENSOR`, which has no reply) never arms it (lab `C5-NUMBERS.md`).
//!
//! - **Command allowlist**: a command outside [`ALLOWED`] trips the tunnel at its first byte, so R
//!   never receives it whole. `COMM_INIT` makes rpc-server listen on 0.0.0.0, or connect to any
//!   host, at the client's word (`rpc_server::comm_init`); `-H 127.0.0.1` does not cover it.
//! - **Graph ops**: a `GRAPH_COMPUTE` node whose op carries a function pointer (`MAP_CUSTOM1..3`,
//!   `CUSTOM`) or is not an op at all trips the tunnel before the payload's last byte. R copies
//!   `op` and `op_params` unchecked (`deserialize_tensor`), and its CPU backend calls the pointer
//!   (`ggml_compute_forward_custom`).
//!
//! **Run it on R's side too.** R must not trust H: R's pump calls `on_chunk(.., Dir::ToR, ..)` on
//! the bytes from the tunnel before writing them to rpc-server. A refused chunk is never written.
//!
//! The command framing is b11609's `send_rpc_cmd`: `| cmd u8 | size u64 LE | payload |`, with the
//! command numbers of `enum rpc_cmd` in `ggml-rpc.cpp` at b11609 (`fcac19079`).
//!
//! Ported from the UCompute Core Lab (Apache-2.0): `integration/common-network/src/rpc_meter.rs` at
//! lab commit ce043ae (sha256 1e99c35e…; the allowlist and graph-op checks); Common added the per-load
//! cap base ([`Session::new_load`]).

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const GET_TENSOR: u8 = 8;
const GRAPH_COMPUTE: u8 = 10;
const GRAPH_RECOMPUTE: u8 = 16;
const GET_TENSOR_2D: u8 = 19;
const HEADER: usize = 9;

/// What a llama-server head sends its stage: the 14 commands of a real session (the replay test
/// below), `FREE_BUFFER` (unload) and `SET_TENSOR_HASH` (weights over 10 MiB; without `-c`, which
/// C3 never passes, R reads no file). Refused: `COPY_TENSOR` (9), `MEMSET_TENSOR` (17) and
/// `SET/GET_TENSOR_2D` (18, 19), which are session-local but never seen, add them only from a
/// session that needs them; `COMM_INIT`, `COMM_ALLREDUCE`, `COMM_FREE` (20-22), which the b11609
/// client never sends; and anything else.
pub const ALLOWED: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 14, 15, 16];
/// `enum ggml_op` at b11609: `MAP_CUSTOM1`..`MAP_CUSTOM3` (92-94) and `CUSTOM` (95) carry a
/// function pointer in `op_params`; `GGML_OP_COUNT` is 101.
const OP_FN_POINTERS: std::ops::RangeInclusive<u32> = 92..=95;
const OP_COUNT: u32 = 101;
/// `sizeof(rpc_tensor)` and `offsetof(rpc_tensor, op)` at b11609.
const RPC_TENSOR: usize = 296;
const OP_AT: usize = 52;
/// ponytail: a whole graph is held per tunnel to check it; the lab's largest is about 41 kB
/// (3 layers). Raise the cap if a model's graph exceeds it.
pub const GRAPH_MAX: u64 = 16 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    /// llama-server (H) to rpc-server (R).
    ToR,
    /// rpc-server to llama-server.
    ToH,
}

/// One session's totals, shared by its tunnels.
#[derive(Debug)]
pub struct Session {
    to_r: AtomicU64,
    to_h: AtomicU64,
    switches: AtomicU64,
    /// `to_r` and `to_h` when the current load began.
    base_to_r: AtomicU64,
    base_to_h: AtomicU64,
    /// Per direction and load, in bytes.
    cap: u64,
}

/// The three counters at one checkpoint (P1.6 reads them after the load, after `n_predict: 1`
/// and after `n_predict: 65`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counters {
    pub to_r: u64,
    pub to_h: u64,
    pub switches: u64,
}

/// Why the pump must stop the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trip {
    ByteCap {
        dir: Dir,
        total: u64,
        cap: u64,
    },
    /// A command outside [`ALLOWED`]; none of its bytes went on.
    Command {
        cmd: u8,
    },
    /// A graph node with a function-pointer op or an op b11609 does not have.
    GraphOp {
        op: u32,
    },
    GraphTooLarge {
        size: u64,
    },
}

impl Session {
    pub fn new(cap_per_direction: u64) -> Self {
        Session {
            to_r: AtomicU64::new(0),
            to_h: AtomicU64::new(0),
            switches: AtomicU64::new(0),
            base_to_r: AtomicU64::new(0),
            base_to_h: AtomicU64::new(0),
            cap: cap_per_direction,
        }
    }

    /// A new engine start: the cap counts from here. The counters stay cumulative.
    pub fn new_load(&self) {
        self.base_to_r
            .store(self.to_r.load(Ordering::Relaxed), Ordering::Relaxed);
        self.base_to_h
            .store(self.to_h.load(Ordering::Relaxed), Ordering::Relaxed);
    }

    pub fn counters(&self) -> Counters {
        Counters {
            to_r: self.to_r.load(Ordering::Relaxed),
            to_h: self.to_h.load(Ordering::Relaxed),
            switches: self.switches.load(Ordering::Relaxed),
        }
    }
}

/// One tunnel (one TCP connection from llama-server). Both pump directions update it, so the pump
/// keeps it behind one lock per tunnel.
#[derive(Debug, Default)]
pub struct Tunnel {
    last: Option<Dir>,
    /// The part of a command header seen so far, H to R.
    head: Vec<u8>,
    /// Payload bytes of the current command still to come.
    left: u64,
    /// The `GRAPH_COMPUTE` payload collected so far.
    graph: Option<Vec<u8>>,
    armed_at: Option<Instant>,
}

impl Tunnel {
    /// Account one chunk the pump read in `dir`, before it forwards the chunk.
    pub fn on_chunk(&mut self, s: &Session, dir: Dir, data: &[u8], now: Instant) -> Result<(), Trip> {
        if data.is_empty() {
            return Ok(());
        }
        if self.last.is_some_and(|d| d != dir) {
            s.switches.fetch_add(1, Ordering::Relaxed);
        }
        self.last = Some(dir);
        let (total, base) = match dir {
            Dir::ToR => {
                self.parse(data, now)?;
                (s.to_r.fetch_add(data.len() as u64, Ordering::Relaxed), &s.base_to_r)
            }
            Dir::ToH => {
                self.armed_at = None;
                (s.to_h.fetch_add(data.len() as u64, Ordering::Relaxed), &s.base_to_h)
            }
        };
        let total = (total + data.len() as u64).saturating_sub(base.load(Ordering::Relaxed));
        if total > s.cap {
            return Err(Trip::ByteCap { dir, total, cap: s.cap });
        }
        Ok(())
    }

    /// How long R has been silent while H waits on it; None when the watchdog is not armed.
    pub fn silence(&self, now: Instant) -> Option<Duration> {
        self.armed_at.map(|t| now.saturating_duration_since(t))
    }

    fn parse(&mut self, mut data: &[u8], now: Instant) -> Result<(), Trip> {
        while !data.is_empty() {
            if self.left > 0 {
                let n = self.left.min(data.len() as u64) as usize;
                if let Some(g) = &mut self.graph {
                    g.extend_from_slice(&data[..n]);
                }
                self.left -= n as u64;
                data = &data[n..];
                if self.left == 0 {
                    if let Some(g) = self.graph.take() {
                        check_graph(&g)?;
                    }
                }
                continue;
            }
            if self.head.is_empty() && !ALLOWED.contains(&data[0]) {
                return Err(Trip::Command { cmd: data[0] });
            }
            let n = (HEADER - self.head.len()).min(data.len());
            self.head.extend_from_slice(&data[..n]);
            data = &data[n..];
            if self.head.len() == HEADER {
                let cmd = self.head[0];
                self.left = u64::from_le_bytes(self.head[1..].try_into().expect("8 bytes"));
                self.head.clear();
                if cmd == GRAPH_COMPUTE && self.left > 0 {
                    if self.left > GRAPH_MAX {
                        return Err(Trip::GraphTooLarge { size: self.left });
                    }
                    self.graph = Some(Vec::with_capacity(self.left as usize));
                }
                if matches!(cmd, GET_TENSOR | GRAPH_COMPUTE | GRAPH_RECOMPUTE | GET_TENSOR_2D) {
                    // ponytail: the silence also counts the bytes queued ahead of this command on
                    // the link; at a slow link with a full QUIC window right after the upload (the
                    // load's warmup), add that drain time to T_rpc
                    self.armed_at.get_or_insert(now);
                }
            }
        }
        Ok(())
    }
}

/// The op of every tensor R would deserialize from a `GRAPH_COMPUTE` payload, laid out as
/// `| device u32 | uid u64 | n_nodes u32 | nodes u64 * n_nodes | n_tensors u32 | rpc_tensor * n_tensors |`.
/// A payload too short for its counts is not checked further: R refuses it unread.
fn check_graph(p: &[u8]) -> Result<(), Trip> {
    let u32_at = |i: usize| {
        p.get(i..i + 4)
            .map(|b| u32::from_le_bytes(b.try_into().expect("4 bytes")))
    };
    let Some(n_nodes) = u32_at(12) else { return Ok(()) };
    let at = 16 + n_nodes as usize * 8;
    let Some(n_tensors) = u32_at(at) else { return Ok(()) };
    // `as_chunks` is Rust 1.88; the workspace's rust-version is 1.85
    #[allow(clippy::chunks_exact_to_as_chunks)]
    for t in p[at + 4..].chunks_exact(RPC_TENSOR).take(n_tensors as usize) {
        let op = u32::from_le_bytes(t[OP_AT..OP_AT + 4].try_into().expect("4 bytes"));
        if op >= OP_COUNT || OP_FN_POINTERS.contains(&op) {
            return Err(Trip::GraphOp { op });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(c: u8, payload: usize) -> Vec<u8> {
        let mut v = vec![c];
        v.extend_from_slice(&(payload as u64).to_le_bytes());
        v.resize(HEADER + payload, 0xab);
        v
    }

    // A REAL session (lab tools/rpc_capture.py, this Mac, b11609, 2026-10-24): R0 configuration A
    // behind a recording proxy, every chunk in the proxy's read order with its real length and
    // timing, payload bytes zeroed and the real command headers kept. K lines carry the proxy's own
    // totals after the load, after n_predict 1 and after n_predict 65.
    const R0A: &str = include_str!("fixtures/rpc-session-b11609-r0a.txt");

    #[test]
    fn a_real_session_replays_to_the_recording_proxys_totals() {
        let s = Session::new(u64::MAX);
        let mut tunnels: Vec<Tunnel> = Vec::new();
        let t0 = Instant::now();
        let (mut longest, mut checkpoints) = (Duration::ZERO, Vec::new());
        // arm-then-reply episodes: as the meter sees them, and as the recorded headers imply them
        // (the recorder located headers with the lab's Python parser; the meter re-parses the bytes)
        let (mut episodes, mut expected) = (0, 0);
        let mut implied: Vec<(usize, bool)> = Vec::new(); // per tunnel: header bytes still to come, armed
        for line in R0A.lines().filter(|l| !l.starts_with('#')) {
            let f: Vec<&str> = line.split(' ').collect();
            if f[0] == "K" {
                let n = |i: usize| f[i].parse::<u64>().unwrap();
                let proxy = Counters {
                    to_r: n(2),
                    to_h: n(3),
                    switches: n(4),
                };
                assert_eq!(s.counters(), proxy, "at {}", f[1]);
                checkpoints.push(proxy);
                continue;
            }
            let (tunnel, dir, now) = (
                f[1].parse::<usize>().unwrap(),
                f[2],
                t0 + Duration::from_micros(f[3].parse().unwrap()),
            );
            let mut chunk = vec![0u8; f[4].parse().unwrap()];
            if implied.len() <= tunnel {
                implied.resize(tunnel + 1, (0, false));
            }
            for h in &f[5..] {
                let (off, hex) = h.split_once(':').unwrap();
                if let Some(off) = off.strip_prefix('g') {
                    // a GRAPH_COMPUTE payload's real bytes: placed, not a header
                    let off: usize = off.parse().unwrap();
                    for i in 0..hex.len() / 2 {
                        chunk[off + i] = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap();
                    }
                    continue;
                }
                let off: usize = off.parse().unwrap();
                let (carry, armed) = &mut implied[tunnel];
                if *carry > 0 {
                    *carry -= hex.len() / 2; // the rest of a header split across chunks
                } else {
                    *carry = HEADER - hex.len() / 2;
                    *armed |= matches!(u8::from_str_radix(&hex[..2], 16).unwrap(), 8 | 10 | 16 | 19);
                }
                for i in 0..hex.len() / 2 {
                    chunk[off + i] = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap();
                }
            }
            if tunnels.len() <= tunnel {
                tunnels.resize_with(tunnel + 1, Tunnel::default);
            }
            for t in &tunnels {
                longest = longest.max(t.silence(now).unwrap_or_default());
            }
            let dir = if dir == "0" { Dir::ToR } else { Dir::ToH };
            if dir == Dir::ToH {
                episodes += usize::from(tunnels[tunnel].silence(now).is_some());
                expected += usize::from(std::mem::take(&mut implied[tunnel].1));
            }
            tunnels[tunnel].on_chunk(&s, dir, &chunk, now).unwrap();
        }
        // P1.6's decode-only method on the meter's own counters gives R0's published numbers
        // (rpc-dataflow/results-r0/results.json, configuration A, decode_only)
        let [loaded, one, full] = checkpoints[..] else {
            panic!("three checkpoints")
        };
        let per = |a: u64, b: u64, c: u64| ((c - b) as f64 - (b - a) as f64) / 64.0;
        assert_eq!(per(loaded.to_r, one.to_r, full.to_r), 6696.765625);
        assert_eq!(per(loaded.to_h, one.to_h, full.to_h), 1159.25);
        assert_eq!(per(loaded.switches, one.switches, full.switches) / 2.0, 0.9609375);
        // every command of the real session is in ALLOWED (on_chunk would have tripped above)
        // healthy on loopback: R never stays silent anywhere near T_rpc (10 s) while H waits on it
        assert!(longest < Duration::from_secs(1), "longest armed silence {longest:?}");
        // every compute or readback got its reply, the meter armed for each, and for nothing else:
        // at least the prefill of both requests and 64 decode steps
        assert_eq!(episodes, expected);
        assert!(episodes >= 66, "{episodes} episodes");
    }

    #[test]
    fn commands_outside_the_allowlist_are_refused_at_their_first_byte() {
        let now = Instant::now();
        for cmd in [9u8, 17, 18, 19, 20, 21, 22, 23, 24, 255] {
            let s = Session::new(u64::MAX);
            let mut t = Tunnel::default();
            assert_eq!(t.on_chunk(&s, Dir::ToR, &[cmd], now), Err(Trip::Command { cmd }));
            assert_eq!(s.counters().to_r, 0, "a refused chunk is not forwarded, so not counted");
        }
        // after an allowed command whose payload ends in the same chunk
        let s = Session::new(u64::MAX);
        let mut t = Tunnel::default();
        let stream = [cmd(6, 20), cmd(20, 40)].concat();
        assert_eq!(t.on_chunk(&s, Dir::ToR, &stream, now), Err(Trip::Command { cmd: 20 }));
    }

    /// A GRAPH_COMPUTE payload with one rpc_tensor per op.
    fn graph(ops: &[u32]) -> Vec<u8> {
        let mut p = Vec::new();
        p.extend_from_slice(&0u32.to_le_bytes()); // device
        p.extend_from_slice(&7u64.to_le_bytes()); // uid
        p.extend_from_slice(&1u32.to_le_bytes()); // n_nodes
        p.extend_from_slice(&1u64.to_le_bytes()); // node ids
        p.extend_from_slice(&(ops.len() as u32).to_le_bytes());
        for (i, op) in ops.iter().enumerate() {
            let mut t = vec![0u8; RPC_TENSOR];
            t[..8].copy_from_slice(&(i as u64 + 1).to_le_bytes());
            t[OP_AT..OP_AT + 4].copy_from_slice(&op.to_le_bytes());
            p.extend_from_slice(&t);
        }
        let mut v = vec![GRAPH_COMPUTE];
        v.extend_from_slice(&(p.len() as u64).to_le_bytes());
        v.extend_from_slice(&p);
        v
    }

    #[test]
    fn a_function_pointer_op_is_refused_before_the_graphs_last_byte() {
        let now = Instant::now();
        for bad in [92u32, 93, 94, 95, 101, u32::MAX] {
            let s = Session::new(u64::MAX);
            let mut t = Tunnel::default();
            let g = graph(&[1, 0, bad, 2]);
            let (body, last) = g.split_at(g.len() - 1);
            for c in body.chunks(1000) {
                t.on_chunk(&s, Dir::ToR, c, now).unwrap();
            }
            assert_eq!(t.on_chunk(&s, Dir::ToR, last, now), Err(Trip::GraphOp { op: bad }));
            assert_eq!(s.counters().to_r, body.len() as u64, "R never got the last byte");
        }
        // ordinary ops pass, and the stream goes on at the next command
        let s = Session::new(u64::MAX);
        let mut t = Tunnel::default();
        t.on_chunk(&s, Dir::ToR, &[graph(&[0, 1, 91, 96, 100]), cmd(8, 40)].concat(), now)
            .unwrap();
        assert!(t.silence(now).is_some());
        assert_eq!(
            t.on_chunk(&s, Dir::ToR, &graph(&[]), now),
            Ok(()),
            "an empty graph has nothing to refuse"
        );
    }

    #[test]
    fn an_oversized_graph_is_refused_at_its_header() {
        let mut h = vec![GRAPH_COMPUTE];
        h.extend_from_slice(&(GRAPH_MAX + 1).to_le_bytes());
        let mut t = Tunnel::default();
        assert_eq!(
            t.on_chunk(&Session::new(u64::MAX), Dir::ToR, &h, Instant::now()),
            Err(Trip::GraphTooLarge { size: GRAPH_MAX + 1 })
        );
    }

    // SYNTHETIC streams in b11609's framing, shaped like the lab's R0 decode step: SET_TENSOR inputs,
    // GRAPH_RECOMPUTE, GET_TENSOR, and R's reply.
    #[test]
    fn a_decode_step_is_one_turn_and_its_bytes() {
        let s = Session::new(u64::MAX);
        let mut t = Tunnel::default();
        let t0 = Instant::now();
        let step: Vec<u8> = [cmd(6, 1152 + 305), cmd(16, 12), cmd(8, 40)].concat();
        for i in 0..64 {
            let now = t0 + Duration::from_millis(10 * i);
            t.on_chunk(&s, Dir::ToR, &step, now).unwrap();
            assert!(t.silence(now).is_some(), "armed by GET_TENSOR");
            t.on_chunk(&s, Dir::ToH, &[0; 1160], now + Duration::from_millis(3))
                .unwrap();
            assert_eq!(t.silence(now), None, "R's reply disarms");
        }
        let c = s.counters();
        assert_eq!((c.to_r, c.to_h), (64 * step.len() as u64, 64 * 1160));
        // the lab's proxy rule: no switch for the first chunk, then one per change of direction
        assert_eq!(c.switches, 2 * 64 - 1);
    }

    #[test]
    fn the_upload_never_arms_the_watchdog() {
        let s = Session::new(u64::MAX);
        let mut t = Tunnel::default();
        let now = Instant::now();
        // a 100 MB SET_TENSOR arriving in 64 KiB chunks, its header split across two chunks; the
        // payload happens to contain GET_TENSOR's byte at chunk starts
        let mut stream = cmd(6, 0);
        stream[1..9].copy_from_slice(&100_000_000u64.to_le_bytes());
        t.on_chunk(&s, Dir::ToR, &stream[..4], now).unwrap();
        t.on_chunk(&s, Dir::ToR, &stream[4..], now).unwrap();
        let chunk = [GET_TENSOR; 65536];
        let mut sent = 0;
        while sent + chunk.len() <= 100_000_000 {
            t.on_chunk(&s, Dir::ToR, &chunk, now).unwrap();
            sent += chunk.len();
        }
        t.on_chunk(&s, Dir::ToR, &chunk[..100_000_000 - sent], now).unwrap();
        assert_eq!(t.silence(now), None);
        // the next command after the payload is parsed at its real boundary
        let later = now + Duration::from_secs(20);
        t.on_chunk(&s, Dir::ToR, &cmd(10, 500), later).unwrap();
        assert_eq!(
            t.silence(later + Duration::from_secs(11)),
            Some(Duration::from_secs(11))
        );
        assert_eq!(s.counters().switches, 0, "one direction only");
    }

    #[test]
    fn header_split_across_chunks_still_arms_at_its_completion() {
        let s = Session::new(u64::MAX);
        let mut t = Tunnel::default();
        let t0 = Instant::now();
        let g = cmd(8, 40);
        t.on_chunk(&s, Dir::ToR, &g[..3], t0).unwrap();
        assert_eq!(t.silence(t0), None);
        let t1 = t0 + Duration::from_millis(5);
        t.on_chunk(&s, Dir::ToR, &g[3..], t1).unwrap();
        assert_eq!(t.silence(t1 + Duration::from_secs(1)), Some(Duration::from_secs(1)));
        // a second arming command while armed keeps the first arming time
        t.on_chunk(&s, Dir::ToR, &cmd(10, 8), t1 + Duration::from_secs(2))
            .unwrap();
        assert_eq!(t.silence(t1 + Duration::from_secs(3)), Some(Duration::from_secs(3)));
    }

    #[test]
    fn switches_are_per_tunnel_and_totals_per_session() {
        let s = Session::new(u64::MAX);
        let (mut a, mut b) = (Tunnel::default(), Tunnel::default());
        let now = Instant::now();
        a.on_chunk(&s, Dir::ToR, &cmd(14, 0), now).unwrap(); // HELLO on tunnel a
        b.on_chunk(&s, Dir::ToH, &[1], now).unwrap(); // tunnel b's first chunk: not a switch
        a.on_chunk(&s, Dir::ToH, &[1, 2, 3], now).unwrap(); // a: ToR -> ToH
        assert_eq!(
            s.counters(),
            Counters {
                to_r: 9,
                to_h: 4,
                switches: 1
            }
        );
        a.on_chunk(&s, Dir::ToR, &[], now).unwrap();
        assert_eq!(s.counters().switches, 1, "an empty read is not a chunk");
    }

    /// A second engine start (the runtime's port-race retry) uploads the weights again: each load
    /// gets the whole cap, and the counters P1 reads stay cumulative.
    #[test]
    fn byte_cap_counts_per_load() {
        let s = Session::new(100);
        let now = Instant::now();
        let mut first = Tunnel::default();
        first.on_chunk(&s, Dir::ToR, &cmd(6, 91), now).unwrap();
        first.on_chunk(&s, Dir::ToH, &[0; 100], now).unwrap();
        s.new_load();
        let mut second = Tunnel::default();
        second.on_chunk(&s, Dir::ToR, &cmd(6, 91), now).unwrap();
        second.on_chunk(&s, Dir::ToH, &[0; 100], now).unwrap();
        assert_eq!((s.counters().to_r, s.counters().to_h), (200, 200));
        assert_eq!(
            second.on_chunk(&s, Dir::ToH, &[0], now),
            Err(Trip::ByteCap {
                dir: Dir::ToH,
                total: 101,
                cap: 100
            })
        );
    }

    #[test]
    fn byte_cap_trips_above_the_cap_per_direction() {
        let s = Session::new(100);
        let mut t = Tunnel::default();
        let now = Instant::now();
        t.on_chunk(&s, Dir::ToH, &[0; 100], now).unwrap();
        t.on_chunk(&s, Dir::ToR, &cmd(6, 91), now).unwrap(); // exactly 100: allowed
        assert_eq!(
            t.on_chunk(&s, Dir::ToR, &[0], now),
            Err(Trip::ByteCap {
                dir: Dir::ToR,
                total: 101,
                cap: 100
            })
        );
    }
}
