//! The stage model: stage numbers and the `gameapp.stage` line the app's UI reads every second (GameAppUi.ParseStage), plus the `Stage` trait
//! the chain driver runs (workers are plugged in as adapters, see `adapters`).
//!
//! `gameapp.stage` is a one-line "where it is right now", overwritten in place on every step, so a user watching a screen that does
//! nothing for three minutes still sees the current step and can tell a frozen run from a slow one. The line format is a contract:
//! `stage k/n: text`, with `(i/n: module)` for the rewrite and `(done/total shaders)` for stage 3.

use crate::ctx::Ctx;
use crate::error::Result;
use crate::layout::Docs;
use crate::log::Log;

/// The chain's stages, with the `k/5` numbering the pipeline prints (the NUP read and the profile are both "1", assembly and signing are both "4").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum StageId {
    Nup,
    Profile,
    Rewrite,
    Shaders,
    Assemble,
    Sign,
    Boot,
}

impl StageId {
    /// `k` of `stage k/5`
    pub fn number(self) -> u32 {
        match self {
            StageId::Nup | StageId::Profile => 1,
            StageId::Rewrite => 2,
            StageId::Shaders => 3,
            StageId::Assemble | StageId::Sign => 4,
            StageId::Boot => 5,
        }
    }
    /// The key of the status map and the name the timings use (`nup`, `profile`, `rewrite`, `shaders`, `assemble`).
    pub fn key(self) -> &'static str {
        match self {
            StageId::Nup => "nup",
            StageId::Profile => "profile",
            StageId::Rewrite => "rewrite",
            StageId::Shaders => "shaders",
            StageId::Assemble | StageId::Sign => "assemble",
            StageId::Boot => "boot",
        }
    }
}

type StageHook = std::sync::Arc<dyn Fn(u32, u32, &str) + Send + Sync>;
static STAGE_HOOK: std::sync::Mutex<Option<StageHook>> = std::sync::Mutex::new(None);

/// A host that wants every stage line as it is written (the C ABI's progress callback) registers it here. One chain runs per process, so the hook is process-wide.
pub fn set_stage_hook(h: Option<StageHook>) {
    *STAGE_HOOK.lock().unwrap() = h;
}

/// `stage(k, n, text, log)`: format the line, replace `Documents/gameapp.stage` with it (through a `.tmp`: the UI polls this file while the chain runs and must never read an empty or half line;
/// review r2 N7) and log it when `log` is given. A failure to write is not an error of the chain (as in Python): it is said in the log when there is one. Returns the line.
pub fn write_stage(docs: &Docs, k: u32, n: u32, text: &str, log: Option<&dyn Log>) -> String {
    let line = format!("stage {k}/{n}: {text}");
    if let Err(e) = crate::util::write_atomic(&docs.stage_file(), format!("{line}\n").as_bytes())
        && let Some(l) = log
    {
        l.line(&format!(
            "stage: could not write {}: {e}",
            docs.stage_file().display()
        ));
    }
    crate::diag::stage(k, n, text);
    let hook = STAGE_HOOK.lock().unwrap().clone();
    if let Some(h) = hook {
        h(k, n, text);
    }
    if let Some(log) = log {
        log.line(&line);
    }
    line
}

/// `write_stage` for a line that changes many times a second (stage 3's shader count, one line every half second from the translation): `gameapp.stage` is rewritten (a whole temporary
/// file and a rename) at most once every `every`, which is what the app's screen reads it at (once a second); a line held back is written by `flush`, and a `force`d one at once, so the
/// LAST line offered always lands (review r4 §6.3: ~280 rewrites in a 141 s translation).
pub struct StageThrottle {
    every: std::time::Duration,
    last: Option<std::time::Instant>,
    held: Option<(u32, u32, String)>,
}

impl StageThrottle {
    pub fn new(every: std::time::Duration) -> StageThrottle {
        StageThrottle {
            every,
            last: None,
            held: None,
        }
    }

