//! Small helpers shared by the whole crate: UTC time stamps, Python-compatible integer parsing, tolerant file reads.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const MIB: u64 = 1 << 20;

/// A MiB count read from the config as bytes: negative reads as 0 and a huge value saturates (`* MIB` would wrap to a small number).
pub fn mib_bytes(v: i64) -> u64 {
    (v.max(0) as u64).saturating_mul(MIB)
}
pub const GIB: u64 = 1 << 30;

/// Seconds since the Unix epoch (0 if the clock is before it).
pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime(secs))`
pub fn utc_iso(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// The same for "now".
pub fn utc_now_iso() -> String {
    utc_iso(now_secs())
}

/// Days since 1970-01-01 -> (year, month, day) in the proleptic Gregorian calendar (Howard Hinnant's algorithm).
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Python's `int(s, 0)`: optional sign, `0x`/`0o`/`0b` prefixes, otherwise decimal (a leading zero followed by other digits is
/// an error, as in Python); surrounding whitespace is ignored. `None` where Python raises ValueError.
pub fn parse_int0(s: &str) -> Option<i128> {
    let s = s.trim();
    let (neg, body) = match s.as_bytes().first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    if body.is_empty() {
        return None;
    }
    let lower = body.to_ascii_lowercase();
    let (radix, digits): (u32, &str) = if let Some(r) = lower.strip_prefix("0x") {
        (16, r)
    } else if let Some(r) = lower.strip_prefix("0o") {
        (8, r)
    } else if let Some(r) = lower.strip_prefix("0b") {
        (2, r)
    } else {
        if lower.len() > 1 && lower.starts_with('0') && lower.bytes().any(|b| b != b'0') {
            return None;
        }
        (10, lower.as_str())
    };
    if digits.is_empty() || !digits.bytes().all(|b| (b as char).is_digit(radix)) {
        return None;
    }
    let v = i128::from_str_radix(digits, radix).ok()?;
    Some(if neg { -v } else { v })
}

/// `-(-a // b) * b`: `a` rounded up to a multiple of `b`.
pub fn round_up(a: i128, b: i128) -> i128 {
    -((-a).div_euclid(b)) * b
}

/// The file as text, invalid UTF-8 replaced (Python's `errors='replace'`).
pub fn read_text(path: &Path) -> io::Result<String> {
    Ok(String::from_utf8_lossy(&fs::read(path)?).into_owned())
}

/// The block `rfind_line` reads at a time, from the end of the file.
pub const TAIL_BLOCK: u64 = 64 << 10;

/// The lines of a text file from the LAST to the first, read backwards in `TAIL_BLOCK`s (review r4 §6.3: logs that grow across launches were read whole to find one line near their end),
/// until `want` returns something: that, and the byte offset in the file where its line starts. The lines are exactly `read_text(path)?.lines()`'s (each one decoded on its own: a newline is
/// never part of a damaged UTF-8 sequence, so `from_utf8_lossy` of a line is that line of the whole text's), and each byte is read once: no match costs one read of the file, as before.
pub fn rfind_line<T>(
    path: &Path,
    mut want: impl FnMut(&str) -> Option<T>,
) -> io::Result<Option<(T, u64)>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = fs::File::open(path)?;
    let mut pos = f.metadata()?.len(); // everything from `pos + head.len()` to the end has been given to `want`
    let mut head: Vec<u8> = Vec::new(); // the bytes from `pos` on that may be the end of a line starting before `pos`
    loop {
        let start = pos.saturating_sub(TAIL_BLOCK);
        let mut data = vec![0u8; usize::try_from(pos - start).unwrap_or(0)];
        f.seek(SeekFrom::Start(start))?;
        f.read_exact(&mut data)?;
        data.extend_from_slice(&head);
        // the complete lines of `data`: after its first newline, or all of it at the start of the file
        let from = if start == 0 {
            0
        } else {
            match data.iter().position(|&b| b == b'\n') {
                Some(i) => i + 1,
                None => data.len(),
            }
        };
        let mut end = data.len();
        while end > from {
            let piece_start = data[from..end - 1]
                .iter()
                .rposition(|&b| b == b'\n')
                .map(|i| from + i + 1)
                .unwrap_or(from);
            let text = String::from_utf8_lossy(&data[piece_start..end]);
            if let Some(v) = want(text.lines().next().unwrap_or("")) {
                return Ok(Some((v, start + piece_start as u64)));
            }
            end = piece_start;
        }
        if start == 0 {
            return Ok(None);
        }
        data.truncate(from);
        head = data;
        pos = start;
    }
}

