//! The run/resume/boot-only state machine. In the Python/C# app it was spread over `gameapp_pipeline.py` (the request files), `GameAppMain.cs` (the launch path,
//! `BeginRun`/`Finish`, `AutoRunFromArgs`, `ReportInterruptedRunIfAny`), `GameAppResume.cs` (`RecordKilledBuild`, `StartContinuedBuild`, `RestartForStage3`) and
//! `GameAppUi.cs` (`ClassifyBootRequest`, `ClassifyFramework`, `ClassifyPreviousRun`, `LastRunLine`). It is one pure-ish module here: every decision is a function of the files
//! in `Documents` and a few facts the UI passes in, and returns what to do plus the lines to put in the screen's log.
//!
//! The files (docs/DECISIONS.md traps 18-21, 28, 74, 75):
//!
//! | file | written by | meaning |
//! |---|---|---|
//! | `gameapp.running` | `begin_run` | a run began and has not finished; present at launch = the previous run was interrupted |
//! | `gameapp.args` | the picker / a harness | a ONE-SHOT request (renamed to `.done` the moment it is honoured, wherever the run was started from) |
//! | `gameapp.boot-only` | the chain / `begin_run` | "boot the framework that is already built": ARMED (a launch starts it by itself) or ATTEMPTED (a boot began from it and never came back: only a tap boots again) |
//! | `gameapp.stage` | the chain | the one-line current step |
//! | `gameapp.killed` | `report_interrupted_run` | iOS ended a BUILD (memory): the library offers Resume |
//! | `gameapp.continue` / `.run` | the chain / the app | the chain ended on purpose before stage 3 (result 75); the next launch continues it |
//! | `gameapp.framework` | the chain, after signing | which build of the app built the framework on disk, for which game |

use crate::layout::Docs;
use crate::stage::stage_number;
use crate::timings::read_kv;
use crate::util::{is_file, read_text, utc_iso};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub const BOOT_ONLY_ATTEMPTED: &str = "attempted";
/// The pipeline's result when it ends the chain on purpose to be restarted (`RESTART_RC`, EX_TEMPFAIL).
pub const RESTART_RC: i32 = 75;

// ---- classification (GameAppUi) -----------------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootRequestState {
    None,
    Armed,
    Attempted,
}

