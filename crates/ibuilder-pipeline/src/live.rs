//! Live shaders (paches/0008): the queue between the host and the translation thread. Port of the `live_*` half of `gameapp_shim.c`.
//!
//! The host (the engine's NVN layer, C, on the game's submit thread) asks `handler(key, ctl, code)` for a shader its library lacks. The
//! first ask queues it; in `pause` mode the submit thread then waits (up to `DM_LIVE_SHADERS_WAIT_MS`, default 8 s) for the answer, so
//! the picture stops a moment and the object is drawn from its first frame; in `skip` mode it returns at once and the shader appears a
//! moment later. The live thread (`live_next` / `live_done`) takes the batch, translates it (the pipeline's `learn_shaders`) and answers.
//! A "GameAppLiveShader" notification (busy 1, then 0) lets the game view say so: `set_notify`.

use crate::env::Env;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Queued,
    Taken,
    Translated,
    Refused,
}

struct Item {
    key: String,
    stage: String,
    ctl: Vec<u8>,
    code: Vec<u8>,
    state: State,
}

#[derive(Default)]
struct Inner {
    items: Vec<Item>,
    stop: bool,
    queued: usize,
    waiting: usize,
}

/// A shader the host queued: key, control block, code.
pub type Queued = (String, Vec<u8>, Vec<u8>);

type NotifyFn = Arc<dyn Fn(bool) + Send + Sync>;
type ReadyFn = Arc<dyn Fn() + Send + Sync>;

pub struct LiveQueue {
    env: Arc<dyn Env>,
    inner: Mutex<Inner>,
    work: Condvar,
    answer: Condvar,
    notify: Mutex<Option<NotifyFn>>,
    ready: Mutex<Option<ReadyFn>>,
}

/// `live_mode`: 2 pause, 1 skip, 0 off.
pub fn live_mode(env: &dyn Env) -> u8 {
    match env.get("DM_LIVE_SHADERS") {
        None => 2,
        Some(m) if m.is_empty() || m == "pause" || m == "1" => 2,
        Some(m) if m == "skip" => 1,
        Some(_) => 0,
    }
}

impl LiveQueue {
    pub fn new(env: Arc<dyn Env>) -> Arc<LiveQueue> {
        Arc::new(LiveQueue {
            env,
            inner: Mutex::new(Inner::default()),
            work: Condvar::new(),
            answer: Condvar::new(),
            notify: Mutex::new(None),
            ready: Mutex::new(None),
        })
    }

    /// Called with `true` when the first submit thread starts waiting, `false` when the last one stops.
    pub fn set_notify(&self, f: Arc<dyn Fn(bool) + Send + Sync>) {
        *self.notify.lock().unwrap() = Some(f);
    }

    /// The host's `nvn_live_ready`: called after every answer so the host asks again.
    pub fn set_ready(&self, f: Arc<dyn Fn() + Send + Sync>) {
        *self.ready.lock().unwrap() = Some(f);
    }

    /// Forget everything and accept shaders again (a second boot in the same process).
    pub fn reset(&self) {
        *self.inner.lock().unwrap() = Inner::default();
    }

    fn do_notify(&self, busy: bool) {
        let f = self.notify.lock().unwrap().clone();
        if let Some(f) = f {
            f(busy);
        }
    }

