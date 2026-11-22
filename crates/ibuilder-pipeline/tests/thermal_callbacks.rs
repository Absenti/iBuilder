//! The thermal gate logs "cooling down" / "cooled down" through the build's `Log`, which under FFI is the app's own C callback (review finding 4.2). The line must be said with no lock of the gate
//! held: a callback that touches the gate again (reads its counters, swaps its log), or that simply takes long, would otherwise deadlock or stall every thread that asks the gate for the state.

use ibuilder_pipeline::log::{Log, NullLog};
use ibuilder_pipeline::testutil::{TempDir, write};
use ibuilder_pipeline::thermal::{Clock, ThermalGate};
use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// time moves only when the gate sleeps, and the first sleep cools the phone down
struct CoolsAtFirstSleep {
    t: Mutex<f64>,
    path: PathBuf,
}
impl Clock for CoolsAtFirstSleep {
    fn now(&self) -> f64 {
        *self.t.lock().unwrap()
    }
    fn sleep(&self, secs: f64) {
        *self.t.lock().unwrap() += secs;
        write(&self.path, "0 0\n");
    }
}

/// a log whose callback asks the gate for things that need its locks, as an app's callback might
struct ReentrantLog {
    gate: Mutex<Option<Arc<ThermalGate>>>,
    lines: Mutex<Vec<String>>,
}
impl Log for ReentrantLog {
    fn line(&self, s: &str) {
        self.lines.lock().unwrap().push(s.to_string());
        let gate = self
            .gate
            .lock()
            .unwrap()
            .clone()
            .expect("the gate is set before the wait");
        let _ = (gate.pauses(), gate.paused_s(), gate.read()); // the gate's state lock
        gate.set_log(Arc::new(NullLog)); // the gate's log slot
    }
}

#[test]
fn the_cooling_lines_are_said_with_no_lock_of_the_gate_held() {
    let d = TempDir::new("thermal-callbacks");
    let path = d.join("gameapp.thermal");
    write(&path, "2 0\n");
    let log = Arc::new(ReentrantLog {
        gate: Mutex::new(None),
        lines: Mutex::new(Vec::new()),
    });
    let clock = Arc::new(CoolsAtFirstSleep {
        t: Mutex::new(1000.0),
        path: path.clone(),
    });
    let gate = Arc::new(ThermalGate::new(&path, log.clone(), clock));
    *log.gate.lock().unwrap() = Some(gate.clone());

    let (tx, rx) = channel();
    let g = gate.clone();
    std::thread::spawn(move || {
        let waited = g.wait(2, "stage 3");
        let _ = tx.send(waited);
    });
    let waited = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("wait() did not return: the log callback ran under a lock of the gate, and its call back into the gate deadlocked");
    assert!(waited >= ThermalGate::POLL, "{waited}");
    let lines = log.lines.lock().unwrap().clone();
    // the first line was said by the log the gate was built with; it swapped the log for a NullLog, so the "cooled down" line goes nowhere (a swapped log is honoured at once)
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("cooling down"), "{lines:?}");
    assert_eq!(gate.pauses(), 1);
    assert!(gate.paused_s() >= ThermalGate::POLL);
}