/// `ClassifyBootRequest`: `exists` is whether the file is there; `content` what it says, `None` when it could not be read. An unreadable request is treated as
/// attempted: the safe reading, which never starts anything by itself.
pub fn classify_boot_request(exists: bool, content: Option<&str>) -> BootRequestState {
    if !exists {
        return BootRequestState::None;
    }
    match content {
        None => BootRequestState::Attempted,
        Some(c) if c.starts_with(BOOT_ONLY_ATTEMPTED) => BootRequestState::Attempted,
        Some(_) => BootRequestState::Armed,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameworkStamp {
    pub app_build: String,
    pub game: String,
    pub content: Option<String>,
    /// `signature=adhoc-export`: stage 4b signed the framework ad hoc (no identity on this install): built for export only, never booted here
    pub export_only: bool,
}

/// `ParseFrameworkStamp`: `key=value` lines (app_build, game, content). `None` when there is no stamp or it names no build.
pub fn parse_framework_stamp(content: Option<&str>) -> Option<FrameworkStamp> {
    let content = content?;
    let (mut build, mut game, mut built, mut export_only) = (None, None, None, false);
    for raw in content.split('\n') {
        let line = raw.trim();
        let Some(eq) = line.find('=').filter(|&i| i > 0) else {
            continue;
        };
        let (key, value) = (line[..eq].trim(), line[eq + 1..].trim().to_string());
        match key {
            "app_build" => build = Some(value),
            "game" => game = Some(value),
            "content" => built = Some(value),
            "signature" => export_only = value == crate::assemble::EXPORT_ONLY_SIGNATURE,
            _ => {}
        }
    }
    match build {
        Some(b) if !b.is_empty() => Some(FrameworkStamp {
            app_build: b,
            game: game.unwrap_or_default(),
            content: built,
            export_only,
        }),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameworkState {
    None,
    Fresh,
    Stale,
}

/// `ClassifyFramework`: `on_disk`: the framework and the extraction it boots from are both there. A framework with no usable stamp is STALE, not fresh: it predates
/// builds being tracked, and the safe reading of "I cannot tell who built this" is "not this build".
pub fn classify_framework(
    on_disk: bool,
    stamp_content: Option<&str>,
    current_build: &str,
) -> FrameworkState {
    if !on_disk {
        return FrameworkState::None;
    }
    match parse_framework_stamp(stamp_content) {
        Some(s) if !current_build.is_empty() && s.app_build == current_build => {
            FrameworkState::Fresh
        }
        _ => FrameworkState::Stale,
    }
}

/// How a run that left `Documents/gameapp.running` behind actually ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviousRun {
    /// its boot ran to the time limit, the shim signalled itself, and the guest's graceful exit finished
    DesignedEnd,
    /// the same clean end, ordered by the player with "Quit game"
    PlayerEnd,
    /// the time limit fired but the guest never logged `graceful exit done`: not a clean end, and not to be hidden
    DesignedEndWithoutCleanExit,
    /// the shim named a fatal signal before re-raising
    FaultInGuest,
    /// nothing says how it ended
    KilledOrUnknown,
}

/// `LastRunLine`: the last line of `gameapp.chain.log` that belongs to the PREVIOUS run. The launch constructor writes its census and reservation lines into the same
/// file at the start of THIS process, so at report time they sit at the end of the file, after the line that says how the previous run ended.
pub fn last_run_line<S: AsRef<str>>(lines: &[S]) -> String {
    for line in lines.iter().rev() {
        let line = line.as_ref();
        if !is_run_line(line) {
            continue;
        }
        return line.to_string();
    }
    "(none)".to_string()
}

/// A line `last_run_line` takes: not blank, and not one of this launch's own census lines.
fn is_run_line(line: &str) -> bool {
    !(line.trim().is_empty()
        || line.starts_with("[gameapp] as-census ")
        || line.starts_with("[gameapp] as-reserve:"))
}

/// `ClassifyPreviousRun`: `stage_line` is `gameapp.stage`, `chain_last` the `last_run_line` of the chain log, `guest_last` the last line of `gameapp.guest.log`.
/// Only a run that reached the boot can have ended at its time limit, so an older run's time-limit line never excuses a run that died earlier.
pub fn classify_previous_run(
    stage_line: Option<&str>,
    chain_last: Option<&str>,
    guest_last: Option<&str>,
) -> PreviousRun {
    let Some(stage) = stage_line else {
        return PreviousRun::KilledOrUnknown;
    };
    if !stage.contains("booting the guest") {
        return PreviousRun::KilledOrUnknown;
    }
    let clean = guest_last
        .map(|g| g.contains("graceful exit done"))
        .unwrap_or(false);
    if let Some(c) = chain_last {
        // The player's Quit: `ibuilder_quit_game` logs this line before it signals the process.
        if c.contains("the player ended the game") {
            return if clean {
                PreviousRun::PlayerEnd
            } else {
                PreviousRun::DesignedEndWithoutCleanExit
            };
        }
        // "boot time limit N s reached" and "the boot ran to its time limit, the designed end" are the shim's two lines; "boot time limit of N s disarmed" is the OTHER
        // case (the boot returned first, no signal was sent).
        if c.contains("time limit") && !c.contains("disarmed") {
            return if clean {
                PreviousRun::DesignedEnd
            } else {
                PreviousRun::DesignedEndWithoutCleanExit
            };
        }
        if c.contains("received SIG") {
            return PreviousRun::FaultInGuest;
        }
    }
    PreviousRun::KilledOrUnknown
}

// ---- small readers --------------------------------------------------------------------------------------------------------------------------

/// `LastLine`: the last non-blank line of a log, "(none)" when the file is missing or empty. Read from the end (`last_line_where`), not whole.
pub fn last_line(path: &Path) -> String {
    crate::util::last_line_where(path, |l| !l.trim().is_empty())
        .ok()
        .flatten()
        .unwrap_or_else(|| "(none)".to_string())
}

/// `LastMatching`: the last line of a log that contains `needle`, "(none)" otherwise. Read from the end (`last_line_where`), not whole.
pub fn last_matching(path: &Path, needle: &str) -> String {
    crate::util::last_line_where(path, |l| l.contains(needle))
        .ok()
        .flatten()
        .unwrap_or_else(|| "(none)".to_string())
}

/// `last_run_line` of a log file, read from the end (`last_line_where`), not whole.
pub fn last_run_line_of(path: &Path) -> String {
    crate::util::last_line_where(path, is_run_line)
        .ok()
        .flatten()
        .unwrap_or_else(|| "(none)".to_string())
}

/// `KeyOf`: the value of `key=` in a small state file's text.
pub fn key_of(text: &str, key: &str) -> String {
    for line in text.split('\n') {
        if let Some(at) = line.find('=').filter(|&i| i > 0)
            && line[..at].trim() == key
        {
            return line[at + 1..].trim().to_string();
        }
    }
    String::new()
}

/// An ISO-8601 time stamp ("2026-11-19T10:00:00.1234567Z", or with a `+hh:mm` offset) as seconds since the epoch.
pub fn parse_iso_secs(s: &str) -> Option<f64> {
    let s = s.trim();
    let (date, rest) = s.split_once('T')?;
    let mut dp = date.split('-');
    let (y, m, d): (i64, i64, i64) = (
        dp.next()?.parse().ok()?,
        dp.next()?.parse().ok()?,
        dp.next()?.parse().ok()?,
    );
    let (time, off) = if let Some(t) = rest.strip_suffix('Z') {
        (t, 0i64)
    } else if let Some(i) = rest.rfind(['+', '-']) {
        let (t, o) = rest.split_at(i);
        let sign = if o.starts_with('-') { -1 } else { 1 };
        let mut op = o[1..].split(':');
        let (oh, om): (i64, i64) = (
            op.next()?.parse().ok()?,
            op.next().unwrap_or("0").parse().ok()?,
        );
        (t, sign * (oh * 3600 + om * 60))
    } else {
        (rest, 0)
    };
    let mut tp = time.split(':');
    let (hh, mm): (i64, i64) = (tp.next()?.parse().ok()?, tp.next()?.parse().ok()?);
    let sec: f64 = tp.next().unwrap_or("0").parse().ok()?;
    // days from civil
    let (yy, mp) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = yy.div_euclid(400);
    let yoe = yy.rem_euclid(400);
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some((days * 86_400 + hh * 3600 + mm * 60 - off) as f64 + sec)
}

fn mtime_f(p: &Path) -> Option<f64> {
    fs::metadata(p)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs_f64())
}

// ---- the run marker -----------------------------------------------------------------------------------------------------------------------------

/// `Documents/gameapp.running`: "<start time> pid <pid>\ngame=<file>[\nbootonly=1]". Its presence at launch means the run never finished.
pub fn write_run_marker(
    docs: &Docs,
    game: &str,
    boot_only: bool,
    pid: u32,
    now_micros: i64,
) -> std::io::Result<()> {
    let secs = now_micros.div_euclid(1_000_000);
    let frac = now_micros.rem_euclid(1_000_000);
    let started = utc_iso(secs).trim_end_matches('Z').to_string() + &format!(".{frac:06}0Z");
    // tmp + rename (`write_atomic`): the next launch reads this file to say what was interrupted, and a kill in the middle of a plain write left it with no `started` / `game` / `bootonly`
    crate::util::write_atomic(
        &docs.running(),
        format!(
            "{started} pid {pid}\ngame={game}{}",
            if boot_only { "\nbootonly=1" } else { "" }
        )
        .as_bytes(),
    )
}

/// The marker removed: the run finished (or ended by design).
pub fn clear_run_marker(docs: &Docs) {
    let _ = fs::remove_file(docs.running());
}

// ---- the one-shot request (gameapp.args) ----------------------------------------------------------------------------------------------------------

/// What a `gameapp.args` asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunRequest {
    /// `chain-only`: skip the extraction, chain on the files already in `Documents/extract`
    ChainOnly,
    /// `boot-only`: the framework is built and signed; boot it in a process that rewrote nothing
    BootOnly,
    /// the extraction argv: `<keys> <base.nsp> <update|-> <out dir> [flags]`, every path re-anchored under the CURRENT Documents (an app reinstall gives the container a new UUID)
    Extraction {
        keys: PathBuf,
        base: PathBuf,
        update: Option<PathBuf>,
        out: PathBuf,
        flag: String,
        re_resolved: bool,
    },
}

/// `ResolveUnderDocuments`: anything remembered across launches is a hint -- only the last path component is kept -- never a path to trust.
pub fn resolve_under_documents(docs: &Docs, hint: &str) -> String {
    if hint.is_empty() || hint == "-" {
        return hint.to_string();
    }
    let name = hint.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    docs.join(name).to_string_lossy().into_owned()
}

/// `ConsumeArgs`: rename the one-shot request aside (kept as evidence, `gameapp.args.done`) so no relaunch can run it again; fall back to deleting it.
pub fn consume_args(docs: &Docs) {
    let (a, d) = (docs.args(), docs.args_done());
    if fs::rename(&a, &d).is_err() {
        let _ = fs::remove_file(&a);
    }
}

/// `AutoRunFromArgs`: read the request, consume it (one-shot: used once, then it cannot fire again), and parse it. `Err` carries the line for the screen's log.
pub fn read_args_request(docs: &Docs) -> Result<RunRequest, String> {
    let text = fs::read(docs.args()).map_err(|e| format!("gameapp.args: {e}"))?;
    let text = String::from_utf8_lossy(&text).into_owned();
    let lines: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
    consume_args(docs);
    if lines.len() == 1 && lines[0] == "chain-only" {
        return Ok(RunRequest::ChainOnly);
    }
    if lines.len() == 1 && lines[0] == "boot-only" {
        return Ok(RunRequest::BootOnly);
    }
    if lines.len() >= 5 {
        let r = |h: &str| resolve_under_documents(docs, h);
        let (keys, base, out) = (r(lines[0]), r(lines[1]), r(lines[3]));
        let update = if lines[2] == "-" {
            None
        } else {
            Some(PathBuf::from(r(lines[2])))
        };
        let re_resolved = keys != lines[0] || base != lines[1] || out != lines[3];
        return Ok(RunRequest::Extraction {
            keys: keys.into(),
            base: base.into(),
            update,
            out: out.into(),
            flag: lines[4].to_string(),
            re_resolved,
        });
    }
    Err(format!(
        "gameapp.args has {} lines; expected the extraction argv (5 lines) or 'chain-only'",
        lines.len()
    ))
}

/// What `BeginRun` does to the files before a run starts: the request is honoured whichever path wrote it (trap 19), a boot-only run makes sure the request exists
/// (an existing one is left as it is: re-arming an `attempted` one here would let a launch start it by itself if this process died before the pipeline marked it
/// again), and the interrupted-run marker is written.
pub fn begin_run(
    docs: &Docs,
    game: &str,
    boot_only: bool,
    pid: u32,
    now_micros: i64,
) -> Vec<String> {
    let mut notes = Vec::new();
    consume_args(docs);
    if boot_only
        && !docs.boot_only().exists()
        && let Err(e) = crate::util::write_atomic(
            &docs.boot_only(),
            "boot the framework that is already built and signed\n".as_bytes(),
        )
    {
        notes.push(format!(
            "could not write {}: {e}",
            docs.boot_only().display()
        ));
    }
    let _ = write_run_marker(docs, game, boot_only, pid, now_micros);
    notes
}

// ---- an interrupted run ----------------------------------------------------------------------------------------------------------------------------

/// `RecordKilledBuild`: an interrupted run that was a BUILD (not a boot): remembered for the library's Resume row and for the next run's log. `marker_text` is
/// `gameapp.running`, `stage_line` the last line of `gameapp.stage`. A stage file older than the run's start is the previous run's: the build was killed before the
/// chain wrote its first stage line, in the extraction (stage 0). Returns the note for the screen's log when a note was written.
pub fn record_killed_build(
    docs: &Docs,
    marker_text: &str,
    stage_line: &str,
    started: &str,
) -> Option<String> {
    let game = key_of(marker_text, "game");
    let killed = docs.killed();
    if key_of(marker_text, "bootonly") == "1" || game.is_empty() || !docs.join(&game).exists() {
        let _ = fs::remove_file(&killed);
        return None;
    }
    let mut stage = stage_number(stage_line);
    let mut stage_line = stage_line.to_string();
    let at = parse_iso_secs(started.split(' ').next().unwrap_or(""));
    let fresh = match (at, mtime_f(&docs.stage_file())) {
        (Some(at), Some(m)) => m >= at,
        _ => false,
    };
    if !fresh {
        stage = 0;
        stage_line = "(the extraction: the chain had not written a stage yet)".to_string();
    }
    if stage >= 5 {
        let _ = fs::remove_file(&killed); // the boot: a game's end, not a build's
        return None;
    }
    let mem = last_matching(&docs.stages_log(), "[mem:");
    let text = format!(
        "game={game}\nstagen={stage}\nstage={stage_line}\nmem={}\nstarted={started}\n",
        mem.trim()
    );
    if crate::util::write_atomic(&killed, text.as_bytes()).is_err() {
        return None;
    }
    Some(format!(
        "  iBuilder offers to resume the build of {game} (Documents/gameapp.killed)"
    ))
}

/// What `ReportInterruptedRunIfAny` found.
#[derive(Debug)]
pub struct InterruptedReport {
    /// a real interruption: this launch must NOT start another run (one run per explicit action)
    pub interrupted: bool,
    pub kind: PreviousRun,
    /// lines for the screen's log, in order
    pub notes: Vec<String>,
}

/// `ReportInterruptedRunIfAny`: a relaunch after a run that never finished must say so, not keep claiming to be running: the app writes `gameapp.running` at the start of a run
/// and removes it at the end, so its presence at launch is "the previous run was interrupted" -- except for the one end that is by design (a boot that ran to its time limit
/// or that the player ended, whose graceful exit finished): that one is said in a single line, without the banner. The last `[mem:` line and the stage it had reached are the
/// two facts that make a real interruption diagnosable, and the append-only logs still hold them.
pub fn report_interrupted_run(docs: &Docs) -> Option<InterruptedReport> {
    let marker = docs.running();
    if !marker.exists() {
        return None;
    }
    let marker_text = read_text(&marker).unwrap_or_default();
    let started = marker_text
        .split('\n')
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    let stage_line = last_line(&docs.stage_file());
    let chain_last = last_run_line_of(&docs.chain_log());
    let guest_last = last_line(&docs.guest_log());
    let guest_opt = if guest_last == "(none)" {
        None
    } else {
        Some(guest_last.as_str())
    };
    let kind = classify_previous_run(Some(&stage_line), Some(&chain_last), guest_opt);
    let mut notes = vec![String::new()];
    if matches!(kind, PreviousRun::DesignedEnd | PreviousRun::PlayerEnd) {
        notes.push(if kind == PreviousRun::PlayerEnd {
            format!("Previous run ended when you quit the game — the designed end, and the guest's exit completed (started {started}).")
        } else {
            format!("Previous run ended at its boot time limit — the designed end, and the guest's exit completed (started {started}).")
        });
        let _ = fs::remove_file(&marker);
        return Some(InterruptedReport {
            interrupted: false,
            kind,
            notes,
        });
    }
    notes.push(format!("PREVIOUS RUN WAS INTERRUPTED — it left Documents/gameapp.running behind (started {started})."));
    notes.push(format!("  last stage reached: {stage_line}"));
    notes.push(format!(
        "  last memory line:   {}",
        last_matching(&docs.stages_log(), "[mem:")
    ));
    if let Some(n) = record_killed_build(docs, &marker_text, &stage_line, &started) {
        notes.push(n);
    }
    if stage_line.contains("booting the guest") {
        notes.push(match kind {
            PreviousRun::DesignedEndWithoutCleanExit => format!(
                "  the boot ended by the shim's own signal (its time limit, or Quit game) — \"{chain_last}\" — but the guest's last line is \"{guest_last}\", not `graceful exit done`: the exit did not finish (it hung, or the process was killed during it). That is not the clean designed end; gameapp.guest.log holds what the guest said last."
            ),
            PreviousRun::FaultInGuest => format!(
                "  the guest died on a signal, and the shim named it before re-raising — \"{chain_last}\": that is a fault INSIDE the guest, not a kill from outside. Its backtrace is in Analytics Data (the GameApp .ips); gameapp.guest.log holds the guest's own last words."
            ),
            _ => "  the chain log stops at the boot call — no `dm_host_probe_boot returned`, no signal line: a kill while the guest runs writes nothing (jetsam and the watchdog are SIGKILL). The memory line above is what the guest had to work with; Analytics Data carries the verdict (a JetsamEvent, or the GameApp .ips).".to_string(),
        });
    }
    notes.push(
        "  the logs are kept across launches — use Export logs to get them off the device."
            .to_string(),
    );
    let _ = fs::remove_file(&marker);
    // The interrupted run already consumed its request: drop the stale argv so a later launch cannot run it (the other half of "do not start again by itself").
    let _ = fs::remove_file(docs.args());
    Some(InterruptedReport {
        interrupted: true,
        kind,
        notes,
    })
}

// ---- the launch decision ------------------------------------------------------------------------------------------------------------------------------

/// What the screen's code knows that Documents does not say.
pub struct LaunchFacts<'a> {
    /// this build of the app (`GameAppBuild.Id`)
    pub current_build: &'a str,
    /// `GameAppUi.CannotRunHere`: a distribution certificate (no get-task-allow) cannot run code from Documents
    pub cannot_run_here: bool,
    /// the framework this build would boot: `Documents/DmGameProbe.framework/DmGameProbe`, or an exported game's
    pub framework: &'a Path,
    /// `Documents/extract/flat`, or an exported game's
    pub flat: &'a Path,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LaunchAction {
    /// nothing starts by itself
    Nothing,
    /// a one-shot request: start it once
    Run(RunRequest),
    /// the last process ended on purpose before stage 3: continue that game's build (`stage` is the note's stage, default 3)
    ContinueBuild {
        game: String,
        stage: i32,
        why: String,
    },
    /// the chain's own request: boot the framework in this process, which has rewritten nothing
    BootOnly,
}

