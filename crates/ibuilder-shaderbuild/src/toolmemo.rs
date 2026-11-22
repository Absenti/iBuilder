//! The second level of the translation cache: what the two native tools answered, keyed by what the tools are and exactly what they read (lever 12 of the 2026-11-20 lever list).
//!
//! **Why.** A shader's cache digest (`key.rs`) carries the fingerprint of the finishing code (`CODE_FP`: every source of this crate and of `ibuilder-shader`), so any change of the Rust
//! finishing, even one that changes no output, makes every shader a miss, and a miss runs the translator and SPIRV-Cross again: about 9 ms of native work a shader on the Mac, the whole
//! cost of a cold build (`docs/shaders/SHADERBUILD-RUST.md` 5). The tools do not depend on the finishing code at all: their answer is a function of the tool and of the bytes they read.
//! This memo keeps those answers, so a finishing change re-finishes every shader (the first level misses, as it must) without translating anything again.
//!
//! **Why it cannot serve a stale answer.** Nothing of the Rust code is in its keys, and nothing it stores depends on the Rust code:
//!
//! * translator, one entry per **job**: the memo format, the chain id (it hashes the translator's sources, `dmsh_chain.FILES`), the translator's own identity (`dmsh_xlate --source-sha`,
//!   the first 16 hex digits of the sha256 of its `Program.cs` + `.csproj`, which the app's in-process translator answers too), the job object as the translator parses it (its JSON,
//!   with the `code` path left out) and the bytes of its `code` file. The stored files are **every** file the translator wrote for the job: it runs into an empty private folder, and a
//!   file is the job's when its name is `<job key>.<anything>` (`.spv`, `.res.json`, a geometry job's `.reader.spv`, and whatever a later translator adds); a file that is no job's makes
//!   the call unrecordable.
//! * SPIRV-Cross, one entry per **call**: the memo format, the chain id, what `spirv-cross --version` prints (its commit line), the argument list with the input file's path replaced by
//!   its bytes and the `--output` path by a placeholder, and the stored answer is the output file (when `--output` names one), stdout and stderr.
//!
//! Only whole successes are recorded: a translator call whose exit code is not 0, a job with a `[RED]` line, a SPIRV-Cross call that failed. Any of them runs the real tool every time,
//! with its own messages. A tool that gives no identity (no 16-hex `--source-sha`, no `Git commit:` line) is not memoized at all. So a translator or SPIRV-Cross change (a new source sha,
//! a new commit line, a new chain) can never hit an old entry, and an input change (a new job, a new SPIR-V word) never either; the tests in `tests/tool_memo.rs` change each of them.
//!
//! **What is assumed** (as the first level and the Python's cache assume it): the tools are deterministic functions of those inputs (the byte-identical builds of the test suites show
//! it), the translator reads nothing but the job file and the jobs' `code` files and writes only under its output folder, and the chain id or the source sha changes whenever the
//! translator's code does (`dmsh_chain.FILES` lists the translator sources it covers).
//!
//! **Layout.** `<cas>/cas-<chain>.dmsb/t/<hh>/<digest>`, inside the chain's folder (another chain's memo goes with its folder; the pipeline's cache limit counts it). An entry:
//! `"DMSBTOL1"`, `u32 count`, `count` x (`u16 name_len`, `name`, `u64 data_len`), the data in that order. Written whole under a temporary name and renamed (as the first level's entries),
//! so a reader sees a whole entry or none; a damaged or truncated entry is a miss and is written again.
use crate::cache::write_atomic;
use crate::records::hex;
use ibuilder_shader::tools::{ShaderTools, ToolRun};
use serde_json::Value;
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const MAGIC: &[u8; 8] = b"DMSBTOL1";
/// part of every key: bump it when the key or the entry changes meaning
const FORMAT: &str = "ibuilder-shaderbuild tool memo 1";
/// the folder of the memo inside a chain's cache folder
pub const DIR: &str = "t";

/// Hits and misses (jobs for the translator, calls for SPIRV-Cross), and entries written
#[derive(Default, Debug)]
pub struct MemoStats {
    pub xlate_hits: AtomicU64,
    pub xlate_misses: AtomicU64,
    pub cross_hits: AtomicU64,
    pub cross_misses: AtomicU64,
    pub stored: AtomicU64,
    /// calls whose outputs could not be recorded (an unknown output file, a write that failed): the answer was the real tool's, only the memo lost it
    pub unrecorded: AtomicU64,
}

