//! What a hot iPhone asks of the build (DECISIONS trap 76): `Documents/gameapp.thermal` ("<state> <lowperf>", written every few seconds
//! by the app's UI during a build: ProcessInfo's thermal state 0 nominal, 1 fair, 2 serious, 3 critical, and the user's "Low performance"
//! mode), read at most every `poll` seconds by the one `ThermalGate` of a build.

use crate::log::Log;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Time and sleep, replaceable so tests can run the gates on a fake clock.
pub trait Clock: Send + Sync {
    /// monotonic seconds
    fn now(&self) -> f64;
    fn sleep(&self, secs: f64);
}

pub struct RealClock {
    start: Instant,
}

impl RealClock {
    pub fn new() -> Arc<RealClock> {
        Arc::new(RealClock {
            start: Instant::now(),
        })
    }
}

impl Clock for RealClock {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }
    fn sleep(&self, secs: f64) {
        std::thread::sleep(Duration::from_secs_f64(secs.max(0.0)));
    }
}

/// `read_thermal`: `(state, lowperf)`. A missing or unreadable file reads as "0 0": nothing is held back. The state is clamped to 0..=3.
/// A garbled second field makes the whole read "0 0", as in Python (`int()` raising).
pub fn read_thermal(path: &Path) -> (u8, bool) {
    let Ok(text) = fs::read_to_string(path) else {
        return (0, false);
    };
    let parts: Vec<&str> = text.split_whitespace().collect();
    let Some(first) = parts.first() else {
        return (0, false);
    };
    let Ok(state) = first.parse::<i64>() else {
        return (0, false);
    };
    let low = match parts.get(1) {
        None => false,
        Some(p) => match p.parse::<i64>() {
            Ok(v) => v != 0,
            Err(_) => return (0, false),
        },
    };
    (state.clamp(0, 3) as u8, low)
}

struct State {
    at: Option<f64>,
    value: (u8, bool),
    since: Option<f64>,
    pauses: u32,
    paused_s: f64,
}

/// The gate: Low performance mode caps parallel jobs at 2 (`parallel_cap`); thermal state serious or worse starts no new job until it is
/// back to fair or nominal (`wait`: one "cooling down" line per pause and one saying how long it lasted); the rewrite waits between
/// modules only at critical (`wait(3, ..)`). A job already running is never stopped.
pub struct ThermalGate {
    path: PathBuf,
    clock: Arc<dyn Clock>,
    log: Mutex<Arc<dyn Log>>,
    poll: Mutex<f64>,
    st: Mutex<State>,
    reads: AtomicUsize,
}

impl ThermalGate {
    pub const POLL: f64 = 3.0;

    pub fn new(path: &Path, log: Arc<dyn Log>, clock: Arc<dyn Clock>) -> ThermalGate {
        ThermalGate {
            path: path.to_path_buf(),
            clock,
            log: Mutex::new(log),
            poll: Mutex::new(Self::POLL),
            st: Mutex::new(State {
                at: None,
                value: (0, false),
                since: None,
                pauses: 0,
                paused_s: 0.0,
            }),
            reads: AtomicUsize::new(0),
        }
    }

    pub fn set_log(&self, log: Arc<dyn Log>) {
        *self.log.lock().unwrap() = log;
    }

    /// Override the poll interval (tests).
    pub fn set_poll(&self, secs: f64) {
        *self.poll.lock().unwrap() = secs;
    }

    /// How many times the file was actually read.
    pub fn file_reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }

    pub fn pauses(&self) -> u32 {
        self.st.lock().unwrap().pauses
    }

    /// Seconds spent paused for heat (the timings row's `cooldown_s`).
    pub fn paused_s(&self) -> f64 {
        self.st.lock().unwrap().paused_s
    }

    /// Forget the cached read (the next `read` hits the file).
    pub fn invalidate(&self) {
        self.st.lock().unwrap().at = None;
    }

    /// `(state, lowperf)`, read at most every poll interval (a read per job would be a read per shader).
    pub fn read(&self) -> (u8, bool) {
        let poll = *self.poll.lock().unwrap();
        let mut st = self.st.lock().unwrap();
        let now = self.clock.now();
        if st.at.is_none() || now - st.at.unwrap() >= poll {
            st.at = Some(now);
            st.value = read_thermal(&self.path);
            self.reads.fetch_add(1, Ordering::SeqCst);
        }
        st.value
    }

    /// 2 in Low performance mode, else `None` (no cap beyond the build's own thread count).
    pub fn parallel_cap(&self) -> Option<usize> {
        if self.read().1 { Some(2) } else { None }
    }

    /// Blocks while the thermal state is `level` or worse; returns the seconds it waited.
    pub fn wait(&self, level: u8, what: &str) -> f64 {
        if self.read().0 < level {
            return 0.0;
        }
        let t0 = self.clock.now();
        // The lines are said AFTER the gate's locks are released: the log is the build's `Log`, under FFI the app's own callback, which may take long or call back into the gate.
        let cooling = {
            let mut st = self.st.lock().unwrap();
            if st.since.is_none() {
                st.since = Some(t0);
                st.pauses += 1;
                Some(st.value.0)
            } else {
                None
            }
        };
        if let Some(state) = cooling {
            self.say(&format!(
                "{what}: cooling down: the iPhone is {} (thermal state {state}): no new work starts until it is back to fair or nominal",
                if state >= 3 { "critical" } else { "serious" }
            ));
        }
        let poll = *self.poll.lock().unwrap();
        // a pause or cancel (`stop`) ends the wait: a person who stops a build does not wait for the phone to cool down first
        let mut stopped = false;
        while self.read().0 >= level {
            if crate::stop::requested() != crate::stop::NONE {
                stopped = true;
                break;
            }
            self.clock.sleep(poll);
        }
        let waited = self.clock.now() - t0;
        let cooled = {
            let mut st = self.st.lock().unwrap();
            match st.since {
                Some(since) => {
                    let total = self.clock.now() - since;
                    st.since = None;
                    st.paused_s += total;
                    Some((total, st.value.0))
                }
                None => None,
            }
        };
        if let Some((total, state)) = cooled {
            if stopped {
                self.say(&format!("{what}: the wait for the iPhone to cool down was ended by a stop request after {total:.0} s (thermal state {state})"));
            } else {
                self.say(&format!(
                    "{what}: cooled down after {total:.0} s (thermal state {state})"
                ));
            }
        }
        waited
    }

    /// One line to the current log, with no lock held while it runs (the log is cloned out of its slot first).
    fn say(&self, text: &str) {
        let log = self.log.lock().unwrap().clone();
        log.line(text);
    }
}
