//! Global FIFO slot dispatcher. Every client connection — one OpenCode, five
//! OpenCodes, an n8n flow, a Codex session — funnels through one queue, so
//! under contention slots are granted strictly in arrival order instead of
//! letting freshly-arrived requests win wakeup races against long waiters.
//!
//! The dispatcher is the only `Pool::reserve` caller in the app, and it holds
//! the [`PoolHandle`] read lock across each reserve — so a settings-driven
//! pool swap (which takes the write lock) can never interleave with a grant.
//! Grants carry the `Arc<Pool>` that made them (see [`Slot`]) so cooldown and
//! release always land on the granting pool; a late op on a retired pool is
//! benign because nothing consults it anymore.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use metrics::{counter, gauge};
use tokio::sync::{mpsc, oneshot};

use crate::pool::{Pool, PoolHandle, Reservation};

/// Minimum gap between consecutive slot grants. Caps burst *concurrency*
/// (a cold pool can grant its full aggregate RPM instantly — hundreds of
/// simultaneous connects look like a stampede to the upstream and skew
/// arrival timing) without capping throughput: 25ms = 2,400 grants/min,
/// far beyond any realistic key pool's aggregate RPM.
const GRANT_GAP: Duration = Duration::from_millis(25);

/// A granted reservation: the key to send with, and the pool that granted it
/// so follow-up cooldown/release ops route to the right generation.
pub struct Slot {
    pub pool: Arc<Pool>,
    pub lane: usize,
    pub key: String,
}

pub struct Dispatcher {
    queue: mpsc::UnboundedSender<Waiter>,
}

struct Waiter {
    reply: oneshot::Sender<Slot>,
    deadline: Instant,
    prefer: Option<usize>,
    used_keys: Option<Arc<Mutex<HashSet<String>>>>,
}

impl Dispatcher {
    pub fn new(pool: PoolHandle) -> Self {
        let (queue, rx) = mpsc::unbounded_channel();
        tokio::spawn(run(pool, rx));
        Self { queue }
    }

    /// Join the queue. The receiver resolves to a reserved [`Slot`], or
    /// errors if no slot can open before `deadline`. Dropping the receiver
    /// leaves the queue; a slot granted to an abandoned waiter is returned to
    /// the pool.
    pub fn acquire(&self, deadline: Instant, prefer: Option<usize>) -> oneshot::Receiver<Slot> {
        self.enqueue(deadline, prefer, None)
    }

    fn enqueue(
        &self,
        deadline: Instant,
        prefer: Option<usize>,
        used_keys: Option<Arc<Mutex<HashSet<String>>>>,
    ) -> oneshot::Receiver<Slot> {
        let (reply, rx) = oneshot::channel();
        gauge!("nimproxy_queue_depth").increment(1.0);
        let _ = self.queue.send(Waiter {
            reply,
            deadline,
            prefer,
            used_keys,
        });
        rx
    }

    /// Join the same FIFO queue with request-local racing key preferences.
    pub fn acquire_racing(
        &self,
        deadline: Instant,
        prefer: Option<usize>,
        used_keys: Arc<Mutex<HashSet<String>>>,
    ) -> oneshot::Receiver<Slot> {
        self.enqueue(deadline, prefer, Some(used_keys))
    }
}

async fn run(handle: PoolHandle, mut queue: mpsc::UnboundedReceiver<Waiter>) {
    while let Some(waiter) = queue.recv().await {
        let _leave = scopeguard(|| gauge!("nimproxy_queue_depth").decrement(1.0));
        loop {
            if waiter.reply.is_closed() {
                break; // client hung up while queued
            }
            // Snapshot + reserve under the read guard (reserve is sync and
            // lock-cheap), then drop the guard before any await.
            let (pool, reservation) = {
                let guard = handle.read().unwrap();
                let pool = guard.clone();
                let r = match &waiter.used_keys {
                    Some(keys) => pool.reserve_avoiding(waiter.prefer, &keys.lock().unwrap()),
                    None => pool.reserve(waiter.prefer),
                };
                (pool, r)
            };
            match reservation {
                Reservation::Ready {
                    lane,
                    key,
                    stamp,
                    sticky,
                } => {
                    let affinity = match waiter.prefer {
                        None => "none",
                        Some(_) if sticky => "sticky",
                        Some(_) => "spill",
                    };
                    counter!("nimproxy_affinity_total", "result" => affinity).increment(1);
                    let granted_key = waiter.used_keys.as_ref().map(|_| key.clone());
                    let slot = Slot {
                        pool: pool.clone(),
                        lane,
                        key,
                    };
                    if waiter.reply.send(slot).is_err() {
                        pool.release(lane, stamp);
                    } else {
                        // Only this FIFO task grants slots. Record the key
                        // before yielding or reserving for another waiter;
                        // abandoned grants consume neither this set nor RPM.
                        if let (Some(keys), Some(key)) = (&waiter.used_keys, granted_key) {
                            keys.lock().unwrap().insert(key);
                        }
                        tokio::time::sleep(GRANT_GAP).await;
                    }
                    break;
                }
                Reservation::Wait(wait) => {
                    if Instant::now() + wait > waiter.deadline {
                        break; // fail fast: dropped reply -> caller sees error
                    }
                    // Short sleep cap so we notice abandoned waiters promptly —
                    // and re-consult the handle, which may have been swapped
                    // for a pool with more capacity in the meantime.
                    tokio::time::sleep(wait.min(Duration::from_millis(500))).await;
                }
            }
        }
    }
}