impl MemoStats {
    pub fn line(&self) -> String {
        let g = |a: &AtomicU64| a.load(Ordering::Relaxed);
        format!(
            "tool memo: translator jobs hit {}, translated {}; SPIRV-Cross calls hit {}, run {}; entries written {}{}",
            g(&self.xlate_hits),
            g(&self.xlate_misses),
            g(&self.cross_hits),
            g(&self.cross_misses),
            g(&self.stored),
            match g(&self.unrecorded) {
                0 => String::new(),
                n => format!(", calls not recorded {n}"),
            }
        )
    }
}

/// `dmsh_xlate --source-sha`: 16 lowercase hex digits, or no identity
pub fn xlate_identity(r: &ToolRun) -> Option<String> {
    let s = r.stdout.trim();
    (r.rc == 0
        && s.len() == 16
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
    .then(|| s.to_string())
}

/// `spirv-cross --version` (a usage error that prints the pin's `Git commit: <tag> Timestamp: <time>` line first, on stdout or stderr): that line, or no identity
pub fn cross_identity(r: &ToolRun) -> Option<String> {
    [&r.stdout, &r.stderr]
        .iter()
        .flat_map(|s| s.lines())
        .find(|l| l.starts_with("Git commit: ") && l.len() > "Git commit: ".len())
        .map(|l| l.trim_end().to_string())
}

/// The tools with the memo in front of them
pub struct MemoTools<'a> {
    inner: &'a dyn ShaderTools,
    /// `<cas>/cas-<chain>.dmsb/t`
    dir: PathBuf,
    chain: String,
    xlate_id: Option<String>,
    cross_id: Option<String>,
    pub stats: MemoStats,
    seq: AtomicU64,
}

fn part(h: &mut Sha256, tag: &str, v: &[u8]) {
    h.update(tag.as_bytes());
    h.update([0u8]);
    h.update((v.len() as u64).to_le_bytes());
    h.update(v);
}

/// an entry's bytes
fn encode(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&(files.len() as u32).to_le_bytes());
    for (n, d) in files {
        out.extend_from_slice(&(n.len() as u16).to_le_bytes());
        out.extend_from_slice(n.as_bytes());
        out.extend_from_slice(&(d.len() as u64).to_le_bytes());
    }
    for (_, d) in files {
        out.extend_from_slice(d);
    }
    out
}

/// an entry read back; `None`: absent, damaged, truncated or longer than it says
fn decode(b: &[u8]) -> Option<Vec<(String, Vec<u8>)>> {
    if b.len() < 12 || &b[..8] != MAGIC {
        return None;
    }
    let count = u32::from_le_bytes(b[8..12].try_into().ok()?) as usize;
    if count > 64 {
        return None;
    }
    let mut p = 12usize;
    let mut table = Vec::with_capacity(count);
    for _ in 0..count {
        let nl = u16::from_le_bytes(b.get(p..p + 2)?.try_into().ok()?) as usize;
        p += 2;
        let name = std::str::from_utf8(b.get(p..p.checked_add(nl)?)?)
            .ok()?
            .to_string();
        p += nl;
        let dl = u64::from_le_bytes(b.get(p..p + 8)?.try_into().ok()?);
        p += 8;
        table.push((name, usize::try_from(dl).ok()?));
    }
    let mut out = Vec::with_capacity(count);
    for (n, l) in table {
        let e = p.checked_add(l)?;
        out.push((n, b.get(p..e)?.to_vec()));
        p = e;
    }
    (p == b.len()).then_some(out)
}

/// a path argument of a tool: absolute, or with a folder in it (a bare word such as `450` is never read as a file)
fn is_path_arg(a: &str) -> bool {
    a.contains('/')
}

