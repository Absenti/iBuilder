//! Log files with rotation: the last 10 minutes of every log file, complete (port of `LogWindow` in `gameapp/GameAppLogs.cs`, without the zip, which stays with the UI).
//!
//! The log files are written by different hands (the app, the chain, the shim, the guest's own stdout) and only the app's has lines it could time-stamp. So the window is kept the
//! format-independent way: every 30 s the app writes down each log file's length and the time ("marks", `Documents/gameapp.logmarks`). The last 10 minutes of a file are then everything from
//! the length it had at the newest mark that is 10 minutes old or older, to its end: no line inside the window is cut and no byte cap sits in the middle. What is older may go.

use crate::layout::{CHAIN_LOG, Docs, GUEST_LOG, STAGE_FILE, STAGES_LOG, UI_LOG};
use crate::util::list_names;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::sync::Mutex;

pub const WINDOW_SECS: i64 = 600;

const NAMED: [&str; 5] = [UI_LOG, STAGES_LOG, CHAIN_LOG, STAGE_FILE, GUEST_LOG];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mark {
    /// seconds since the epoch (the C# app stored ticks; this crate stores whole seconds, so a marks file from before is ignored once)
    pub at: i64,
    pub name: String,
    pub length: u64,
}

static GATE: Mutex<()> = Mutex::new(());

/// Every log file of the app, as paths relative to Documents: the known ones, any other `*.log` in Documents, and what is in `Documents/logs`. Only files that exist.
pub fn log_files(docs: &Docs) -> Vec<String> {
    let mut found: Vec<String> = NAMED
        .iter()
        .filter(|n| docs.join(n).exists())
        .map(|n| n.to_string())
        .collect();
    for f in list_names(docs.root()) {
        if f.ends_with(".log") && !found.contains(&f) && docs.join(&f).is_file() {
            found.push(f);
        }
    }
    let sub = docs.join("logs");
    for f in list_names(&sub) {
        found.push(format!("logs/{f}"));
    }
    found
}

pub fn mark_line(m: &Mark) -> String {
    format!("{}\t{}\t{}", m.at, m.name, m.length)
}

pub fn parse_marks(text: &str) -> Vec<Mark> {
    text.lines()
        .filter_map(|l| {
            let p: Vec<&str> = l.trim_end_matches('\r').split('\t').collect();
            if p.len() != 3 {
                return None;
            }
            let (at, length) = (p[0].parse::<i64>().ok()?, p[2].parse::<u64>().ok()?);
            if at <= 0 {
                return None;
            }
            Some(Mark {
                at,
                name: p[1].to_string(),
                length,
            })
        })
        .collect()
}

/// Where the window of `name` starts: the length the file had at its newest mark that is `WINDOW_SECS` old or older (never past the file's end: a file that was replaced shorter is kept whole).
/// 0 when no mark is old enough.
pub fn start_offset(marks: &[Mark], name: &str, now: i64, length: u64) -> u64 {
    let (mut start, mut best) = (0u64, i64::MIN);
    for m in marks {
        if m.name == name && now - m.at >= WINDOW_SECS && m.at > best && m.length <= length {
            best = m.at;
            start = m.length;
        }
    }
    start
}

/// The marks worth keeping: everything inside the window, and per file the newest one just outside it.
pub fn prune(marks: &[Mark], now: i64) -> Vec<Mark> {
    let mut keep: Vec<Mark> = marks
        .iter()
        .filter(|m| now - m.at < WINDOW_SECS)
        .cloned()
        .collect();
    let mut names: Vec<&String> = marks
        .iter()
        .filter(|m| now - m.at >= WINDOW_SECS)
        .map(|m| &m.name)
        .collect();
    names.sort();
    names.dedup();
    for n in names {
        if let Some(m) = marks
            .iter()
            .filter(|m| &m.name == n && now - m.at >= WINDOW_SECS)
            .max_by_key(|m| m.at)
        {
            keep.push(m.clone());
        }
    }
    keep.sort_by_key(|m| m.at);
    keep
}

fn read_marks(docs: &Docs) -> Vec<Mark> {
    fs::read_to_string(docs.logmarks())
        .map(|t| parse_marks(&t))
        .unwrap_or_default()
}

fn write_marks(docs: &Docs, marks: &[Mark]) {
    let text = marks.iter().map(mark_line).collect::<Vec<_>>().join("\n") + "\n";
    // through a `.tmp`: a kill between truncating and writing must not leave marks that read as "no history" (review r2 N7). A failure only means the window is computed from older marks. There is no
    // log handle here (the app calls `mark` / `trim_to_window` with a folder and a time), so it is said where the chain's lines go when no `Ctx` is at hand: the hidden diagnostics.
    if let Err(e) = crate::util::write_atomic(&docs.logmarks(), text.as_bytes()) {
        crate::diag::log_line(&format!(
            "log marks: could not write {}: {e} -- the log window is computed from the older marks",
            docs.logmarks().display()
        ));
    }
}

/// Writes down each log file's length now. Called every 30 s while the app is open, and at launch.
pub fn mark(docs: &Docs, now: i64) {
    let _g = GATE.lock().unwrap_or_else(|e| e.into_inner());
    let mut marks = read_marks(docs);
    for f in log_files(docs) {
        if let Ok(m) = fs::metadata(docs.join(&f)) {
            marks.push(Mark {
                at: now,
                name: f,
                length: m.len(),
            });
        }
    }
    write_marks(docs, &prune(&marks, now));
}

