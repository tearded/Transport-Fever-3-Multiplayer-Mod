//! A few long-lived threads for the emission step.
//!
//! The game's own thread pool sits idle while its update waits for the
//! grid, so the hook runs the step on threads of its own: started once,
//! parked between steps. Every participant (the workers and the calling
//! thread) runs the same job, which claims bands from a shared counter;
//! which thread runs which band changes nothing in the result
//! (`fused.rs`).

#![allow(unsafe_code)]

use std::sync::{Arc, Condvar, Mutex};

/// A job every participant runs with its index (0 is the caller).
type Job = dyn Fn(usize) + Sync;

struct State {
    generation: u64,
    /// The running job, its lifetime erased; only set while `run` waits.
    job: Option<*const Job>,
    /// Workers still running the current job.
    active: usize,
    panicked: bool,
}

// SAFETY: the job pointer is only dereferenced while `run`, which owns the
// job, waits for every worker to finish with it.
unsafe impl Send for State {}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    done: Condvar,
}

/// The workers, and the lock that lets one job run at a time.
pub struct Pool {
    shared: Arc<Shared>,
    workers: usize,
    serial: Mutex<()>,
}

impl Pool {
    /// A pool of `workers` threads besides the caller's (fewer if the
    /// system will not start them).
    pub fn new(workers: usize) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                generation: 0,
                job: None,
                active: 0,
                panicked: false,
            }),
            wake: Condvar::new(),
            done: Condvar::new(),
        });
        let mut started = 0;
        for index in 1..=workers {
            let shared = Arc::clone(&shared);
            let spawned = std::thread::Builder::new()
                .name(format!("tpf3mp emission {index}"))
                .stack_size(256 * 1024)
                .spawn(move || worker(&shared, index));
            if spawned.is_err() {
                break;
            }
            started += 1;
        }
        Self {
            shared,
            workers: started,
            serial: Mutex::new(()),
        }
    }

    /// How many threads run a job, the caller's included.
    pub fn participants(&self) -> usize {
        self.workers + 1
    }

    /// Runs `job` on every participant and returns once all are done.
    /// `false` if any of them panicked.
    pub fn run<'a>(&self, job: &'a (dyn Fn(usize) + Sync + 'a)) -> bool {
        let _serial = self.serial.lock().unwrap_or_else(|p| p.into_inner());
        {
            let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
            // SAFETY: only the lifetime is erased; `run` does not return
            // before every worker is done with the job (below).
            let erased: *const Job = unsafe {
                std::mem::transmute::<&'a (dyn Fn(usize) + Sync + 'a), &'static Job>(job)
            };
            state.job = Some(erased);
            state.active = self.workers;
            state.panicked = false;
            state.generation = state.generation.wrapping_add(1);
            self.shared.wake.notify_all();
        }
        let here = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(0))).is_ok();
        let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        while state.active > 0 {
            state = self
                .shared
                .done
                .wait(state)
                .unwrap_or_else(|p| p.into_inner());
        }
        state.job = None;
        here && !state.panicked
    }
}

fn worker(shared: &Shared, index: usize) {
    let mut seen = 0u64;
    loop {
        let job = {
            let mut state = shared.state.lock().unwrap_or_else(|p| p.into_inner());
            while state.generation == seen {
                state = shared.wake.wait(state).unwrap_or_else(|p| p.into_inner());
            }
            seen = state.generation;
            state.job
        };
        let ok = match job {
            // SAFETY: `run` keeps the job alive until `active` is zero.
            Some(job) => {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { (*job)(index) }))
                    .is_ok()
            }
            None => true,
        };
        let mut state = shared.state.lock().unwrap_or_else(|p| p.into_inner());
        if !ok {
            state.panicked = true;
        }
        state.active -= 1;
        if state.active == 0 {
            shared.done.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn every_participant_runs_each_job_once() {
        let pool = Pool::new(3);
        assert_eq!(pool.participants(), 4);
        for _ in 0..100 {
            let ran = AtomicUsize::new(0);
            let seen = Mutex::new(Vec::new());
            assert!(pool.run(&|i| {
                ran.fetch_add(1, Ordering::SeqCst);
                seen.lock().unwrap().push(i);
            }));
            assert_eq!(ran.load(Ordering::SeqCst), 4);
            let mut seen = seen.into_inner().unwrap();
            seen.sort_unstable();
            assert_eq!(seen, vec![0, 1, 2, 3]);
        }
        assert!(!pool.run(&|i| assert_ne!(i, 2)));
        assert!(pool.run(&|_| {}));
    }
}