    /// `write_stage(docs, k, n, text, None)` now when nothing was written for `every` (or ever) or when `force`; otherwise kept for `flush` (a later line replaces it). True when written.
    pub fn offer(
        &mut self,
        docs: &Docs,
        k: u32,
        n: u32,
        text: String,
        force: bool,
        now: std::time::Instant,
    ) -> bool {
        let due = force
            || self
                .last
                .is_none_or(|l| now.saturating_duration_since(l) >= self.every);
        if !due {
            self.held = Some((k, n, text));
            return false;
        }
        self.held = None;
        self.last = Some(now);
        write_stage(docs, k, n, &text, None);
        true
    }

    /// The line held back, if any, written now. True when there was one.
    pub fn flush(&mut self, docs: &Docs) -> bool {
        match self.held.take() {
            Some((k, n, text)) => {
                write_stage(docs, k, n, &text, None);
                true
            }
            None => false,
        }
    }
}

/// What a stage reports back to the driver.
#[derive(Debug, PartialEq, Eq)]
pub enum StageOutcome {
    /// ran to the end
    Done,
    /// ran, and the status is not a plain "ok" (stage 3: `none (...)` when the game has no library)
    Status(String),
    /// the stage asks for the process to end and the build to continue in the next launch (result 75)
    Restart,
}

/// One step of the chain. Workers live behind `ctx.adapters`; the stage is the orchestration around them (cache, markers, memory lines).
pub trait Stage {
    fn id(&self) -> StageId;
    fn run(&self, ctx: &mut Ctx) -> Result<StageOutcome>;
}

// ---- reading the line back (GameAppUi.ParseStage and friends) ---------------------------------------------------------

/// One `gameapp.stage` line, read. `fraction` is where the bar stands when that step STARTS, 0..1.
#[derive(Clone, Debug, PartialEq)]
pub struct StageProgress {
    pub valid: bool,
    pub step: u32,
    pub of: u32,
    pub text: String,
    pub fraction: f64,
}

// Where each of the five stages starts on the bar, from the device timings in docs/STATE-GAMEAPP.md: profile 3.7 s, rewrite ~140 s, shaders
// (the longest), assemble+sign ~2 s, boot until the guest ends. The weights only shape the bar.
const STAGE_WEIGHTS: [f64; 5] = [0.03, 0.45, 0.40, 0.06, 0.06];
const MODULE_ORDER: [&str; 3] = ["main", "subsdk0", "sdk"];
const MODULE_START: [f64; 3] = [0.00, 0.88, 0.92];

/// `GameAppUi.ParseStage`: anything that is not `stage k/n: text` is not valid and stands at 0.
pub fn parse_stage(line: &str) -> StageProgress {
    let none = StageProgress {
        valid: false,
        step: 0,
        of: 0,
        text: line.to_string(),
        fraction: 0.0,
    };
    let t = line.trim();
    let Some(rest) = t.strip_prefix("stage") else {
        return none;
    };
    if !rest.starts_with(char::is_whitespace) {
        return none;
    }
    let rest = rest.trim_start();
    let Some((counts, text)) = rest.split_once(':') else {
        return none;
    };
    let Some((a, b)) = counts.split_once('/') else {
        return none;
    };
    if a.is_empty()
        || b.is_empty()
        || !a.bytes().all(|c| c.is_ascii_digit())
        || !b.bytes().all(|c| c.is_ascii_digit())
    {
        return none;
    }
    let (Ok(step), Ok(of)) = (a.parse::<u32>(), b.parse::<u32>()) else {
        return none;
    };
    if step < 1 || of < 1 || step > of {
        return none;
    }
    let text = text.trim_start();
    let (start, width) = if of as usize == STAGE_WEIGHTS.len() {
        (
            STAGE_WEIGHTS[..step as usize - 1].iter().sum::<f64>(),
            STAGE_WEIGHTS[step as usize - 1],
        )
    } else {
        ((step - 1) as f64 / of as f64, 1.0 / of as f64)
    };
    let fraction = (start + width * within_stage(text)).clamp(0.0, 1.0);
    StageProgress {
        valid: true,
        step,
        of,
        text: text.to_string(),
        fraction,
    }
}