impl<'a> MemoTools<'a> {
    /// `cache_dir` is the chain's cache folder (`cache::Store::cache`); `xlate_id` / `cross_id` are the tools' identities (`xlate_identity`, `cross_identity`): `None` turns the memo off
    /// for that tool
    pub fn new(
        inner: &'a dyn ShaderTools,
        cache_dir: &Path,
        chain: &str,
        xlate_id: Option<String>,
        cross_id: Option<String>,
    ) -> MemoTools<'a> {
        MemoTools {
            inner,
            dir: cache_dir.join(DIR),
            chain: chain.to_string(),
            xlate_id,
            cross_id,
            stats: MemoStats::default(),
            seq: AtomicU64::new(0),
        }
    }

    pub fn translator_memoized(&self) -> bool {
        self.xlate_id.is_some()
    }
    pub fn spirv_cross_memoized(&self) -> bool {
        self.cross_id.is_some()
    }

    fn path_of(&self, digest: &str) -> PathBuf {
        self.dir.join(&digest[..2]).join(digest)
    }

    fn load(&self, digest: &str) -> Option<Vec<(String, Vec<u8>)>> {
        decode(&fs::read(self.path_of(digest)).ok()?)
    }

    fn store(&self, digest: &str, files: &[(String, Vec<u8>)]) {
        let p = self.path_of(digest);
        let ok = p.parent().is_some_and(|d| fs::create_dir_all(d).is_ok())
            && write_atomic(&p, &encode(files)).is_ok();
        let c = if ok {
            &self.stats.stored
        } else {
            &self.stats.unrecorded
        };
        c.fetch_add(1, Ordering::Relaxed);
    }

    fn digest_start(&self, tool: &str, id: &str) -> Sha256 {
        let mut h = Sha256::new();
        part(&mut h, "format", FORMAT.as_bytes());
        part(&mut h, "tool", tool.as_bytes());
        part(&mut h, "chain", self.chain.as_bytes());
        part(&mut h, "identity", id.as_bytes());
        h
    }

    /// a private, empty folder for one real translator call (inside the call's own output folder, so it goes with it)
    fn private_dir(&self, out: &Path) -> Option<PathBuf> {
        let n = self.seq.fetch_add(1, Ordering::Relaxed);
        let d = out.join(format!(".memo-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).ok()?;
        Some(d)
    }
}

/// one job of a translator call: its JSON text as the job file has it, its key, and its memo digest (`None`: not a job the memo can key)
struct MemoJob {
    raw: String,
    key: String,
    digest: Option<String>,
}

impl ShaderTools for MemoTools<'_> {
    fn xlate(&self, argv: &[String]) -> ToolRun {
        let Some(id) = &self.xlate_id else {
            return self.inner.xlate(argv);
        };
        if argv.len() != 2 {
            return self.inner.xlate(argv);
        }
        let out = Path::new(&argv[1]);
        let Ok(text) = fs::read_to_string(&argv[0]) else {
            return self.inner.xlate(argv);
        };
        let Ok(raws) = serde_json::from_str::<Vec<Box<RawValue>>>(&text) else {
            return self.inner.xlate(argv);
        };
        let mut jobs: Vec<MemoJob> = Vec::with_capacity(raws.len());
        for r in &raws {
            let mut digest = None;
            let mut key = String::new();
            if let Ok(Value::Object(mut o)) = serde_json::from_str::<Value>(r.get())
                && let Some(k) = o.get("key").and_then(|k| k.as_str()).map(str::to_string)
                && !k.is_empty()
                && !k.contains(['/', '.'])
                && let Some(code) = o
                    .remove("code")
                    .and_then(|c| c.as_str().map(str::to_string))
                && let Ok(bytes) = fs::read(&code)
            {
                let mut h = self.digest_start("xlate", id);
                part(&mut h, "job", Value::Object(o).to_string().as_bytes());
                part(&mut h, "code", &bytes);
                digest = Some(hex(&h.finalize()));
                key = k;
            }
            jobs.push(MemoJob {
                raw: r.get().to_string(),
                key,
                digest,
            });
        }
        // the hits: their files go into the output folder, as the translator would have written them
        let mut miss: Vec<&MemoJob> = Vec::new();
        for j in &jobs {
            let hit = j.digest.as_deref().and_then(|d| self.load(d));
            match hit {
                Some(files)
                    if fs::create_dir_all(out).is_ok()
                        && files.iter().all(|(suffix, b)| {
                            fs::write(out.join(format!("{}{suffix}", j.key)), b).is_ok()
                        }) =>
                {
                    self.stats.xlate_hits.fetch_add(1, Ordering::Relaxed);
                }
                _ => miss.push(j),
            }
        }
        if miss.is_empty() {
            return ToolRun::ok();
        }
        self.stats
            .xlate_misses
            .fetch_add(miss.len() as u64, Ordering::Relaxed);
        // the misses: the real translator, into an empty folder of its own, over the original job file when nothing hit and over the missed jobs' own JSON texts otherwise
        let Some(private) = self.private_dir(out) else {
            self.stats.unrecorded.fetch_add(1, Ordering::Relaxed);
            return self.inner.xlate(argv);
        };
        let job_file = if miss.len() == jobs.len() {
            argv[0].clone()
        } else {
            let p = private.with_extension("json");
            let body = format!(
                "[{}]",
                miss.iter()
                    .map(|j| j.raw.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            if fs::write(&p, body).is_err() {
                let _ = fs::remove_dir_all(&private);
                self.stats.unrecorded.fetch_add(1, Ordering::Relaxed);
                return self.inner.xlate(argv);
            }
            p.to_string_lossy().into_owned()
        };
        let r = self
            .inner
            .xlate(&[job_file.clone(), private.to_string_lossy().into_owned()]);
        if job_file != argv[0] {
            let _ = fs::remove_file(&job_file);
        }
        // what the translator wrote: every entry of its private folder goes where the caller asked for it (a rename, whatever the memo does with it); the regular files are read first
        let entries: Vec<(String, bool)> = match fs::read_dir(&private) {
            Ok(rd) => rd
                .flatten()
                .map(|e| {
                    (
                        e.file_name().to_string_lossy().into_owned(),
                        e.file_type().is_ok_and(|t| t.is_file()),
                    )
                })
                .collect(),
            Err(_) => {
                // the folder cannot be listed: its files cannot be handed over, so the call is made again the plain way
                let _ = fs::remove_dir_all(&private);
                self.stats.unrecorded.fetch_add(1, Ordering::Relaxed);
                return self.inner.xlate(argv);
            }
        };
        let mut unknown = false;
        let mut written: Vec<(String, Vec<u8>)> = Vec::new();
        for (name, is_file) in &entries {
            let from = private.join(name);
            match fs::read(&from) {
                Ok(b) if *is_file => written.push((name.clone(), b)),
                _ => unknown = true, // not a regular file: handed over, never recorded
            }
            if fs::rename(&from, out.join(name)).is_err() {
                // a rename across folders of one folder cannot fail but on a full drive: then the file is copied, or the call is the plain one again
                let copied = written
                    .last()
                    .filter(|w| &w.0 == name)
                    .is_some_and(|w| fs::write(out.join(name), &w.1).is_ok());
                if !copied {
                    let _ = fs::remove_dir_all(&private);
                    self.stats.unrecorded.fetch_add(1, Ordering::Relaxed);
                    return self.inner.xlate(argv);
                }
            }
        }
        let _ = fs::remove_dir_all(&private);
        // by job: `<job key>.<suffix>`; a file that is not exactly one job's makes the call unrecordable
        let mut by_job: HashMap<&str, Vec<(String, Vec<u8>)>> = HashMap::new();
        for (name, b) in &written {
            let owners: Vec<&MemoJob> = miss
                .iter()
                .copied()
                .filter(|j| {
                    !j.key.is_empty()
                        && name.len() > j.key.len()
                        && name.starts_with(&j.key)
                        && name.as_bytes()[j.key.len()] == b'.'
                })
                .collect();
            match owners.as_slice() {
                [j] => by_job
                    .entry(j.key.as_str())
                    .or_default()
                    .push((name[j.key.len()..].to_string(), b.clone())),
                _ => unknown = true,
            }
        }
        if r.rc == 0 && !unknown {
            let mut red: HashMap<String, String> = HashMap::new();
            crate::build::parse_xlate_errors(&r.stderr, &mut red);
            for j in &miss {
                let (Some(d), Some(files)) = (&j.digest, by_job.get(j.key.as_str())) else {
                    continue;
                };
                if red.contains_key(&j.key) {
                    continue;
                }
                let mut files = files.clone();
                files.sort();
                self.store(d, &files);
            }
        } else if r.rc == 0 {
            self.stats.unrecorded.fetch_add(1, Ordering::Relaxed);
        }
        r
    }

    fn spirv_cross(&self, argv: &[String]) -> ToolRun {
        let Some(id) = &self.cross_id else {
            return self.inner.spirv_cross(argv);
        };
        // the key: every argument as it is, but the input file (its bytes instead of its path) and the output path (a placeholder)
        let mut h = self.digest_start("spirv-cross", id);
        let (mut inputs, mut output): (usize, Option<&str>) = (0, None);
        let mut i = 0;
        while i < argv.len() {
            let a = argv[i].as_str();
            if a == "--output" {
                let Some(o) = argv.get(i + 1) else {
                    return self.inner.spirv_cross(argv);
                };
                if output.is_some() {
                    return self.inner.spirv_cross(argv);
                }
                part(&mut h, "arg", b"--output");
                part(&mut h, "output", b"");
                output = Some(o.as_str());
                i += 2;
                continue;
            }
            if is_path_arg(a) {
                let Ok(b) = fs::read(a) else {
                    return self.inner.spirv_cross(argv); // a path that is not a file the tool reads: not a call the memo knows
                };
                part(&mut h, "input", &b);
                inputs += 1;
            } else {
                part(&mut h, "arg", a.as_bytes());
            }
            i += 1;
        }
        if inputs != 1 {
            return self.inner.spirv_cross(argv); // `--version`, or a call with more than one file
        }
        let digest = hex(&h.finalize());
        if let Some(files) = self.load(&digest) {
            let get = |n: &str| files.iter().find(|f| f.0 == n).map(|f| f.1.as_slice());
            let wrote = match (output, get("output")) {
                (Some(o), Some(b)) => fs::write(o, b).is_ok(),
                (None, None) => true,
                _ => false,
            };
            if let (true, Some(so), Some(se)) = (wrote, get("stdout"), get("stderr")) {
                self.stats.cross_hits.fetch_add(1, Ordering::Relaxed);
                return ToolRun {
                    rc: 0,
                    stdout: String::from_utf8_lossy(so).into_owned(),
                    stderr: String::from_utf8_lossy(se).into_owned(),
                };
            }
        }
        self.stats.cross_misses.fetch_add(1, Ordering::Relaxed);
        let r = self.inner.spirv_cross(argv);
        if r.rc == 0 {
            let mut files = vec![
                ("stderr".to_string(), r.stderr.clone().into_bytes()),
                ("stdout".to_string(), r.stdout.clone().into_bytes()),
            ];
            match output.map(fs::read) {
                Some(Ok(b)) => files.push(("output".to_string(), b)),
                Some(Err(_)) => {
                    self.stats.unrecorded.fetch_add(1, Ordering::Relaxed);
                    return r;
                }
                None => {}
            }
            files.sort();
            self.store(&digest, &files);
        }
        r
    }
}

/// Stray temporary files of killed builds in the memo (older than an hour: a younger one may be another build's, being written right now)
pub fn sweep(cache_dir: &Path) {
    let Ok(rd) = fs::read_dir(cache_dir.join(DIR)) else {
        return;
    };
    for d in rd.flatten() {
        if let Ok(files) = fs::read_dir(d.path()) {
            for f in files.flatten() {
                let old = f
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|a| a.as_secs() >= 3600);
                if old && f.file_name().to_string_lossy().contains(".tmp-") {
                    let _ = fs::remove_file(f.path());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_reads_back_and_a_damaged_one_is_none() {
        let files = vec![
            (".res.json".to_string(), b"{}".to_vec()),
            (".spv".to_string(), vec![3u8; 100]),
        ];
        let b = encode(&files);
        assert_eq!(decode(&b).unwrap(), files);
        for n in 0..b.len() {
            assert!(decode(&b[..n]).is_none(), "truncated to {n}");
        }
        let mut longer = b.clone();
        longer.push(0);
        assert!(decode(&longer).is_none());
        let mut bad = b.clone();
        bad[0] = b'X';
        assert!(decode(&bad).is_none());
        let mut huge = b.clone();
        huge[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(&huge).is_none());
    }

    #[test]
    fn identities_are_taken_only_when_they_are_one() {
        let run = |rc, so: &str, se: &str| ToolRun {
            rc,
            stdout: so.into(),
            stderr: se.into(),
        };
        assert_eq!(
            xlate_identity(&run(0, "c75f3cb403920cde\n", "")).as_deref(),
            Some("c75f3cb403920cde")
        );
        assert!(xlate_identity(&run(1, "c75f3cb403920cde\n", "")).is_none());
        assert!(xlate_identity(&run(0, "Unhandled exception", "")).is_none());
        assert!(xlate_identity(&run(0, "C75F3CB403920CDE", "")).is_none());
        let help = "Git commit: vulkan-sdk-1.4.363.0 Timestamp: 2026-09-16T11:27:19\nUsage: spirv-cross [SPIR-V file] [options]\n";
        assert_eq!(
            cross_identity(&run(1, "", help)).as_deref(),
            Some("Git commit: vulkan-sdk-1.4.363.0 Timestamp: 2026-09-16T11:27:19")
        );
        assert_eq!(
            cross_identity(&run(0, help, "")),
            cross_identity(&run(1, "", help))
        );
        assert!(cross_identity(&run(1, "", "usage: no commit here")).is_none());
        assert!(cross_identity(&run(0, "Git commit: ", "")).is_none());
    }
}