    /// The host's callback (`live_handler`). Returns `(r, stage)`: -1 refused or off, 0 not translated yet (ask again), 1 translated by this
    /// ask, 2 translated by an earlier one; `stage` is set for 1 and 2.
    pub fn handler(&self, key: &str, ctl: &[u8], code: &[u8]) -> (i32, Option<String>) {
        if key.len() != 16 {
            return (-1, None);
        }
        let mut g = self.inner.lock().unwrap();
        let mut idx = g.items.iter().position(|it| it.key == key);
        let mut first = false;
        let mut mode = 0u8;
        if idx.is_none() {
            mode = live_mode(&*self.env); // read once per new shader, not on every ask
            if mode == 0 || g.stop {
                return (-1, None);
            }
            g.items.insert(
                0,
                Item {
                    key: key.to_string(),
                    stage: String::new(),
                    ctl: ctl.to_vec(),
                    code: code.to_vec(),
                    state: State::Queued,
                },
            );
            g.queued += 1;
            self.work.notify_all();
            first = true;
            idx = Some(0);
        }
        let key = key.to_string();
        if first
            && mode == 2
            && g.items[0].state != State::Translated
            && g.items[0].state != State::Refused
        {
            let ms = self
                .env
                .get("DM_LIVE_SHADERS_WAIT_MS")
                .filter(|s| !s.is_empty())
                .and_then(|s| s.trim().parse::<u64>().ok())
                .unwrap_or(8000);
            let until = Instant::now() + Duration::from_millis(ms);
            let was_idle = g.waiting == 0;
            g.waiting += 1;
            if was_idle {
                drop(g);
                self.do_notify(true);
                g = self.inner.lock().unwrap();
            }
            loop {
                let done = g
                    .items
                    .iter()
                    .find(|it| it.key == key)
                    .map(|it| matches!(it.state, State::Translated | State::Refused))
                    .unwrap_or(true);
                if done || g.stop {
                    break;
                }
                let now = Instant::now();
                if now >= until {
                    break;
                }
                let (ng, res) = self.answer.wait_timeout(g, until - now).unwrap();
                g = ng;
                if res.timed_out() {
                    break;
                }
            }
            g.waiting -= 1;
            if g.waiting == 0 {
                drop(g);
                self.do_notify(false);
                g = self.inner.lock().unwrap();
            }
        }
        let it = g.items.iter().find(|it| it.key == key);
        let _ = idx;
        match it {
            Some(it) if it.state == State::Translated => {
                (if first { 1 } else { 2 }, Some(it.stage.clone()))
            }
            Some(it) if it.state == State::Refused => (-1, None),
            _ => (0, None),
        }
    }

    /// `live_next(timeout_ms)`: every shader queued since the last call (marked taken); an empty vector when none came within the timeout;
    /// `None` once `stop` was called and nothing is left.
    pub fn next(&self, timeout_ms: u64) -> Option<Vec<Queued>> {
        let mut g = self.inner.lock().unwrap();
        let until = Instant::now() + Duration::from_millis(timeout_ms);
        while !g.stop && g.queued == 0 {
            let now = Instant::now();
            if now >= until {
                break;
            }
            let (ng, res) = self.work.wait_timeout(g, until - now).unwrap();
            g = ng;
            if res.timed_out() {
                break;
            }
        }
        let mut got = Vec::new();
        let mut taken = 0;
        for it in g.items.iter_mut() {
            if got.len() >= 64 {
                break;
            }
            if it.state == State::Queued {
                it.state = State::Taken;
                taken += 1;
                got.push((it.key.clone(), it.ctl.clone(), it.code.clone()));
            }
        }
        g.queued -= taken;
        if g.stop && got.is_empty() {
            return None;
        }
        Some(got)
    }

    /// `live_done(key, stage)`: the translation of a taken shader is laid out (`Some(stage)`) or refused (`None`); the host asks again.
    pub fn done(&self, key: &str, stage: Option<&str>) {
        {
            let mut g = self.inner.lock().unwrap();
            for it in g.items.iter_mut() {
                if it.key == key && matches!(it.state, State::Queued | State::Taken) {
                    match stage {
                        Some(s) if !s.is_empty() => {
                            it.state = State::Translated;
                            it.stage = s.chars().take(23).collect();
                        }
                        _ => it.state = State::Refused,
                    }
                    it.ctl = Vec::new();
                    it.code = Vec::new();
                }
            }
            self.answer.notify_all();
        }
        let f = self.ready.lock().unwrap().clone();
        if let Some(f) = f {
            f();
        }
    }

    /// `live_stop()`: the game is over; the live thread's `next` returns `None`, a waiting submit thread is let go.
    pub fn stop(&self) {
        let mut g = self.inner.lock().unwrap();
        g.stop = true;
        self.work.notify_all();
        self.answer.notify_all();
    }

