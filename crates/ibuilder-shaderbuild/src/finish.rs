//! The seam between the build driver and the per-shader `finish()` (`dmsh_build.py` l.737-875: spirv-cross, the MSL rewrite chain, reflection, the side libraries, the reflection JSON).
//!
//! Package F2 writes `ibuilder_shader::finish` (see `docs/shaders/SHADER-FINISH-RUST.md`); the driver codes against [`Finisher`], which has the shape the plan sketched for it, and is joined to
//! that module by `finish_adapter` (feature `f2-finish`). Until then three finishers exist, none of them a translator:
//!
//! * [`MockFinisher`]: runs spirv-cross exactly as `finish` does (same flags) and writes `<key>_b.metal` (spirv-cross's text) and a minimal `<key>_b.json`. It exists so the orchestration (jobs, translator
//!   files, cache, phases, library layout) is exercised without F2's code. **Its output is not a shader library.**
//! * [`ReplayFinisher`]: hands back, for each key, the files a Python build left in a library image (`<key>_b*`), so the driver's own work (digests, batches, pairs, cache, library, tables, stamp)
//!   is proven byte-identical to the Python's image *independently* of the finish port.
//! * `finish_adapter::RustFinisher` (feature `f2-finish`): the real thing.
pub use ibuilder_shader::finish::{Partners, SphReads};
use ibuilder_shader::romfs::Image;
use ibuilder_shader::tools::ShaderTools;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;

/// What `finish` returns (`(status, detail, refl)` in the Python, plus the files it left on disk, in their library names)
#[derive(Clone, Debug, Default)]
pub struct Finished {
    /// `"ok"`, or the Python's failure kind (`"MSL failed"`, `"translation failed"`, ...), or `"unported: <why>"`
    pub status: String,
    pub detail: String,
    /// the reflection dict (what the summary reads: `drv_reads`, `bindings`, ...)
    pub refl: Option<Value>,
    /// the shader's library files: name -> bytes (`<key>_b.metal`, `<key>_b.json`, `<key>_bx.metal`, ...). Empty unless `status == "ok"`.
    pub outputs: Vec<(String, Vec<u8>)>,
    /// the `user(locnN)` numbers of `<key>_b.metal` (a pixel shader): what a paired vertex shader's `vs_pad` needs, kept instead of the MSL
    pub ps_locs: BTreeSet<u32>,
    /// the `[RED]` lines the Python prints for what a shader lost
    pub log: Vec<String>,
}

/// `re.findall(r'\[\[user\(locn(\d+)\)', msl)`: the input locations a pixel shader's MSL declares
pub fn ps_locs_of(msl: &[u8]) -> BTreeSet<u32> {
    let mut out = BTreeSet::new();
    let pat = b"[[user(locn";
    let mut from = 0usize;
    while let Some(i) = memchr::memmem::find(&msl[from..], pat) {
        let mut j = from + i + pat.len();
        let start = j;
        while j < msl.len() && msl[j].is_ascii_digit() {
            j += 1;
        }
        if j > start
            && msl.get(j) == Some(&b')')
            && let Some(n) = std::str::from_utf8(&msl[start..j])
                .ok()
                .and_then(|t| t.parse::<u32>().ok())
        {
            out.insert(n);
        }
        from += i + 1;
    }
    out
}

impl Finished {
    pub fn failed(status: &str, detail: impl Into<String>) -> Finished {
        Finished {
            status: status.to_string(),
            detail: detail.into(),
            refl: None,
            outputs: Vec::new(),
            ps_locs: BTreeSet::new(),
            log: Vec::new(),
        }
    }
    pub fn is_ok(&self) -> bool {
        self.status == "ok"
    }
}

pub trait Finisher: Send + Sync {
    /// Identifies the finish code in the cache digest: it must change whenever any output of `finish` can. (The chain id covers the engine's tools; this covers the Rust code.)
    fn fingerprint(&self) -> String;

    /// `finish(key, w, j, c1)`: `work` holds `<key>.spv`, `<key>.res.json` (and `<key>.bin`); `job` is the job object `job_for` made, mutated as the Python does (the vertex shaders' integer-input
    /// pass); `c1` its constant segment; `tools` the translator and spirv-cross (for the second pass and the geometry shaders' re-translations); `paired_locs(ps key)` the `user(locnN)` numbers of a
    /// paired pixel shader's finished MSL (`vs_pad`), `None` when that shader was not built.
    fn finish(
        &self,
        key: &str,
        work: &Path,
        job: &mut Value,
        c1: &[u8],
        tools: &dyn ShaderTools,
        paired_locs: &dyn Fn(&str) -> Option<BTreeSet<u32>>,
    ) -> Finished;