#[derive(Debug)]
pub struct LaunchPlan {
    pub action: LaunchAction,
    pub interrupted: bool,
    pub notes: Vec<String>,
}

/// The launch path of `GameAppViewController.ViewDidLoad`, in its order: report an interrupted run first (and then start NOTHING: after a kill an auto-run on every launch
/// became a loop of ~3.5 GiB attempts, docs/MEMORY.md), else a one-shot `gameapp.args`, else a pending continuation, else the chain's own boot-only request. A reopen after
/// a CLEAN end finishes the refusing run; a reopen after a kill is not a request, so this can never become a retry loop.
pub fn decide_launch(docs: &Docs, facts: &LaunchFacts<'_>) -> LaunchPlan {
    let had_args = docs.args().exists();
    let mut notes = Vec::new();
    let report = report_interrupted_run(docs);
    let interrupted = report.as_ref().map(|r| r.interrupted).unwrap_or(false);
    if let Some(r) = report {
        notes.extend(r.notes);
    }
    let mut action = LaunchAction::Nothing;
    if interrupted {
        if had_args {
            notes.push("gameapp.args present, but the previous run was interrupted — not starting again; tap Set up and play on a game to start one".into());
        }
    } else if docs.args().exists() {
        notes.push("gameapp.args present — starting once (one launch, one process)".into());
        match read_args_request(docs) {
            Ok(r) => {
                if let RunRequest::Extraction {
                    re_resolved: true, ..
                } = &r
                {
                    notes.push("gameapp.args: paths re-resolved under the current Documents (the stored ones were stale)".into());
                }
                action = LaunchAction::Run(r);
            }
            Err(e) => notes.push(e),
        }
    } else if docs.continue_file().exists() {
        let text = read_text(&docs.continue_file()).unwrap_or_default();
        let game = key_of(&text, "game");
        if game.is_empty() {
            let _ = fs::remove_file(docs.continue_file());
        } else {
            notes.push(format!("gameapp.continue present — the last process ended on purpose to give the rewrite's memory back; this launch continues the build of {game} from stage 3"));
            let stage = key_of(&text, "stage").parse::<i32>().unwrap_or(3);
            action = LaunchAction::ContinueBuild {
                game,
                stage,
                why: "memory between the rewrite and the shaders".into(),
            };
        }
    } else if docs.boot_only().exists() {
        let content = read_text(&docs.boot_only());
        if let Err(e) = &content {
            notes.push(format!("gameapp.boot-only is unreadable: {e} — not starting; tap Play to boot the framework"));
        }
        let request = content.ok();
        if let Some(req) = request {
            let on_disk = is_file(facts.framework) && facts.flat.is_dir();
            let stamp = if on_disk {
                read_text(&docs.framework_stamp()).ok()
            } else {
                None
            };
            let fw = classify_framework(on_disk, stamp.as_deref(), facts.current_build);
            if facts.cannot_run_here {
                // A distribution certificate: the boot would be refused at the dlopen ("file system sandbox blocked mmap()"), and starting it at every launch was the "iBuilder starts again" loop (2026-11-18).
                let _ = fs::remove_file(docs.boot_only());
                notes.push("gameapp.boot-only withdrawn: this install's certificate (distribution, no get-task-allow) cannot run a game from Documents — use Export as .ipa and install the game as its own app".into());
            } else if fw != FrameworkState::Fresh {
                // The request points at a framework THIS version of the app did not build (an older build's, or none): booting it would run an older host and rewrite.
                let _ = fs::remove_file(docs.boot_only());
                notes.push("gameapp.boot-only present, but the framework on disk was not built by this version of the app — withdrawn, nothing is booted; tap Set up and play on a game to rebuild".into());
            } else if parse_framework_stamp(stamp.as_deref()).is_some_and(|s| s.export_only) {
                // Signed ad hoc for export only (no .p12 at the build): iOS would refuse to map it from Documents, so it is not booted at launch either.
                let _ = fs::remove_file(docs.boot_only());
                notes.push("gameapp.boot-only withdrawn: the framework on disk was signed ad hoc for export only (no .p12 at the build) — Export it as its own app, or import your .p12 and sign it again to play here".into());
            } else if classify_boot_request(true, Some(&req)) == BootRequestState::Attempted {
                notes.push("gameapp.boot-only present, but a boot from it was started and never came back — not starting again; tap Play to boot the framework once more (nothing is rebuilt)".into());
            } else {
                notes.push("gameapp.boot-only present — the framework is built and signed; this launch boots it (~3.3 GiB free, nothing is rebuilt)".into());
                action = LaunchAction::BootOnly;
            }
        }
    }
    LaunchPlan {
        action,
        interrupted,
        notes,
    }
}