/// The last line of a text file for which `want` is true (`read_text(path)?.lines()`'s lines), read from the end (`rfind_line`).
pub fn last_line_where(path: &Path, want: impl Fn(&str) -> bool) -> io::Result<Option<String>> {
    Ok(rfind_line(path, |l| want(l).then(|| l.to_string()))?.map(|(l, _)| l))
}

/// The text of a file from byte `offset` to its end, decoded as `read_text` decodes it (`offset` is a line start).
pub fn read_text_from(path: &Path, offset: u64) -> io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = fs::File::open(path)?;
    f.seek(SeekFrom::Start(offset))?;
    let mut b = Vec::new();
    f.read_to_end(&mut b)?;
    Ok(String::from_utf8_lossy(&b).into_owned())
}

/// `os.path.isfile`
pub fn is_file(p: &Path) -> bool {
    fs::metadata(p).map(|m| m.is_file()).unwrap_or(false)
}

/// `os.path.isdir`
pub fn is_dir(p: &Path) -> bool {
    fs::metadata(p).map(|m| m.is_dir()).unwrap_or(false)
}

/// Names in a folder, sorted (the Python code relied on `os.listdir`'s order only where it then sorted; sorting here makes the rest deterministic).
pub fn list_names(p: &Path) -> Vec<String> {
    let mut v: Vec<String> = match fs::read_dir(p) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    };
    v.sort();
    v
}

/// `shutil.rmtree(path, ignore_errors=True)`
pub fn rmtree(p: &Path) {
    let _ = fs::remove_dir_all(p);
}

/// Write `data` to a temporary file beside `path` and rename it over `path` (a reader never sees half a file). The temporary file is this writer's own, `<path>.<pid>.<n>.tmp` (`n` counts the
/// calls of this process): two writers of one path (two threads, or the app and a chain) never write into, or rename away, each other's (review r3 §5.5). It is removed when the write fails.
pub fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let tmp = temp_beside(path);
    let r = fs::write(&tmp, data).and_then(|()| fs::rename(&tmp, path));
    if r.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    r
}

/// The temporary name `write_atomic` and `copy_atomic` write to: `<path>.<pid>.<n>.tmp`, this writer's own (`n` counts the calls of this process).
fn temp_beside(path: &Path) -> PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.{n}.tmp", std::process::id()));
    PathBuf::from(tmp)
}

/// `write_atomic` for a copy: `src` is copied to this writer's own temporary name beside `dst`, which is then renamed over `dst` (review r4 §4.1: a fixed `<dst>.tmp` was shared by
/// every writer). The temporary file is removed when the copy or the rename fails.
pub fn copy_atomic(src: &Path, dst: &Path) -> io::Result<()> {
    let tmp = temp_beside(dst);
    let r = fs::copy(src, &tmp).and_then(|_| fs::rename(&tmp, dst));
    if r.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    r
}

/// `dst` becomes a copy of `src` made of hard links (a plain copy where linking fails). Nothing downstream edits these files in place.
pub fn link_tree(src: &Path, dst: &Path) -> io::Result<()> {
    rmtree(dst);
    fs::create_dir_all(dst)?;
    for name in list_names(src) {
        let a = src.join(&name);
        if is_file(&a) {
            let b = dst.join(&name);
            if fs::hard_link(&a, &b).is_err() {
                fs::copy(&a, &b)?;
            }
        }
    }
    Ok(())
}

/// Total size of the files under `path` (unreadable entries count 0).
pub fn folder_bytes(path: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(e.path()),
                Ok(_) => total += entry_bytes(&e),
                Err(_) => {}
            }
        }
    }
    total
}

/// The size of a folder entry that is not a folder: a link is followed (its target's size), anything else is read from the entry itself (`DirEntry::metadata`: an `fstatat` relative to the
/// folder being read on Apple and Linux, not a `stat` that walks the whole path again from the root: 40,000 files of a shader cache, review r4 §6.3). The same number either way.
pub fn entry_bytes(e: &fs::DirEntry) -> u64 {
    let followed = match e.file_type() {
        Ok(t) if t.is_symlink() => fs::metadata(e.path()),
        _ => e.metadata(),
    };
    followed.map(|m| m.len()).unwrap_or(0)
}

