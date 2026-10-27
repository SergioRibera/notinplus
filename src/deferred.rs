//! Writes that wait for a pause.
//!
//! Typing or drawing changes a document many times a second; writing
//! each change to disk and announcing it to every device costs more
//! than the change itself. A deferred write runs on one background
//! thread once its key has been quiet for [`QUIET`], or [`AT_MOST`]
//! after it was first scheduled, whichever comes first. Scheduling the
//! same key again replaces the job that was waiting.

use std::collections::HashMap;
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// Quiet time after the last change before the write runs.
pub const QUIET: Duration = Duration::from_millis(400);
/// Longest a write waits while changes keep coming.
pub const AT_MOST: Duration = Duration::from_millis(1500);

type Job = Box<dyn FnOnce() + Send>;

struct Waiting {
    job: Job,
    first: Instant,
    last: Instant,
}

impl Waiting {
    fn due(&self) -> Instant {
        (self.last + QUIET).min(self.first + AT_MOST)
    }
}

#[derive(Default)]
struct Queue {
    waiting: HashMap<String, Waiting>,
    /// Jobs taken off the queue and not finished yet.
    running: usize,
}

struct Deferred {
    queue: Mutex<Queue>,
    wake: Condvar,
}

fn deferred() -> &'static Deferred {
    static DEFERRED: OnceLock<Deferred> = OnceLock::new();
    static STARTED: OnceLock<()> = OnceLock::new();
    let deferred = DEFERRED.get_or_init(|| Deferred {
        queue: Mutex::new(Queue::default()),
        wake: Condvar::new(),
    });
    STARTED.get_or_init(|| {
        let spawned = std::thread::Builder::new()
            .name("notein-deferred".to_owned())
            .spawn(move || deferred.run());
        if let Err(err) = spawned {
            log::error!("deferred writes thread: {err}");
        }
    });
    deferred
}

impl Deferred {
    fn queue(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn run(&self) {
        loop {
            let jobs = {
                let mut queue = self.queue();
                loop {
                    let now = Instant::now();
                    let next = queue.waiting.values().map(Waiting::due).min();
                    match next {
                        Some(due) if due <= now => break,
                        Some(due) => {
                            queue = self
                                .wake
                                .wait_timeout(queue, due - now)
                                .unwrap_or_else(PoisonError::into_inner)
                                .0;
                        }
                        None => {
                            queue = self
                                .wake
                                .wait(queue)
                                .unwrap_or_else(PoisonError::into_inner);
                        }
                    }
                }
                let now = Instant::now();
                let due: Vec<String> = queue
                    .waiting
                    .iter()
                    .filter(|(_, w)| w.due() <= now)
                    .map(|(key, _)| key.clone())
                    .collect();
                let jobs: Vec<Job> = due
                    .iter()
                    .filter_map(|key| queue.waiting.remove(key))
                    .map(|w| w.job)
                    .collect();
                queue.running += jobs.len();
                jobs
            };
            for job in jobs {
                job();
                self.queue().running -= 1;
                self.wake.notify_all();
            }
        }
    }

    fn take(&self, key: &str) -> Option<Job> {
        let mut queue = self.queue();
        let job = queue.waiting.remove(key)?.job;
        queue.running += 1;
        drop(queue);
        Some(job)
    }

    fn finish(&self) {
        self.queue().running -= 1;
        self.wake.notify_all();
    }
}

/// Run `job` once `key` has been quiet for a moment, replacing whatever
/// was waiting under it.
pub fn schedule(key: impl Into<String>, job: impl FnOnce() + Send + 'static) {
    let deferred = deferred();
    let key = key.into();
    let now = Instant::now();
    let mut queue = deferred.queue();
    let first = queue.waiting.get(&key).map_or(now, |w| w.first);
    queue.waiting.insert(
        key,
        Waiting {
            job: Box::new(job),
            first,
            last: now,
        },
    );
    drop(queue);
    deferred.wake.notify_all();
}

/// Run what waits under `key` now, on this thread.
pub fn run_now(key: &str) {
    let deferred = deferred();
    if let Some(job) = deferred.take(key) {
        job();
        deferred.finish();
    }
}

/// Run everything that waits now, on this thread, and wait for what
/// the background thread already started. For when the app may go away.
pub fn flush() {
    let deferred = deferred();
    let jobs: Vec<Job> = {
        let mut queue = deferred.queue();
        let jobs: Vec<Job> = queue.waiting.drain().map(|(_, w)| w.job).collect();
        queue.running += jobs.len();
        jobs
    };
    for job in jobs {
        job();
        deferred.finish();
    }
    let mut queue = deferred.queue();
    while queue.running > 0 {
        queue = deferred
            .wake
            .wait(queue)
            .unwrap_or_else(PoisonError::into_inner);
    }
    drop(queue);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn a_burst_runs_once_with_the_last_job() {
        let ran = Arc::new(AtomicUsize::new(0));
        for i in 1..=5 {
            let ran = Arc::clone(&ran);
            schedule("test.burst", move || ran.store(i, Ordering::SeqCst));
        }
        std::thread::sleep(QUIET + Duration::from_millis(300));
        assert_eq!(ran.load(Ordering::SeqCst), 5);
    }

    #[test]
    fn run_now_runs_what_waits_and_only_once() {
        let ran = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&ran);
        schedule("test.now", move || {
            counted.fetch_add(1, Ordering::SeqCst);
        });
        run_now("test.now");
        assert_eq!(ran.load(Ordering::SeqCst), 1);
        std::thread::sleep(QUIET + Duration::from_millis(300));
        assert_eq!(ran.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn flush_runs_everything_waiting() {
        let ran = Arc::new(AtomicUsize::new(0));
        for key in ["test.flush.a", "test.flush.b"] {
            let ran = Arc::clone(&ran);
            schedule(key, move || {
                ran.fetch_add(1, Ordering::SeqCst);
            });
        }
        flush();
        assert_eq!(ran.load(Ordering::SeqCst), 2);
    }
}