// ---- resume / continue --------------------------------------------------------------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
pub enum ResumePlan {
    /// killed in the extraction: all of it again (a normal "Set up and play")
    Whole,
    /// the same game's chain again on the files already extracted
    Continue { stage: i32 },
}

/// `ResumeKilledBuild`: an NSP/XCI game was extracted before its chain wrote a stage, so the extraction is not repeated; a .nup is read by the chain itself, which skips
/// that when the killed run had got past it (stage 2 and later).
pub fn resume_plan(game: &str, stage: i32) -> ResumePlan {
    let is_nup = game.to_lowercase().ends_with(".nup");
    if (!is_nup && stage < 1) || (is_nup && stage < 2) {
        ResumePlan::Whole
    } else {
        ResumePlan::Continue { stage }
    }
}

/// `StartContinuedBuild`'s file work: the chain reads `gameapp.continue.run` (`take_continue`) and the request is used once; a pending boot request would make the chain only boot.
pub fn prepare_continued_build(
    docs: &Docs,
    game: &str,
    stage: i32,
    why: &str,
) -> std::io::Result<()> {
    crate::util::write_atomic(
        &docs.continue_run(),
        format!("game={game}\nstage={stage}\nwhy={why}\n").as_bytes(),
    )?;
    let _ = fs::remove_file(docs.continue_file());
    let _ = fs::remove_file(docs.boot_only());
    Ok(())
}