/// Rewrites `name` (a file nothing else holds open: the ui log) from the start of its window to its end, and moves its marks with it. Nothing inside the window is cut. False when there was nothing
/// to drop.
pub fn trim_to_window(docs: &Docs, name: &str, now: i64) -> bool {
    let _g = GATE.lock().unwrap_or_else(|e| e.into_inner());
    let go = || -> std::io::Result<bool> {
        let path = docs.join(name);
        let length = fs::metadata(&path)?.len();
        let marks = read_marks(docs);
        let start = start_offset(&marks, name, now, length);
        if start == 0 {
            return Ok(false);
        }
        let tmp = docs.join(format!("{name}.trim"));
        {
            let mut input = fs::File::open(&path)?;
            input.seek(SeekFrom::Start(start))?;
            let mut out = fs::File::create(&tmp)?;
            let mut buf = vec![0u8; 1 << 20];
            loop {
                let n = input.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                std::io::Write::write_all(&mut out, &buf[..n])?;
            }
        }
        fs::rename(&tmp, &path)?;
        let moved: Vec<Mark> = marks
            .into_iter()
            .filter(|m| m.name != name || m.length >= start)
            .map(|m| {
                if m.name == name {
                    Mark {
                        length: m.length - start,
                        ..m
                    }
                } else {
                    m
                }
            })
            .collect();
        write_marks(docs, &moved);
        Ok(true)
    };
    go().unwrap_or(false)
}

/// The bytes of `name` inside its window (what the zip of the last 10 minutes holds): from `start_offset` to the end.
pub fn window_bytes(docs: &Docs, name: &str, now: i64) -> std::io::Result<Vec<u8>> {
    let marks = read_marks(docs);
    let mut f = fs::File::open(docs.join(name))?;
    let len = f.metadata()?.len();
    f.seek(SeekFrom::Start(start_offset(&marks, name, now, len)))?;
    let mut v = Vec::new();
    f.read_to_end(&mut v)?;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{TempDir, write};

    fn m(at: i64, name: &str, length: u64) -> Mark {
        Mark {
            at,
            name: name.into(),
            length,
        }
    }

    #[test]
    fn start_offset_is_the_newest_mark_at_least_ten_minutes_old() {
        let now = 100_000;
        let marks = vec![
            m(now - 3600, "a.log", 100),
            m(now - 700, "a.log", 400),
            m(now - 300, "a.log", 900),
            m(now - 700, "b.log", 7),
            m(now - 20, "c.log", 1),
        ];
        assert_eq!(start_offset(&marks, "a.log", now, 2000), 400);
        assert_eq!(start_offset(&marks, "b.log", now, 50), 7);
        assert_eq!(
            start_offset(&marks, "c.log", now, 50),
            0,
            "no mark old enough: the whole file"
        );
        assert_eq!(
            start_offset(&marks, "a.log", now, 300),
            100,
            "a mark past the file's end (replaced shorter) is skipped"
        );
    }

    #[test]
    fn prune_keeps_the_window_and_one_mark_just_outside() {
        let now = 100_000;
        let marks = vec![
            m(now - 3600, "a.log", 1),
            m(now - 1200, "a.log", 2),
            m(now - 700, "a.log", 3),
            m(now - 100, "a.log", 4),
            m(now - 900, "b.log", 5),
        ];
        let p = prune(&marks, now);
        assert_eq!(
            p.iter()
                .map(|x| (x.name.as_str(), x.length))
                .collect::<Vec<_>>(),
            vec![("b.log", 5), ("a.log", 3), ("a.log", 4)]
        );
    }

    #[test]
    fn marks_round_trip_and_garbage_is_ignored() {
        let line = mark_line(&m(1_795_000_000, "x.log", 42));
        assert_eq!(
            parse_marks(&format!("{line}\ngarbage\n\n0\tz\t1\n")),
            vec![m(1_795_000_000, "x.log", 42)]
        );
    }

    #[test]
    fn trim_drops_only_what_is_older_than_the_window() {
        let d = TempDir::new("logwin");
        let docs = Docs::new(d.path());
        let now = 1_795_000_000;
        write(
            docs.join(UI_LOG),
            "old line 1\nold line 2\nline inside the window 1\nline inside the window 2\n",
        );
        let cut = "old line 1\nold line 2\n".len() as u64;
        write(
            docs.logmarks(),
            format!(
                "{}\n{}\n",
                mark_line(&m(now - 660, UI_LOG, cut)),
                mark_line(&m(now - 300, UI_LOG, cut + 10))
            ),
        );
        assert!(trim_to_window(&docs, UI_LOG, now));
        assert_eq!(
            fs::read_to_string(docs.join(UI_LOG)).unwrap(),
            "line inside the window 1\nline inside the window 2\n"
        );
        let marks = read_marks(&docs);
        assert!(
            marks.iter().all(|x| x.length <= 52),
            "marks move with the file: {marks:?}"
        );
        assert!(!trim_to_window(&docs, UI_LOG, now), "nothing older is left");
    }

    #[test]
    fn mark_writes_every_log_files_length() {
        let d = TempDir::new("logmark");
        let docs = Docs::new(d.path());
        write(docs.join(CHAIN_LOG), "12345");
        write(docs.join("other.log"), "xy");
        write(docs.join("logs/a.txt"), "z");
        write(docs.join("not-a-log.txt"), "n");
        mark(&docs, 1_795_000_000);
        let marks = read_marks(&docs);
        let names: Vec<&str> = marks.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, vec![CHAIN_LOG, "other.log", "logs/a.txt"]);
        assert_eq!(marks[0].length, 5);
        assert_eq!(
            window_bytes(&docs, CHAIN_LOG, 1_795_000_000).unwrap(),
            b"12345",
            "no old mark: the whole file"
        );
    }
}