/// `(done/total shaders)` in a stage text.
pub fn shader_counter(text: &str) -> Option<(u64, u64)> {
    let mut from = 0;
    while let Some(i) = text[from..].find('(') {
        let s = from + i + 1;
        if let Some(j) = text[s..].find(')') {
            let inner = &text[s..s + j];
            if let Some(body) = inner.strip_suffix(" shaders")
                && let Some((a, b)) = body.split_once('/')
                && let (Ok(a), Ok(b)) = (a.parse::<u64>(), b.parse::<u64>())
            {
                return Some((a, b));
            }
        }
        from = s;
    }
    None
}

/// `(i/n: module)` in a stage text.
pub fn module_counter(text: &str) -> Option<(u32, u32, String)> {
    let mut from = 0;
    while let Some(i) = text[from..].find('(') {
        let s = from + i + 1;
        if let Some(j) = text[s..].find(')') {
            let inner = &text[s..s + j];
            if let Some((counts, name)) = inner.split_once(':')
                && let Some((a, b)) = counts.split_once('/')
            {
                let name = name.trim_start();
                if let (Ok(a), Ok(b)) = (a.parse::<u32>(), b.parse::<u32>())
                    && !name.is_empty()
                    && name
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
                {
                    return Some((a, b, name.to_string()));
                }
            }
        }
        from = s;
    }
    None
}

fn within_stage(text: &str) -> f64 {
    if let Some((done, total)) = shader_counter(text)
        && total > 0
    {
        return (done as f64 / total as f64).min(1.0);
    }
    let Some((i, n, name)) = module_counter(text) else {
        return 0.0;
    };
    if let Some(idx) = MODULE_ORDER.iter().position(|m| *m == name) {
        return MODULE_START[idx];
    }
    if n >= 1 && i >= 1 && i <= n {
        return (i - 1) as f64 / n as f64;
    }
    0.0
}

/// `GameAppUi.IsBootStage`: the LAST stage of five, saying it boots the guest -- the moment the app puts the game view on screen.
pub fn is_boot_stage(line: &str) -> bool {
    let s = parse_stage(line);
    s.valid && s.of == 5 && s.step == 5 && s.text.contains("booting the guest")
}