/// `RestartForStage3`: the chain ended with its restart result and left `gameapp.continue`: the run ends BY DESIGN (an end, not a kill): the marker goes. Returns the
/// game to continue when the result and the file say this is that end.
pub fn restart_end_by_design(docs: &Docs, rc: i32, chain_error: bool) -> Option<String> {
    if rc != RESTART_RC || chain_error || !docs.continue_file().exists() {
        return None;
    }
    let game = key_of(
        &read_text(&docs.continue_file()).unwrap_or_default(),
        "game",
    );
    clear_run_marker(docs);
    Some(game)
}

/// A whole `read_kv` of the killed note, for the library's banner.
pub fn read_killed_note(docs: &Docs) -> std::collections::BTreeMap<String, String> {
    read_kv(&docs.killed())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{TempDir, write};

    #[test]
    fn boot_request_states() {
        use BootRequestState::{Armed, Attempted};
        assert_eq!(classify_boot_request(false, None), BootRequestState::None);
        assert_eq!(
            classify_boot_request(false, Some("attempted pid 1")),
            BootRequestState::None,
            "no file wins over any content"
        );
        assert_eq!(
            classify_boot_request(
                true,
                Some("boot the framework that is already built and signed\n")
            ),
            Armed
        );
        assert_eq!(
            classify_boot_request(
                true,
                Some(
                    "boot /x in a fresh process (in-process boot refused: 1095 MiB available < 2500 MiB)\n"
                )
            ),
            Armed
        );
        assert_eq!(
            classify_boot_request(
                true,
                Some(
                    "attempted pid 6888: a boot started from this request and has not returned. A tap boots again; a launch does not\n"
                )
            ),
            Attempted
        );
        assert_eq!(
            classify_boot_request(true, None),
            Attempted,
            "an unreadable request is the safe reading"
        );
        assert_eq!(
            classify_boot_request(true, Some("")),
            Armed,
            "an empty file is armed"
        );
        assert_eq!(
            classify_boot_request(true, Some("not attempted")),
            Armed,
            "the word is matched at the start only"
        );
    }

    #[test]
    fn framework_stamp_and_state() {
        let stamp = "app_build=20261117-173055-c842913\ngame=Grizzy.nsp\ncontent=u:a|d:b\n";
        let p = parse_framework_stamp(Some(stamp)).unwrap();
        assert_eq!(
            (p.app_build.as_str(), p.game.as_str(), p.content.as_deref()),
            ("20261117-173055-c842913", "Grizzy.nsp", Some("u:a|d:b"))
        );
        assert_eq!(
            parse_framework_stamp(Some("x=1\r\napp_build=B1\r\n\r\ngame=G\r\n"))
                .unwrap()
                .app_build,
            "B1",
            "CRLF and blank lines"
        );
        assert_eq!(parse_framework_stamp(None), None);
        assert_eq!(
            parse_framework_stamp(Some("game=Grizzy.nsp\n")),
            None,
            "a stamp that names no build is no stamp"
        );
        assert_eq!(parse_framework_stamp(Some("app_build=\n")), None);
        assert_eq!(
            parse_framework_stamp(Some("app_build=B1\n")).unwrap().game,
            ""
        );
        let b = "20261117-173055-c842913";
        assert_eq!(
            classify_framework(false, Some(stamp), b),
            FrameworkState::None
        );
        assert_eq!(
            classify_framework(true, Some(stamp), b),
            FrameworkState::Fresh
        );
        assert_eq!(
            classify_framework(true, Some(stamp), "20261118-090000-aaaaaaa"),
            FrameworkState::Stale
        );
        assert_eq!(classify_framework(true, None, b), FrameworkState::Stale);
        assert_eq!(
            classify_framework(true, Some("garbage"), b),
            FrameworkState::Stale
        );
        assert_eq!(
            classify_framework(true, Some(stamp), ""),
            FrameworkState::Stale
        );
        assert_eq!(
            classify_framework(true, Some(stamp), &b.to_uppercase()),
            FrameworkState::Stale,
            "the comparison is ordinal"
        );
    }

    #[test]
    fn previous_run_classification() {
        use PreviousRun::*;
        let booting = "stage 5/5: booting the guest (dlopen + dm_host_probe_boot -> nnMain; quit from the gear menu)";
        let limit = "[gameapp] boot time limit 30 s reached: sending SIGTERM to self (the CAUSE of this exit, not a crash)";
        let consumed = "[gameapp] Documents/gameapp.boot-only consumed: the boot ran to its time limit, the designed end";
        let clean = "[dm] graceful exit done";
        assert_eq!(
            classify_previous_run(Some(booting), Some(limit), Some(clean)),
            DesignedEnd
        );
        assert_eq!(
            classify_previous_run(Some(booting), Some(consumed), Some(clean)),
            DesignedEnd
        );
        assert_eq!(
            classify_previous_run(
                Some(booting),
                Some(limit),
                Some("[dm] audio suspended (background / interruption)")
            ),
            DesignedEndWithoutCleanExit
        );
        assert_eq!(
            classify_previous_run(Some(booting), Some(limit), None),
            DesignedEndWithoutCleanExit
        );
        assert_eq!(
            classify_previous_run(
                Some(booting),
                Some(
                    "[gameapp] received SIGSEGV: the process is terminating here (this line is the cause the log carries)"
                ),
                Some("[dm] CRASH sig=11")
            ),
            FaultInGuest
        );
        assert_eq!(
            classify_previous_run(
                Some(booting),
                Some("[gameapp] dlopen OK"),
                Some("[dm] >>> nnMain")
            ),
            KilledOrUnknown
        );
        assert_eq!(
            classify_previous_run(
                Some(booting),
                Some(
                    "[gameapp] boot time limit of 30 s disarmed at 3 s: the boot returned first, no SIGTERM is sent"
                ),
                Some(clean)
            ),
            KilledOrUnknown
        );
        assert_eq!(
            classify_previous_run(
                Some("stage 2/5: rewriting the modules (1/3: main)"),
                Some(limit),
                Some(clean)
            ),
            KilledOrUnknown,
            "an older run's time-limit line never excuses a run that died earlier"
        );
        assert_eq!(
            classify_previous_run(None, Some(limit), Some(clean)),
            KilledOrUnknown
        );
        let quit = "[gameapp] the player ended the game: sending SIGTERM to self (the CAUSE of this exit, not a crash)";
        assert_eq!(
            classify_previous_run(Some(booting), Some(quit), Some(clean)),
            PlayerEnd
        );
        assert_eq!(
            classify_previous_run(Some(booting), Some(quit), Some("[dm] audio suspended")),
            DesignedEndWithoutCleanExit
        );
        assert_eq!(
            classify_previous_run(Some("stage 3/5: shaders"), Some(quit), Some(clean)),
            KilledOrUnknown
        );
    }

    #[test]
    fn last_run_line_skips_this_launchs_own_lines() {
        let limit = "[gameapp] boot time limit 30 s reached: sending SIGTERM to self";
        let after = [
            "[gameapp] as-census at launch, before the .NET runtime: x",
            limit,
            "",
            "[gameapp] as-census at launch, nothing reserved: y",
            "[gameapp] as-reserve: skipped -- z",
        ];
        assert_eq!(last_run_line(&after), limit);
        assert_eq!(
            last_run_line(&[
                "[gameapp] dlopen OK",
                "[gameapp] as-census after the reservation: x"
            ]),
            "[gameapp] dlopen OK"
        );
        let empty: [&str; 0] = [];
        assert_eq!(last_run_line(&empty), "(none)");
        assert_eq!(
            last_run_line(&["", "[gameapp] as-census a", " ", "[gameapp] as-reserve: b"]),
            "(none)"
        );
    }

    #[test]
    fn iso_round_trips() {
        let t = parse_iso_secs("2026-11-19T10:00:00.5000000Z").unwrap();
        assert_eq!(t, 1_795_082_400.5);
        assert_eq!(parse_iso_secs("1970-01-01T01:00:00+01:00").unwrap(), 0.0);
        assert_eq!(parse_iso_secs("garbage"), None);
    }

    #[test]
    fn args_are_one_shot_and_reanchored() {
        let d = TempDir::new("args");
        let docs = Docs::new(d.path());
        write(
            docs.args(),
            "/old/container/Documents/prod.keys\n/old/container/Documents/Grizzy.nsp\n-\n/old/container/Documents/extract\n--romfs-image\n",
        );
        let r = read_args_request(&docs).unwrap();
        match r {
            RunRequest::Extraction {
                keys,
                base,
                update,
                out,
                flag,
                re_resolved,
            } => {
                assert_eq!(keys, docs.join("prod.keys"));
                assert_eq!(base, docs.join("Grizzy.nsp"));
                assert_eq!(update, None);
                assert_eq!(out, docs.join("extract"));
                assert_eq!(flag, "--romfs-image");
                assert!(re_resolved);
            }
            other => panic!("{other:?}"),
        }
        assert!(
            !docs.args().exists() && docs.args_done().exists(),
            "consumed where it is used: renamed aside, kept as evidence"
        );
        write(docs.args(), "chain-only\n");
        assert_eq!(read_args_request(&docs).unwrap(), RunRequest::ChainOnly);
        write(docs.args(), "boot-only\n");
        assert_eq!(read_args_request(&docs).unwrap(), RunRequest::BootOnly);
        write(docs.args(), "a\nb\n");
        assert!(read_args_request(&docs).unwrap_err().contains("2 lines"));
        assert!(
            !docs.args().exists(),
            "a malformed request is consumed too: it cannot fire again"
        );
    }

    fn facts<'a>(fw: &'a Path, flat: &'a Path, build: &'a str) -> LaunchFacts<'a> {
        LaunchFacts {
            current_build: build,
            cannot_run_here: false,
            framework: fw,
            flat,
        }
    }

    #[test]
    fn launch_after_a_kill_starts_nothing_and_drops_the_stale_args() {
        let d = TempDir::new("launch-kill");
        let docs = Docs::new(d.path());
        write(
            docs.running(),
            "2026-11-19T10:00:00.0000000Z pid 5\ngame=Grizzy.nsp",
        );
        write(
            docs.stage_file(),
            "stage 2/5: rewriting the modules (1/3: main)\n",
        );
        write(docs.args(), "chain-only\n");
        write(docs.join("Grizzy.nsp"), "x");
        write(
            docs.stages_log(),
            "  [mem:before-rewrite] resident 300 MiB\n",
        );
        let (fw, flat) = (docs.join("fw"), docs.join("flat"));
        let plan = decide_launch(&docs, &facts(&fw, &flat, "B"));
        assert!(plan.interrupted);
        assert_eq!(
            plan.action,
            LaunchAction::Nothing,
            "a launch never starts a run by itself after a kill (trap 18)"
        );
        assert!(!docs.running().exists(), "the marker is reported once");
        assert!(
            !docs.args().exists(),
            "the stale argv goes: a later launch cannot run it"
        );
        assert!(
            plan.notes
                .iter()
                .any(|n| n.contains("PREVIOUS RUN WAS INTERRUPTED"))
        );
        assert!(plan.notes.iter().any(|n| n.contains("not starting again")));
        // a stage file older than the run's start: it was killed in the extraction
        let note = read_killed_note(&docs);
        assert_eq!(
            note.get("game").map(String::as_str),
            Some("Grizzy.nsp"),
            "{note:?}"
        );
    }

    #[test]
    fn a_kill_after_a_fresh_stage_line_keeps_the_stage_for_resume() {
        let d = TempDir::new("launch-kill2");
        let docs = Docs::new(d.path());
        write(docs.join("Grizzy.nsp"), "x");
        write(
            docs.running(),
            "2026-01-01T00:00:00.0000000Z pid 5\ngame=Grizzy.nsp",
        );
        write(
            docs.stage_file(),
            "stage 3/5: shaders: translating for this iPhone (938/21990 shaders)\n",
        );
        write(
            docs.stages_log(),
            "  [mem:shaders 938/21990] resident 2926 MiB\n",
        );
        let r = report_interrupted_run(&docs).unwrap();
        assert!(r.interrupted);
        let note = read_killed_note(&docs);
        assert_eq!(note.get("stagen").map(String::as_str), Some("3"));
        assert!(note["mem"].contains("[mem:shaders 938/21990]"));
        assert_eq!(
            resume_plan("Grizzy.nsp", 3),
            ResumePlan::Continue { stage: 3 }
        );
        assert_eq!(
            resume_plan("Grizzy.nsp", 0),
            ResumePlan::Whole,
            "killed in the extraction: all of it again"
        );
        assert_eq!(
            resume_plan("Game.nup", 1),
            ResumePlan::Whole,
            "a .nup is read by the chain: skipped only past stage 1"
        );
        assert_eq!(
            resume_plan("Game.NUP", 2),
            ResumePlan::Continue { stage: 2 }
        );
    }

    #[test]
    fn a_kill_in_the_boot_is_a_games_end_not_a_builds() {
        let d = TempDir::new("launch-boot-kill");
        let docs = Docs::new(d.path());
        write(docs.join("Grizzy.nsp"), "x");
        write(
            docs.running(),
            "2026-01-01T00:00:00.0000000Z pid 5\ngame=Grizzy.nsp",
        );
        write(docs.killed(), "stale");
        write(
            docs.stage_file(),
            "stage 5/5: booting the guest (dlopen + dm_host_probe_boot -> nnMain; quit from the gear menu)\n",
        );
        let r = report_interrupted_run(&docs).unwrap();
        assert!(r.interrupted);
        assert!(
            !docs.killed().exists(),
            "no resume offer for a kill inside the boot"
        );
        assert!(
            r.notes
                .iter()
                .any(|n| n.contains("the chain log stops at the boot call"))
        );
    }

    #[test]
    fn the_designed_end_is_one_quiet_line() {
        let d = TempDir::new("launch-designed");
        let docs = Docs::new(d.path());
        write(docs.running(), "2026-01-01T00:00:00.0000000Z pid 5\ngame=x");
        write(docs.stage_file(), "stage 5/5: booting the guest (x)\n");
        write(
            docs.chain_log(),
            "[gameapp] the player ended the game: sending SIGTERM to self\n[gameapp] as-census at launch, before the .NET runtime: a\n",
        );
        write(docs.guest_log(), "[dm] graceful exit done\n");
        let r = report_interrupted_run(&docs).unwrap();
        assert!(!r.interrupted && r.kind == PreviousRun::PlayerEnd);
        assert!(r.notes.iter().any(|n| n.contains("quit the game")));
        assert!(!docs.running().exists());
    }

    #[test]
    fn boot_only_request_on_launch() {
        let d = TempDir::new("launch-boot");
        let docs = Docs::new(d.path());
        let (fw, flat) = (
            docs.join("DmGameProbe.framework/DmGameProbe"),
            docs.join("extract/flat"),
        );
        write(&fw, "x");
        std::fs::create_dir_all(&flat).unwrap();
        write(docs.framework_stamp(), "app_build=B1\ngame=G\n");
        write(
            docs.boot_only(),
            "boot the framework that is already built and signed\n",
        );
        let plan = decide_launch(&docs, &facts(&fw, &flat, "B1"));
        assert_eq!(
            plan.action,
            LaunchAction::BootOnly,
            "an armed request is started by the launch (a reopen after a CLEAN end)"
        );
        write(
            docs.boot_only(),
            "attempted pid 6888: a boot started from this request and has not returned.\n",
        );
        let plan = decide_launch(&docs, &facts(&fw, &flat, "B1"));
        assert_eq!(
            plan.action,
            LaunchAction::Nothing,
            "an attempted request is never started on sight (no retry loop)"
        );
        assert!(
            docs.boot_only().exists(),
            "but it stays: a tap boots again for the price of a dlopen"
        );
        let plan = decide_launch(&docs, &facts(&fw, &flat, "B2"));
        assert_eq!(plan.action, LaunchAction::Nothing);
        assert!(
            !docs.boot_only().exists(),
            "a framework another build made is never booted: the request is withdrawn"
        );
        write(
            docs.boot_only(),
            "boot the framework that is already built and signed\n",
        );
        let mut f = facts(&fw, &flat, "B1");
        f.cannot_run_here = true;
        let plan = decide_launch(&docs, &f);
        assert_eq!(plan.action, LaunchAction::Nothing);
        assert!(
            !docs.boot_only().exists(),
            "a distribution certificate withdraws it (the 'starts again' loop)"
        );
        // a framework signed ad hoc for export only (no .p12 at the build): never booted at launch, the request is withdrawn
        write(
            docs.framework_stamp(),
            "app_build=B1\ngame=G\nbuilt=t\ncontent=\nsignature=adhoc-export\n",
        );
        write(
            docs.boot_only(),
            "boot the framework that is already built and signed\n",
        );
        let plan = decide_launch(&docs, &facts(&fw, &flat, "B1"));
        assert_eq!(plan.action, LaunchAction::Nothing);
        assert!(!docs.boot_only().exists());
        assert!(
            plan.notes.iter().any(|n| n.contains("export only")),
            "{:?}",
            plan.notes
        );
    }

    #[test]
    fn an_export_only_stamp_is_read_as_such() {
        let p = parse_framework_stamp(Some(
            "app_build=B\ngame=G.nsp\nbuilt=t\ncontent=\nsignature=adhoc-export\n",
        ))
        .unwrap();
        assert!(p.export_only);
        assert_eq!(
            classify_framework(true, Some("app_build=B\nsignature=adhoc-export\n"), "B"),
            FrameworkState::Fresh,
            "an export-only build is still this build's (Export IPA needs it)"
        );
        assert!(
            !parse_framework_stamp(Some("app_build=B\ngame=G\n"))
                .unwrap()
                .export_only
        );
        assert!(
            !parse_framework_stamp(Some("app_build=B\nsignature=device\n"))
                .unwrap()
                .export_only
        );
    }

    #[test]
    fn continue_at_launch_and_the_restart_end() {
        let d = TempDir::new("launch-continue");
        let docs = Docs::new(d.path());
        let (fw, flat) = (docs.join("fw"), docs.join("flat"));
        write(
            docs.continue_file(),
            "game=Grizzy.nsp\nstage=3\nwhy=memory\n",
        );
        let plan = decide_launch(&docs, &facts(&fw, &flat, "B"));
        assert_eq!(
            plan.action,
            LaunchAction::ContinueBuild {
                game: "Grizzy.nsp".into(),
                stage: 3,
                why: "memory between the rewrite and the shaders".into()
            }
        );
        write(docs.join("Grizzy.nsp"), "x");
        write(docs.boot_only(), "boot");
        prepare_continued_build(&docs, "Grizzy.nsp", 3, "killed").unwrap();
        assert_eq!(
            std::fs::read_to_string(docs.continue_run()).unwrap(),
            "game=Grizzy.nsp\nstage=3\nwhy=killed\n"
        );
        assert!(
            !docs.continue_file().exists() && !docs.boot_only().exists(),
            "a pending boot request would make the chain only boot"
        );
        // the chain ended on purpose: the marker goes, it is an end by design
        write(docs.continue_file(), "game=Grizzy.nsp\nstage=3\n");
        write(docs.running(), "x");
        assert_eq!(restart_end_by_design(&docs, 1, false), None);
        assert_eq!(restart_end_by_design(&docs, RESTART_RC, true), None);
        assert_eq!(
            restart_end_by_design(&docs, RESTART_RC, false).as_deref(),
            Some("Grizzy.nsp")
        );
        assert!(!docs.running().exists());
        // an empty continue request is dropped
        write(docs.continue_file(), "stage=3\n");
        let plan = decide_launch(&docs, &facts(&fw, &flat, "B"));
        assert_eq!(plan.action, LaunchAction::Nothing);
        assert!(!docs.continue_file().exists());
    }

    #[test]
    fn begin_run_writes_the_marker_and_consumes_args() {
        let d = TempDir::new("beginrun");
        let docs = Docs::new(d.path());
        write(docs.args(), "chain-only\n");
        let notes = begin_run(
            &docs,
            "boot-only (the framework is built and signed)",
            true,
            4242,
            1_795_082_400_000_000,
        );
        assert!(notes.is_empty());
        let m = std::fs::read_to_string(docs.running()).unwrap();
        assert!(
            m.starts_with("2026-11-19T10:00:00.0000000Z pid 4242\ngame=boot-only"),
            "{m}"
        );
        assert!(m.contains("\nbootonly=1"));
        assert!(docs.boot_only().exists());
        assert!(
            !docs.args().exists(),
            "a request is consumed where it is used, whichever path wrote it (trap 19)"
        );
        // an existing request (even attempted) is left as it is
        write(docs.boot_only(), "attempted pid 1");
        begin_run(&docs, "x", true, 1, 0);
        assert_eq!(
            std::fs::read_to_string(docs.boot_only()).unwrap(),
            "attempted pid 1"
        );
    }
}