    /// [`finish`](Finisher::finish) with the paired shaders given as [`Partners`] (a mesh shader's `partner_reads` needs the pixel shaders' SPH input maps as well as their
    /// finished MSL's locations). The default hands on the locations only: a finisher that can finish mesh shaders overrides it.
    fn finish_with(
        &self,
        key: &str,
        work: &Path,
        job: &mut Value,
        c1: &[u8],
        tools: &dyn ShaderTools,
        partners: &dyn Partners,
    ) -> Finished {
        self.finish(key, work, job, c1, tools, &|ps| partners.locs(ps))
    }
}

/// The spirv-cross flags of `dmsh_build.XC`
pub const XC: [&str; 6] = [
    "--msl",
    "--msl-version",
    "20300",
    "--msl-argument-buffers",
    "--msl-texture-buffer-native",
    "--msl-pad-fragment-output",
];

pub struct MockFinisher;

impl Finisher for MockFinisher {
    fn fingerprint(&self) -> String {
        "mock-finisher-1".into()
    }

    fn finish(
        &self,
        key: &str,
        work: &Path,
        job: &mut Value,
        c1: &[u8],
        tools: &dyn ShaderTools,
        _paired: &dyn Fn(&str) -> Option<BTreeSet<u32>>,
    ) -> Finished {
        let spv = work.join(format!("{key}.spv"));
        let msl = work.join(format!("{key}.metal"));
        let mut argv: Vec<String> = XC.iter().map(|s| s.to_string()).collect();
        argv.push(spv.to_string_lossy().into_owned());
        argv.push("--output".into());
        argv.push(msl.to_string_lossy().into_owned());
        let r = tools.spirv_cross(&argv);
        if r.rc != 0 {
            return Finished::failed(
                "MSL failed",
                r.stderr.trim().chars().take(160).collect::<String>(),
            );
        }
        let text = match std::fs::read(&msl) {
            Ok(t) => t,
            Err(e) => return Finished::failed("MSL failed", e.to_string()),
        };
        let refl = json!({"key": format!("{key}_b"), "stage": job["stage"], "c1": crate::records::hex(c1), "bindings": []});
        let json = crate::pyjson::dumps(&refl, &crate::pyjson::Opts::INDENT1).into_bytes();
        let ps_locs = ps_locs_of(&text);
        Finished {
            status: "ok".into(),
            detail: String::new(),
            refl: Some(refl),
            outputs: vec![
                (format!("{key}_b.json"), json),
                (format!("{key}_b.metal"), text),
            ],
            ps_locs,
            log: Vec::new(),
        }
    }
}

/// Replays the files of an existing library image (see the module doc). `keep` filters the names that are given back (`None`: every `<key>_*` file).
pub struct ReplayFinisher<'a> {
    pub image: Image<'a>,
}

impl<'a> ReplayFinisher<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self, String> {
        Ok(ReplayFinisher {
            image: Image::open(data)?,
        })
    }
}

impl Finisher for ReplayFinisher<'_> {
    fn fingerprint(&self) -> String {
        "replay-finisher-1".into()
    }

    fn finish(
        &self,
        key: &str,
        _work: &Path,
        _job: &mut Value,
        _c1: &[u8],
        _tools: &dyn ShaderTools,
        _paired: &dyn Fn(&str) -> Option<BTreeSet<u32>>,
    ) -> Finished {
        let prefix = format!("{key}_");
        // names are sorted: the key's files are contiguous
        let start = self
            .image
            .files
            .partition_point(|f| f.name.as_str() < prefix.as_str());
        let outputs: Vec<(String, Vec<u8>)> = self.image.files[start..]
            .iter()
            .take_while(|f| f.name.starts_with(&prefix))
            .map(|f| (f.name.clone(), self.image.bytes(f).to_vec()))
            .collect();
        if outputs.is_empty() {
            return Finished::failed("not in the replayed image", key.to_string());
        }
        let ps_locs = outputs
            .iter()
            .find(|o| o.0 == format!("{key}_b.metal"))
            .map(|o| ps_locs_of(&o.1))
            .unwrap_or_default();
        Finished {
            status: "ok".into(),
            detail: String::new(),
            refl: None,
            outputs,
            ps_locs,
            log: Vec::new(),
        }
    }
}