/// `GameAppResume.StageNumber`: the stage number of a pipeline stage line ("stage 3/5: ..."), 0 when there is none.
pub fn stage_number(line: &str) -> u32 {
    let b = line.as_bytes();
    if line.starts_with("stage ")
        && b.len() > 7
        && b[6].is_ascii_digit()
        && b[7] == b'/'
        && b.get(8).map(|c| c.is_ascii_digit()).unwrap_or(false)
    {
        return (b[6] - b'0') as u32;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Review r4 §6.3: the shader count is written at most once a period; a line held back is replaced by the next one, a forced one goes out at once, and `flush` writes what was held,
    /// so the last line offered always lands.
    #[test]
    fn a_throttled_stage_line_is_written_once_a_period_and_the_last_one_always_lands() {
        let d = crate::testutil::TempDir::new("stage-throttle");
        let docs = Docs::new(d.path());
        let read = || std::fs::read_to_string(docs.stage_file()).unwrap();
        let t0 = std::time::Instant::now();
        let s = std::time::Duration::from_millis;
        let mut th = StageThrottle::new(s(1000));
        let line = |n: u32| format!("shaders: translating for this iPhone ({n}/100 shaders)");
        assert!(
            th.offer(&docs, 3, 5, line(1), false, t0),
            "the first line goes out"
        );
        assert!(!th.offer(&docs, 3, 5, line(2), false, t0 + s(400)));
        assert!(!th.offer(&docs, 3, 5, line(3), false, t0 + s(900)));
        assert_eq!(read(), format!("stage 3/5: {}\n", line(1)), "held back");
        assert!(
            th.offer(&docs, 3, 5, line(4), false, t0 + s(1000)),
            "a period later"
        );
        assert!(!th.offer(&docs, 3, 5, line(5), false, t0 + s(1200)));
        assert!(th.flush(&docs), "the held line");
        assert_eq!(read(), format!("stage 3/5: {}\n", line(5)));
        assert!(!th.flush(&docs), "nothing held any more");
        assert!(
            th.offer(&docs, 3, 5, line(100), true, t0 + s(1300)),
            "a forced line goes out at once"
        );
        assert_eq!(read(), format!("stage 3/5: {}\n", line(100)));
        assert!(!th.flush(&docs));
    }

    fn f(l: &str) -> f64 {
        parse_stage(l).fraction
    }

    #[test]
    fn parse_matches_the_apps_reader() {
        let p = parse_stage("stage 2/5: rewriting the modules (1/3: main)");
        assert!(p.valid && p.step == 2 && p.of == 5);
        assert!(
            (p.fraction - 0.03).abs() < 1e-9,
            "stage 2 of 5 starts after profile's 0.03: {}",
            p.fraction
        );
        let p = parse_stage("stage 2/5: rewriting the modules (2/3: subsdk0)");
        assert!((p.fraction - (0.03 + 0.45 * 0.88)).abs() < 1e-9);
        let p = parse_stage("stage 3/5: shaders: translating for this iPhone (500/1000 shaders)");
        assert!((p.fraction - (0.48 + 0.40 * 0.5)).abs() < 1e-9);
        assert!(
            !parse_stage("stage 6/5: nope").valid,
            "a step past the end is not valid"
        );
        assert!(!parse_stage("stage 0/5: nope").valid);
        assert!(
            !parse_stage("stage 1/5 profile: derived in 5.2s -> /x").valid,
            "a log line is not a stage line"
        );
        assert!(!parse_stage("").valid && f("") == 0.0);
        assert!(
            parse_stage("  stage 4/5: x \n").valid,
            "the file ends with a newline"
        );
        // an unknown module name falls back to its position
        let p = parse_stage("stage 2/5: rewriting the modules (2/4: subsdk1)");
        assert!((p.fraction - (0.03 + 0.45 * 0.25)).abs() < 1e-9);
        // another stage count: equal parts
        let p = parse_stage("stage 2/4: x");
        assert!((p.fraction - 0.25).abs() < 1e-9);
    }

    #[test]
    fn boot_stage_is_only_the_fifth_of_five_that_boots() {
        let boot = "stage 5/5: booting the guest (dlopen + dm_host_probe_boot -> nnMain; quit from the gear menu)";
        assert!(is_boot_stage(boot));
        assert!(is_boot_stage(&format!("{boot}\n")));
        assert!(!is_boot_stage(
            "stage 4/5: signing the framework in-process (zsign, no codesign)"
        ));
        assert!(!is_boot_stage(
            "stage 2/5: rewriting the modules (1/3: main) while booting the guest"
        ));
        assert!(!is_boot_stage("stage 5/6: booting the guest"));
        assert!(!is_boot_stage("stage 5/5: cleaning up"));
        assert!(!is_boot_stage(
            "stage 5 boot returned 0, the guest returned 0"
        ));
        assert!(!is_boot_stage(""));
    }

    #[test]
    fn stage_numbers() {
        assert_eq!(stage_number("stage 3/5: shaders"), 3);
        assert_eq!(stage_number("nothing"), 0);
        assert_eq!(stage_number(""), 0);
    }

    #[test]
    fn write_stage_overwrites_the_one_line_file() {
        let d = crate::testutil::TempDir::new("stage");
        let docs = Docs::new(d.path());
        let log = crate::log::MemLog::new();
        let l = write_stage(
            &docs,
            2,
            5,
            "rewriting the modules (1/3: main)",
            Some(&*log),
        );
        assert_eq!(l, "stage 2/5: rewriting the modules (1/3: main)");
        assert_eq!(
            fs::read_to_string(docs.stage_file()).unwrap(),
            format!("{l}\n")
        );
        write_stage(&docs, 3, 5, "shaders", None);
        assert_eq!(
            fs::read_to_string(docs.stage_file()).unwrap(),
            "stage 3/5: shaders\n"
        );
        assert_eq!(
            log.lines(),
            vec![l],
            "a call with no log writes the file only"
        );
    }
}
