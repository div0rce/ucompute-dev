//! Admission control (F-08-08): at most `max_concurrent` generations run, at most `max_queue`
//! wait, and nobody waits longer than `timeout`. Memory stays bounded because a waiting request
//! holds only its (size-limited) body.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub struct Admission {
    sem: Arc<Semaphore>,
    queued: AtomicUsize,
    pub max_concurrent: usize,
    pub max_queue: usize,
    pub timeout: Duration,
}

/// Why a request was turned away. `QueueFull` is HTTP 429, `Timeout` is HTTP 503.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejected {
    QueueFull,
    Timeout,
}

impl Rejected {
    pub fn reason(self) -> &'static str {
        match self {
            Rejected::QueueFull => "queue_full",
            Rejected::Timeout => "queue_timeout",
        }
    }
}

impl Admission {
    pub fn new(max_concurrent: usize, max_queue: usize, timeout: Duration) -> Self {
        let max_concurrent = max_concurrent.max(1);
        Self {
            sem: Arc::new(Semaphore::new(max_concurrent)),
            queued: AtomicUsize::new(0),
            max_concurrent,
            max_queue,
            timeout,
        }
    }

    /// A permit, or the typed rejection. The permit is held for the whole request (streaming included).
    pub async fn acquire(&self) -> Result<OwnedSemaphorePermit, Rejected> {
        if let Ok(p) = self.sem.clone().try_acquire_owned() {
            return Ok(p);
        }
        if self.queued.fetch_add(1, Ordering::SeqCst) >= self.max_queue {
            self.queued.fetch_sub(1, Ordering::SeqCst);
            return Err(Rejected::QueueFull);
        }
        let r = tokio::time::timeout(self.timeout, self.sem.clone().acquire_owned()).await;
        self.queued.fetch_sub(1, Ordering::SeqCst);
        match r {
            Ok(Ok(p)) => Ok(p),
            // Closed semaphore cannot happen (never closed); report it as a timeout rather than panic.
            Ok(Err(_)) | Err(_) => Err(Rejected::Timeout),
        }
    }

    pub fn queued(&self) -> usize {
        self.queued.load(Ordering::SeqCst)
    }

    pub fn inflight(&self) -> usize {
        self.max_concurrent.saturating_sub(self.sem.available_permits())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn one_slot_one_queue_five_callers() {
        let a = Arc::new(Admission::new(1, 1, Duration::from_millis(200)));
        let first = a.acquire().await.unwrap();
        assert_eq!(a.inflight(), 1);
        // One waiter fits in the queue; it is admitted when `first` is released.
        let waiter = tokio::spawn({
            let a = a.clone();
            async move { a.acquire().await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(a.queued(), 1);
        for _ in 0..3 {
            assert_eq!(a.acquire().await.unwrap_err(), Rejected::QueueFull);
        }
        drop(first);
        let second = waiter.await.unwrap().unwrap();
        assert_eq!(a.queued(), 0);
        assert_eq!(a.inflight(), 1);
        drop(second);
        assert_eq!(a.inflight(), 0);
    }

    #[tokio::test]
    async fn waiter_times_out_with_503_reason() {
        let a = Admission::new(1, 4, Duration::from_millis(30));
        let _held = a.acquire().await.unwrap();
        let t = std::time::Instant::now();
        assert_eq!(a.acquire().await.unwrap_err(), Rejected::Timeout);
        assert!(t.elapsed() >= Duration::from_millis(30));
        assert_eq!(a.queued(), 0);
        assert_eq!(Rejected::Timeout.reason(), "queue_timeout");
    }

    #[test]
    fn zero_concurrency_is_clamped_to_one() {
        assert_eq!(Admission::new(0, 0, Duration::ZERO).max_concurrent, 1);
    }
}
