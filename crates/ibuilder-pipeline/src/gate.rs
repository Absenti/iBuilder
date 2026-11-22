//! `ShaderMemoryGate`: holds a new translation (or split job) back while less than `LOW` is left under the ceiling and another one is
//! running -- the running ones finish and give their memory back first. One always runs, so the build cannot stall. The same gate asks
//! the `ThermalGate`: a hot phone starts nothing new (state serious or worse), and Low performance mode runs at most 2 at once
//! (DECISIONS traps 71 and 76).

use crate::log::Log;
use crate::memory::MemSource;
use crate::thermal::{Clock, ThermalGate};
use crate::util::MIB;
use std::sync::{Arc, Mutex};

struct State {
    active: usize,
    waits: u64,
}

pub struct ShaderMemoryGate {
    mem: Arc<dyn MemSource>,
    log: Arc<dyn Log>,
    thermal: Option<Arc<ThermalGate>>,
    clock: Arc<dyn Clock>,
    st: Mutex<State>,
}

impl ShaderMemoryGate {
    /// A translation waits while less than this is left and another is running.
    pub const LOW: u64 = 900 * MIB;
    /// ... but never longer than this many seconds (the Python's `waited >= 30`).
    pub const MAX_WAIT_S: f64 = 30.0;

    pub fn new(
        mem: Arc<dyn MemSource>,
        log: Arc<dyn Log>,
        thermal: Option<Arc<ThermalGate>>,
        clock: Arc<dyn Clock>,
    ) -> ShaderMemoryGate {
        ShaderMemoryGate {
            mem,
            log,
            thermal,
            clock,
            st: Mutex::new(State {
                active: 0,
                waits: 0,
            }),
        }
    }

    fn left(&self) -> u64 {
        self.mem.available()
    }

    /// How many jobs are running through the gate right now.
    pub fn active(&self) -> usize {
        self.st.lock().unwrap().active
    }

    /// How many times a job had to wait.
    pub fn waits(&self) -> u64 {
        self.st.lock().unwrap().waits
    }

