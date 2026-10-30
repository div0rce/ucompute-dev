//! Local work first on CPU engines (D-026). Measured on sentinel (fix round 7): a peer generation
//! batched with local work on a CPU engine slows every local token by about 35-50 %, and slot
//! reservation cannot prevent it (it limits slots, not compute). Metal and other GPU engines are
//! not affected, where a second sequence costs little. The device's owner chooses ([`LocalPriority`],
//! `config.json` `"local_priority"`, `PUT /api/network`):
//!
//! - `refuse_new` (the default): while a local generation runs on a CPU engine, or for
//!   [`WINDOW`] after the last one ended, a new peer generation is refused with the typed,
//!   retryable `overloaded`; peer generations already running finish.
//! - `preempt` (opt-in): as `refuse_new`, and a local generation starting on a CPU engine also
//!   cancels the running peer generations, each ending with `overloaded`. This breaks "admitted
//!   requests complete" (F-08-08) for peers, which is why it is never the default.
//! - `none`: no local-priority rule (the behaviour before D-026).
//!
//! Local requests are never refused by this rule. One engine runs at a time (`ModelManager`), so
//! "the CPU engine" is the device's engine whenever it runs on the CPU.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use ucompute_core::{ErrorCode, Result, UComputeError};
use ucompute_runtime::LoadedModelInfo;

/// How long after a local generation on a CPU engine ends new peer generations are still refused.
pub const WINDOW: Duration = Duration::from_secs(2);

/// The message a refused or preempted peer reads.
pub const BUSY: &str = "this device is busy with its own work; retry later";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum LocalPriority {
    #[default]
    RefuseNew,
    Preempt,
    None,
}

/// An engine this rule covers: one running on the CPU (`LoadedModelInfo::device_actual == "cpu"`).
pub fn on_cpu(info: &LoadedModelInfo) -> bool {
    info.device_actual == "cpu"
}

fn busy() -> UComputeError {
    UComputeError::new(ErrorCode::Overloaded, BUSY)
}

#[derive(Default)]
struct State {
    running: usize,
    last_end: Option<Instant>,
    next: u64,
    peers: HashMap<u64, (CancellationToken, Arc<AtomicBool>)>,
}

/// Local generations on the CPU engine and the peer generations running on it. One per daemon,
/// shared by the API (local) and `remote::ServeOptions::local` (peers).
#[derive(Default)]
pub struct LocalWork {
    mode: AtomicU8,
    state: Mutex<State>,
}

impl LocalWork {
    pub fn new(mode: LocalPriority) -> Arc<Self> {
        let w = Arc::new(Self::default());
        w.set_mode(mode);
        w
    }

    pub fn set_mode(&self, mode: LocalPriority) {
        self.mode.store(mode as u8, Ordering::SeqCst);
    }