/// `'%.2f GB' % (n / 2**30)`
pub fn gib_text(n: f64) -> String {
    format!("{:.2} GB", n / (1u64 << 30) as f64)
}

/// A modification time as seconds since the epoch (0.0 when unreadable), for "most recently written" sorts.
pub fn mtime_secs(p: &Path) -> f64 {
    fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review r4 §6.3: reading from the end finds what a whole read found: the last matching line and where it starts (CRLF, a damaged UTF-8 byte, a line longer than a block, no newline at
    /// the end, the file shorter than a block, a match only at the very start, empty lines).
    #[test]
    fn a_search_from_the_end_finds_what_a_whole_read_found() {
        let d = crate::testutil::TempDir::new("tail");
        let p = d.join("x.log");
        let whole_last = |p: &Path, want: &dyn Fn(&str) -> bool| -> Option<(String, u64)> {
            let b = fs::read(p).ok()?;
            let mut at = 0u64;
            let mut found = None;
            for piece in b.split_inclusive(|&c| c == b'\n') {
                let text = String::from_utf8_lossy(piece);
                let line = text.lines().next().unwrap_or("").to_string();
                if want(&line) {
                    found = Some((line, at));
                }
                at += piece.len() as u64;
            }
            found
        };
        let filler = |n: usize| -> String {
            (0..n)
                .map(|i| format!("line {i} with some words in it\n"))
                .collect()
        };
        let big = "B".repeat(3 * TAIL_BLOCK as usize + 7);
        let cases: Vec<Vec<u8>> = vec![
            b"".to_vec(),
            b"one".to_vec(),
            b"\n\n".to_vec(),
            b"[mem:a] x\n\n   \n".to_vec(),
            format!("[mem:first]\n{}", filler(20_000)).into_bytes(),
            format!("{}[mem:mid] y\r\n{}", filler(9_000), filler(9_000)).into_bytes(),
            [
                filler(5_000).as_bytes(),
                b"[mem:bad \xff\xfe byte]\n",
                filler(3_000).as_bytes(),
            ]
            .concat(),
            format!("{}[mem:long] {big}\n{}", filler(100), filler(10)).into_bytes(),
            format!("{}[mem:end-no-newline]", filler(4_000)).into_bytes(),
            format!("{}{big}", filler(10)).into_bytes(),
            format!("{big}[mem:in a long last line]\r").into_bytes(),
        ];
        let wants: [&dyn Fn(&str) -> bool; 4] = [
            &|l| l.contains("[mem:"),
            &|l| !l.trim().is_empty(),
            &|l| l.contains("never there"),
            &|l| l.is_empty(),
        ];
        for (i, c) in cases.iter().enumerate() {
            fs::write(&p, c).unwrap();
            for (j, w) in wants.iter().enumerate() {
                let got = rfind_line(&p, |l| w(l).then(|| l.to_string())).unwrap();
                assert_eq!(got, whole_last(&p, w), "case {i}, predicate {j}");
                assert_eq!(
                    last_line_where(&p, w).unwrap(),
                    whole_last(&p, w).map(|x| x.0),
                    "case {i}, predicate {j}"
                );
                // and the line read from its offset on is that line
                if let Some((line, at)) = got {
                    assert_eq!(
                        read_text_from(&p, at).unwrap().lines().next().unwrap_or(""),
                        line,
                        "case {i}"
                    );
                }
            }
            let whole = read_text(&p).unwrap();
            assert_eq!(
                last_line_where(&p, |_| true).unwrap(),
                whole.lines().last().map(String::from),
                "case {i}: the last line"
            );
        }
        assert!(rfind_line(&d.join("missing"), |_| Some(())).is_err());
    }

    /// Review r4 §6.3: `folder_bytes` reads a file's size from its folder entry now; the number is the one the full-path `stat` gave (a link followed for its size, never walked into, a
    /// dangling one 0).
    #[test]
    fn folder_bytes_is_what_a_stat_of_every_path_said() {
        fn by_path(path: &Path) -> u64 {
            let mut total = 0;
            let mut stack = vec![path.to_path_buf()];
            while let Some(d) = stack.pop() {
                let Ok(rd) = fs::read_dir(&d) else { continue };
                for e in rd.flatten() {
                    let p = e.path();
                    match e.file_type() {
                        Ok(t) if t.is_dir() => stack.push(p),
                        Ok(_) => total += fs::metadata(&p).map(|m| m.len()).unwrap_or(0),
                        Err(_) => {}
                    }
                }
            }
            total
        }
        let d = crate::testutil::TempDir::new("folder-bytes");
        for i in 0..50 {
            crate::testutil::write(d.join(format!("a/{}/f{i}", i % 4)), vec![b'x'; i * 31]);
        }
        crate::testutil::write(d.join("big"), vec![b'y'; 100_000]);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(d.join("big"), d.join("a/link-to-file")).unwrap();
            std::os::unix::fs::symlink(d.join("a/0"), d.join("a/link-to-dir")).unwrap();
            std::os::unix::fs::symlink(d.join("nowhere"), d.join("a/dangling")).unwrap();
        }
        assert_eq!(folder_bytes(d.path()), by_path(d.path()));
        assert_eq!(folder_bytes(&d.join("a")), by_path(&d.join("a")));
        assert!(
            folder_bytes(&d.join("a")) > 100_000,
            "the link to a file counts its target"
        );
        assert_eq!(folder_bytes(&d.join("none")), 0);
    }

    /// Review r4 §4.1: a copy is renamed into place from this writer's own temporary name; a fixed `<dst>.tmp` (here a folder, which no copy can write) is not used, and a failed copy leaves
    /// nothing behind.
    #[test]
    fn an_atomic_copy_does_not_use_a_fixed_temporary_name() {
        let d = crate::testutil::TempDir::new("copy-atomic");
        let src = d.join("icon-src.jpg");
        fs::write(&src, "JPEG").unwrap();
        let dst = d.join("icon.jpg");
        fs::create_dir_all(d.join("icon.jpg.tmp")).unwrap(); // what another writer's fixed temporary name would be
        copy_atomic(&src, &dst).unwrap();
        assert_eq!(fs::read_to_string(&dst).unwrap(), "JPEG");
        assert!(copy_atomic(&d.join("missing"), &dst).is_err());
        let mut names = list_names(d.path());
        names.sort();
        assert_eq!(
            names,
            ["icon-src.jpg", "icon.jpg", "icon.jpg.tmp"],
            "no temporary file left"
        );
    }

    #[test]
    fn iso_matches_python() {
        assert_eq!(utc_iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_iso(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(utc_iso(1_795_000_000), "2026-11-18T11:06:40Z");
    }

    #[test]
    fn int0_is_pythons() {
        assert_eq!(parse_int0("0x140000000"), Some(0x1_4000_0000));
        assert_eq!(parse_int0(" 0X10 "), Some(16));
        assert_eq!(parse_int0("123"), Some(123));
        assert_eq!(parse_int0("-5"), Some(-5));
        assert_eq!(parse_int0("0"), Some(0));
        assert_eq!(parse_int0("00"), Some(0));
        assert_eq!(
            parse_int0("010"),
            None,
            "a leading zero is an error in base 0"
        );
        assert_eq!(parse_int0("0x"), None);
        assert_eq!(parse_int0(""), None);
        assert_eq!(parse_int0("on"), None);
    }

    /// Review r3 §5.5: two writers of the same path each go through their own temporary file: neither renames the other's half-written one, and neither fails because the
    /// other one renamed theirs away.
    #[test]
    fn two_writers_of_one_path_never_share_a_temporary_file() {
        let d = crate::testutil::TempDir::new("util-atomic");
        let path = d.join("gameapp.stage");
        let contents: Vec<Vec<u8>> = (0..4u8).map(|i| vec![b'a' + i; 256 * 1024]).collect();
        std::thread::scope(|s| {
            for c in &contents {
                let path = &path;
                s.spawn(move || {
                    for _ in 0..50 {
                        write_atomic(path, c)
                            .expect("an atomic write never fails because of another writer");
                    }
                });
            }
        });
        let got = fs::read(&path).unwrap();
        assert!(
            contents.contains(&got),
            "the file is one writer's whole content"
        );
        let left: Vec<String> = list_names(d.path())
            .into_iter()
            .filter(|n| n != "gameapp.stage")
            .collect();
        assert!(left.is_empty(), "no temporary file left: {left:?}");
    }

    #[test]
    fn round_up_is_ceiling_to_multiple() {
        assert_eq!(round_up(0xc9400000, 0x8000000), 0xd0000000);
        assert_eq!(round_up(0x8000000, 0x8000000), 0x8000000);
        assert_eq!(round_up(1, 0x8000000), 0x8000000);
    }
}