    /// Run `f` when the gate lets it through (the Python `wrap(fn)(*args)`).
    pub fn run<R>(&self, f: impl FnOnce() -> R) -> R {
        let mut waited = 0.0f64;
        if let Some(t) = &self.thermal {
            t.wait(2, "stage 3");
        }
        loop {
            // what the app is asked (the memory, a syscall; the thermal cap) is read before the lock, and its log is written after it: the other workers never wait behind them (review r3 §5.4)
            let left = self.left();
            let cap = self.thermal.as_ref().and_then(|t| t.parallel_cap());
            let mut say = None;
            {
                let mut st = self.st.lock().unwrap();
                let capped = cap.map(|c| st.active >= c).unwrap_or(false);
                // a pause or cancel (`stop`) lets the job through at once: the build is winding down, and waiting for memory or for the other jobs would only delay it
                let stopping = crate::stop::requested() != crate::stop::NONE;
                if st.active == 0
                    || stopping
                    || (!capped && (left == 0 || left >= Self::LOW || waited >= Self::MAX_WAIT_S))
                {
                    st.active += 1;
                    break;
                }
                if waited == 0.0 && !capped {
                    st.waits += 1;
                    if matches!(st.waits, 1 | 10 | 100 | 1000) {
                        say = Some(st.waits);
                    }
                }
            }
            if let Some(waits) = say {
                self.log.line(&format!(
                    "stage 3 shaders: {} MiB left: a translation waits for the running ones to finish ({waits} so far)",
                    left / MIB
                ));
            }
            self.clock.sleep(0.25);
            waited += 0.25;
        }
        struct Done<'a>(&'a ShaderMemoryGate);
        impl Drop for Done<'_> {
            fn drop(&mut self) {
                self.0.st.lock().unwrap().active -= 1;
            }
        }
        let _done = Done(self);
        f()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemInfo;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{OnceLock, Weak};

    /// Counts the calls the gate makes into the app's memory source and log, and how many of them came while the gate's own lock was held.
    #[derive(Default)]
    struct Probe {
        gate: OnceLock<Weak<ShaderMemoryGate>>,
        calls: AtomicUsize,
        under_lock: AtomicUsize,
        logged: AtomicBool,
    }
    impl Probe {
        fn check(&self) {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(g) = self.gate.get().and_then(Weak::upgrade)
                && g.st.try_lock().is_err()
            {
                self.under_lock.fetch_add(1, Ordering::SeqCst);
            }
        }
    }
    struct ProbeMem(Arc<Probe>);
    impl MemSource for ProbeMem {
        fn mem(&self) -> MemInfo {
            self.0.check();
            MemInfo {
                available: 100 * MIB, // below LOW: a second job waits
                ..Default::default()
            }
        }
    }
    struct ProbeLog(Arc<Probe>);
    impl Log for ProbeLog {
        fn line(&self, _s: &str) {
            self.0.check();
            self.0.logged.store(true, Ordering::SeqCst);
        }
    }
    /// a quarter second of the gate is a millisecond here
    struct FastClock;
    impl Clock for FastClock {
        fn now(&self) -> f64 {
            0.0
        }
        fn sleep(&self, _secs: f64) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// Review r4 §6.3: the gate asks the memory source for what it left alone decides on (`available`), not for every number (`mem`: two `task_info`, a `getrusage`, a `statvfs`).
    #[test]
    fn the_gate_asks_only_for_the_memory_left() {
        struct OnlyAvailable(AtomicUsize);
        impl MemSource for OnlyAvailable {
            fn mem(&self) -> MemInfo {
                panic!("the gate read every number")
            }
            fn available(&self) -> u64 {
                self.0.fetch_add(1, Ordering::SeqCst);
                100 * MIB
            }
        }
        let mem = Arc::new(OnlyAvailable(AtomicUsize::new(0)));
        let gate = ShaderMemoryGate::new(
            mem.clone(),
            Arc::new(crate::log::NullLog),
            None,
            Arc::new(FastClock),
        );
        assert_eq!(gate.run(|| 7), 7);
        assert!(mem.0.load(Ordering::SeqCst) >= 1);
        // and the default of a source that has only `mem` is its `available`
        let fake = crate::memory::FakeMem::default();
        fake.available.store(123, Ordering::SeqCst);
        assert_eq!(MemSource::available(&fake), 123);
        #[cfg(target_vendor = "apple")]
        {
            let sys = crate::sys::mem::SystemMem::new(".");
            assert_eq!(
                sys.available(),
                sys.mem().available,
                "the same number as mem() gives"
            );
        }
    }

    /// Review r3 §5.4: the memory source (a syscall in the app) and the app's log are called with the gate's lock released, so the other workers are not held behind them.
    #[test]
    fn the_memory_source_and_the_log_are_called_outside_the_gates_lock() {
        let probe = Arc::new(Probe::default());
        let gate = Arc::new(ShaderMemoryGate::new(
            Arc::new(ProbeMem(probe.clone())),
            Arc::new(ProbeLog(probe.clone())),
            None,
            Arc::new(FastClock),
        ));
        let _ = probe.gate.set(Arc::downgrade(&gate));
        let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        std::thread::scope(|s| {
            let g = &gate;
            s.spawn(move || {
                g.run(|| {
                    started_tx.send(()).unwrap();
                    let _ = release_rx.recv();
                })
            });
            started_rx.recv().unwrap(); // one job runs (and holds no lock while it does)
            let g = &gate;
            let second = s.spawn(move || g.run(|| ()));
            while !probe.logged.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            release_tx.send(()).unwrap();
            second.join().unwrap();
        });
        assert!(probe.calls.load(Ordering::SeqCst) >= 3);
        assert_eq!(
            probe.under_lock.load(Ordering::SeqCst),
            0,
            "calls made under the gate's lock"
        );
        assert_eq!(gate.active(), 0);
    }
}