/// Minimal drop-guard so gauges stay honest on every exit path (granted,
/// expired, abandoned, or panicked).
pub fn scopeguard<F: FnMut()>(f: F) -> impl Drop {
    struct Guard<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Guard<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    Guard(f)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::RwLock;

    fn handle(lanes: usize, rpm: usize) -> PoolHandle {
        Arc::new(RwLock::new(Arc::new(Pool::new(
            (0..lanes)
                .map(|i| crate::pool::LaneSpec {
                    key: format!("key{i}"),
                    rpm,
                    enabled: true,
                })
                .collect(),
        ))))
    }

    #[tokio::test]
    async fn grants_slots_in_order_while_capacity_remains() {
        let d = Dispatcher::new(handle(2, 1));
        let deadline = Instant::now() + Duration::from_secs(5);
        let a = d.acquire(deadline, None).await.expect("first slot");
        let b = d.acquire(deadline, None).await.expect("second slot");
        assert_eq!(a.lane, 0);
        assert_eq!(b.lane, 1);
    }

    #[tokio::test]
    async fn fails_fast_when_no_slot_can_open_before_deadline() {
        let d = Dispatcher::new(handle(1, 1));
        let deadline = Instant::now() + Duration::from_millis(200);
        d.acquire(deadline, None).await.expect("first slot");
        // Lane is at capacity for ~60s, far past the deadline.
        assert!(d.acquire(deadline, None).await.is_err());
    }

    #[tokio::test]
    async fn racing_queued_attempts_receive_distinct_available_keys() {
        let d = Dispatcher::new(handle(3, 10));
        let deadline = Instant::now() + Duration::from_secs(5);
        let used = Arc::new(Mutex::new(HashSet::new()));
        let a = d.acquire_racing(deadline, Some(0), used.clone());
        let b = d.acquire_racing(deadline, Some(0), used.clone());
        let c = d.acquire_racing(deadline, Some(0), used.clone());
        let a = a.await.expect("first racing grant");
        let b = b.await.expect("second racing grant");
        let c = c.await.expect("third racing grant");
        assert_eq!([a.key, b.key, c.key], ["key0", "key1", "key2"]);
        assert_eq!(used.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn racing_single_key_reuse_preserves_fifo_and_rpm() {
        let d = Dispatcher::new(handle(1, 3));
        let deadline = Instant::now() + Duration::from_secs(5);
        let used = Arc::new(Mutex::new(HashSet::new()));
        let a = d.acquire_racing(deadline, Some(0), used.clone());
        let ordinary = d.acquire(deadline, Some(0));
        let b = d.acquire_racing(deadline, Some(0), used.clone());
        assert_eq!(a.await.expect("first race grant").key, "key0");
        assert_eq!(
            ordinary.await.expect("interleaved ordinary grant").key,
            "key0"
        );
        assert_eq!(b.await.expect("reused race grant").key, "key0");
        assert_eq!(used.lock().unwrap().len(), 1);
        assert!(d.acquire_racing(deadline, Some(0), used).await.is_err());
    }

    #[tokio::test]
    async fn racing_grants_track_keys_across_live_lane_reordering() {
        let h = handle(2, 3);
        let d = Dispatcher::new(h.clone());
        let deadline = Instant::now() + Duration::from_secs(5);
        let used = Arc::new(Mutex::new(HashSet::new()));
        let first = d
            .acquire_racing(deadline, Some(0), used.clone())
            .await
            .expect("first racing grant");
        assert_eq!(first.key, "key0");
        {
            let mut guard = h.write().unwrap();
            *guard = Arc::new(
                guard.rebuild(
                    ["key1", "key0"]
                        .into_iter()
                        .map(|key| crate::pool::LaneSpec {
                            key: key.to_owned(),
                            rpm: 3,
                            enabled: true,
                        })
                        .collect(),
                ),
            );
        }
        let second = d
            .acquire_racing(deadline, Some(1), used.clone())
            .await
            .expect("racing grant after reorder");
        assert_eq!(second.key, "key1");
        assert_eq!(used.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn racing_abandoned_waiter_does_not_use_key_or_rpm() {
        let h = handle(1, 1);
        let d = Dispatcher::new(h.clone());
        let deadline = Instant::now() + Duration::from_secs(5);
        let used = Arc::new(Mutex::new(HashSet::new()));
        drop(d.acquire_racing(deadline, Some(0), used.clone()));
        let slot = d
            .acquire(deadline, None)
            .await
            .expect("ordinary grant after abandonment");
        assert_eq!(slot.key, "key0");
        assert!(used.lock().unwrap().is_empty());
        assert_eq!(h.read().unwrap().lane_stats()[0].in_window, 1);
    }

    #[tokio::test]
    async fn queued_waiter_is_served_after_a_capacity_raising_swap() {
        let h = handle(1, 1);
        let d = Dispatcher::new(h.clone());
        // Deadline beyond the ~61s window so the waiter queues instead of
        // failing fast.
        let deadline = Instant::now() + Duration::from_secs(120);
        let first = d.acquire(deadline, None).await.expect("first slot");
        let started = Instant::now();
        let pending = d.acquire(deadline, None);
        tokio::time::sleep(Duration::from_millis(100)).await;
        // Swap in a rebuilt pool with double the rpm — carried state keeps the
        // spent slot, the new headroom serves the waiter.
        {
            let mut guard = h.write().unwrap();
            let rebuilt = guard.rebuild(vec![crate::pool::LaneSpec {
                key: "key0".into(),
                rpm: 2,
                enabled: true,
            }]);
            *guard = Arc::new(rebuilt);
        }
        let slot = pending.await.expect("slot after swap");
        assert_eq!(slot.lane, 0);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "waiter should be served promptly after the swap, waited {:?}",
            started.elapsed()
        );
        // The first grant's pool is the retired generation; releasing on it
        // must be harmless.
        first.pool.release(first.lane, Instant::now());
    }
}