    pub fn mode(&self) -> LocalPriority {
        match self.mode.load(Ordering::SeqCst) {
            1 => LocalPriority::Preempt,
            2 => LocalPriority::None,
            _ => LocalPriority::RefuseNew,
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A local generation starts on the CPU engine; it counts as running until the guard drops.
    /// In `preempt` mode every running peer generation is cancelled (and reads `overloaded`).
    pub fn local_started(self: &Arc<Self>) -> LocalGuard {
        let mut s = self.state();
        s.running += 1;
        if self.mode() == LocalPriority::Preempt {
            for (cancel, preempted) in s.peers.values() {
                preempted.store(true, Ordering::SeqCst);
                cancel.cancel();
            }
        }
        LocalGuard(self.clone())
    }

    /// A peer generation about to start on the CPU engine: refused with `overloaded` under
    /// `refuse_new` and `preempt` while local work runs or within [`WINDOW`] of its end, else
    /// registered (so `preempt` can cancel it through `cancel`) until the guard drops.
    pub fn peer_started(self: &Arc<Self>, cancel: CancellationToken) -> Result<PeerGuard> {
        self.peer_started_at(cancel, Instant::now())
    }

    /// [`Self::peer_started`] at a given instant (the injectable clock of the unit tests).
    pub fn peer_started_at(self: &Arc<Self>, cancel: CancellationToken, now: Instant) -> Result<PeerGuard> {
        let mut s = self.state();
        let recent = s.last_end.is_some_and(|t| now.saturating_duration_since(t) < WINDOW);
        if self.mode() != LocalPriority::None && (s.running > 0 || recent) {
            return Err(busy());
        }
        let id = s.next;
        s.next += 1;
        let preempted = Arc::new(AtomicBool::new(false));
        s.peers.insert(id, (cancel, preempted.clone()));
        Ok(PeerGuard {
            work: self.clone(),
            id,
            preempted,
        })
    }

    fn local_ended_at(&self, now: Instant) {
        let mut s = self.state();
        s.running = s.running.saturating_sub(1);
        s.last_end = Some(now);
    }
}

/// A local generation on the CPU engine; its end starts the [`WINDOW`].
pub struct LocalGuard(Arc<LocalWork>);

impl Drop for LocalGuard {
    fn drop(&mut self) {
        self.0.local_ended_at(Instant::now());
    }
}

/// A peer generation on the CPU engine.
pub struct PeerGuard {
    work: Arc<LocalWork>,
    id: u64,
    preempted: Arc<AtomicBool>,
}

impl PeerGuard {
    /// Cancelled by local work (`preempt`): the generation's end is reported as `overloaded`.
    pub fn preempted(&self) -> bool {
        self.preempted.load(Ordering::SeqCst)
    }

    /// `e` as the peer should read it: [`BUSY`] when local work cancelled the generation.
    pub fn map_err(&self, e: UComputeError) -> UComputeError {
        if self.preempted() {
            busy()
        } else {
            e
        }
    }
}

impl Drop for PeerGuard {
    fn drop(&mut self) {
        self.work.state().peers.remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(r: Result<PeerGuard>) -> bool {
        matches!(r, Err(e) if e.code == ErrorCode::Overloaded && e.message == BUSY)
    }

    #[test]
    fn refuse_new_refuses_during_local_work_and_for_two_seconds_after() {
        let w = LocalWork::new(LocalPriority::default());
        assert_eq!(w.mode(), LocalPriority::RefuseNew, "the default is B");
        let t0 = Instant::now();
        assert!(
            w.peer_started_at(CancellationToken::new(), t0).is_ok(),
            "idle: accepted"
        );
        let local = w.local_started();
        assert!(refused(w.peer_started_at(CancellationToken::new(), t0)));
        drop(local);
        let end = w.state().last_end.unwrap();
        for after in [0, 1, 1999] {
            assert!(
                refused(w.peer_started_at(CancellationToken::new(), end + Duration::from_millis(after))),
                "{after} ms after the end"
            );
        }
        assert!(
            w.peer_started_at(CancellationToken::new(), end + WINDOW).is_ok(),
            "2 s after"
        );
        assert!(w
            .peer_started_at(CancellationToken::new(), end + Duration::from_secs(60))
            .is_ok());
    }

    #[test]
    fn refuse_new_lets_a_running_peer_generation_finish() {
        let w = LocalWork::new(LocalPriority::RefuseNew);
        let cancel = CancellationToken::new();
        let peer = w.peer_started(cancel.clone()).unwrap();
        let _local = w.local_started();
        assert!(!cancel.is_cancelled() && !peer.preempted());
        let e = UComputeError::new(ErrorCode::Cancelled, "x");
        assert_eq!(
            peer.map_err(e).code,
            ErrorCode::Cancelled,
            "its own errors stay its own"
        );
    }

    #[test]
    fn two_local_generations_keep_the_rule_until_both_end() {
        let w = LocalWork::new(LocalPriority::RefuseNew);
        let (a, b) = (w.local_started(), w.local_started());
        drop(a);
        let far = Instant::now() + Duration::from_secs(60);
        assert!(
            refused(w.peer_started_at(CancellationToken::new(), far)),
            "b still runs"
        );
        drop(b);
        assert!(w.peer_started_at(CancellationToken::new(), far).is_ok());
    }

    #[test]
    fn preempt_cancels_running_peer_generations_as_overloaded() {
        let w = LocalWork::new(LocalPriority::Preempt);
        let (c1, c2) = (CancellationToken::new(), CancellationToken::new());
        let (p1, p2) = (w.peer_started(c1.clone()).unwrap(), w.peer_started(c2.clone()).unwrap());
        let ended = CancellationToken::new();
        drop(w.peer_started(ended.clone()).unwrap()); // finished before local work: untouched
        let local = w.local_started();
        assert!(c1.is_cancelled() && c2.is_cancelled() && !ended.is_cancelled());
        let e = p1.map_err(UComputeError::new(ErrorCode::Cancelled, "generation cancelled"));
        assert_eq!((e.code, e.message.as_str()), (ErrorCode::Overloaded, BUSY));
        assert!(p2.preempted());
        assert!(
            refused(w.peer_started(CancellationToken::new())),
            "new ones refused too"
        );
        drop(local);
        assert!(w
            .peer_started_at(CancellationToken::new(), Instant::now() + WINDOW)
            .is_ok());
    }

    #[test]
    fn none_accepts_and_never_cancels() {
        let w = LocalWork::new(LocalPriority::None);
        let cancel = CancellationToken::new();
        let peer = w.peer_started(cancel.clone()).unwrap();
        let _local = w.local_started();
        assert!(w.peer_started(CancellationToken::new()).is_ok());
        assert!(!cancel.is_cancelled() && !peer.preempted());
    }

    #[test]
    fn a_mode_change_applies_to_the_next_peer_request() {
        let w = LocalWork::new(LocalPriority::None);
        let _local = w.local_started();
        assert!(w.peer_started(CancellationToken::new()).is_ok());
        w.set_mode(LocalPriority::RefuseNew);
        assert!(refused(w.peer_started(CancellationToken::new())));
    }

    #[test]
    fn serde_names_and_unknown_values() {
        for (m, s) in [
            (LocalPriority::RefuseNew, "\"refuse_new\""),
            (LocalPriority::Preempt, "\"preempt\""),
            (LocalPriority::None, "\"none\""),
        ] {
            assert_eq!(serde_json::to_string(&m).unwrap(), s);
            assert_eq!(serde_json::from_str::<LocalPriority>(s).unwrap(), m);
        }
        assert!(serde_json::from_str::<LocalPriority>("\"always\"").is_err());
    }

    #[test]
    fn only_cpu_engines_are_covered() {
        let mut info = LoadedModelInfo {
            device_actual: "cpu".into(),
            ..Default::default()
        };
        assert!(on_cpu(&info));
        info.device_actual = "Metal0 (Apple M2)".into();
        assert!(!on_cpu(&info));
    }
}