    pub fn is_stopped(&self) -> bool {
        self.inner.lock().unwrap().stop
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::MapEnv;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const K: &str = "0123456789abcdef";

    #[test]
    fn mode_is_read_from_dm_live_shaders() {
        let e = MapEnv::new();
        assert_eq!(live_mode(&e), 2);
        for (v, m) in [
            ("pause", 2),
            ("1", 2),
            ("", 2),
            ("skip", 1),
            ("off", 0),
            ("0", 0),
        ] {
            e.set("DM_LIVE_SHADERS", v);
            assert_eq!(live_mode(&e), m, "DM_LIVE_SHADERS={v}");
        }
    }

    #[test]
    fn off_and_bad_keys_are_refused() {
        let env = Arc::new(MapEnv::with(&[("DM_LIVE_SHADERS", "off")]));
        let q = LiveQueue::new(env);
        assert_eq!(q.handler(K, b"c", b"d").0, -1);
        assert_eq!(
            q.handler("short", b"c", b"d").0,
            -1,
            "a key is 16 characters"
        );
    }

    #[test]
    fn skip_mode_queues_and_returns_at_once_then_answers_later() {
        let env = Arc::new(MapEnv::with(&[("DM_LIVE_SHADERS", "skip")]));
        let q = LiveQueue::new(env);
        assert_eq!(
            q.handler(K, b"ctl", b"code").0,
            0,
            "queued, not translated yet"
        );
        assert_eq!(
            q.handler(K, b"ctl", b"code").0,
            0,
            "asked again: still pending, queued once"
        );
        let batch = q.next(1000).unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0], (K.to_string(), b"ctl".to_vec(), b"code".to_vec()));
        assert!(
            q.next(10).unwrap().is_empty(),
            "nothing new: an empty batch after the timeout"
        );
        q.done(K, Some("vs"));
        assert_eq!(
            q.handler(K, b"ctl", b"code"),
            (2, Some("vs".into())),
            "translated by an earlier ask"
        );
    }

    #[test]
    fn pause_mode_blocks_the_submit_thread_until_the_answer() {
        let env = Arc::new(MapEnv::with(&[
            ("DM_LIVE_SHADERS", "pause"),
            ("DM_LIVE_SHADERS_WAIT_MS", "5000"),
        ]));
        let q = LiveQueue::new(env);
        let busy = Arc::new(AtomicUsize::new(0));
        let b2 = busy.clone();
        q.set_notify(Arc::new(move |b| {
            b2.fetch_add(if b { 1 } else { 100 }, Ordering::SeqCst);
        }));
        let ready = Arc::new(AtomicUsize::new(0));
        let r2 = ready.clone();
        q.set_ready(Arc::new(move || {
            r2.fetch_add(1, Ordering::SeqCst);
        }));
        let q2 = q.clone();
        let t = std::thread::spawn(move || q2.handler(K, b"c", b"d"));
        let batch = q.next(2000).unwrap();
        assert_eq!(batch.len(), 1);
        q.done(K, Some("ps"));
        assert_eq!(
            t.join().unwrap(),
            (1, Some("ps".into())),
            "the first ask that waited gets 1"
        );
        assert_eq!(busy.load(Ordering::SeqCst), 101, "busy on, then off, once");
        assert_eq!(
            ready.load(Ordering::SeqCst),
            1,
            "the host is told to ask again"
        );
    }

    #[test]
    fn a_refused_shader_answers_minus_one_and_stop_releases_everyone() {
        let env = Arc::new(MapEnv::with(&[("DM_LIVE_SHADERS", "skip")]));
        let q = LiveQueue::new(env);
        q.handler(K, b"c", b"d");
        q.next(100).unwrap();
        q.done(K, None);
        assert_eq!(q.handler(K, b"c", b"d").0, -1);
        q.stop();
        assert!(q.next(10).is_none(), "stopped and nothing left");
        assert_eq!(
            q.handler("fedcba9876543210", b"c", b"d").0,
            -1,
            "no new shader after stop"
        );
        q.reset();
        assert!(!q.is_stopped());
    }

    #[test]
    fn pause_wait_times_out_without_an_answer() {
        let env = Arc::new(MapEnv::with(&[
            ("DM_LIVE_SHADERS", "pause"),
            ("DM_LIVE_SHADERS_WAIT_MS", "50"),
        ]));
        let q = LiveQueue::new(env);
        let t0 = Instant::now();
        assert_eq!(q.handler(K, b"c", b"d").0, 0);
        assert!(t0.elapsed() >= Duration::from_millis(40));
    }
}
