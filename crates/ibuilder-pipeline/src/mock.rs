//! Deterministic stand-in stage workers: the chain runs end to end with no game data and none of the real stage crates. Used by this crate's tests, by the C ABI test (feature
//! `mock-stages` of `ibuilder-ffi`) and by `ibuilder --mock`. Each mock does the smallest honest thing (writes the files the next stage reads, from the bytes it was given), so a
//! run exercises the cache, the markers, the stage lines and the timings exactly as a real run does.

use crate::adapters::*;
use crate::ctx::View;
use crate::error::{PipelineError, Result};
use crate::log::Log;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Derives a profile for a fixed title id (the game table lives in `Documents/profile-derived/<id>/game.tsv`).
pub struct MockProfile {
    pub title_id: String,
    pub modules: String,
    pub extra_rows: String,
}

impl MockProfile {
    pub fn new(title_id: &str, modules: &str) -> MockProfile {
        MockProfile {
            title_id: title_id.into(),
            modules: modules.into(),
            extra_rows: String::new(),
        }
    }
}

impl ProfileDeriver for MockProfile {
    fn derive(&self, v: &View<'_>) -> Result<PathBuf> {
        let dir = v.docs.derived_dir(&self.title_id);
        fs::create_dir_all(&dir)?;
        fs::write(
            dir.join("game.tsv"),
            format!(
                "title_id\t{}\nmodules\t{}\nmem_reserve\t0x140000000\nmem_alias\t0x118000000\nmem_stack\t0x10000000\n{}",
                self.title_id, self.modules, self.extra_rows
            ),
        )?;
        let out = v
            .cfg
            .get("profile")
            .map(PathBuf::from)
            .unwrap_or_else(|| v.docs.join("profile.tsv"));
        fs::write(
            &out,
            format!(
                "# mock profile\ntitle_id\t{}\nmem_reserve\t0x140000000\nmem_alias\t0x118000000\n",
                self.title_id
            ),
        )?;
        Ok(out)
    }
    fn render(&self, v: &View<'_>, tid: &str, profile: &Path) -> Result<()> {
        v.log.line(&format!("mock profile: rendered {tid} again"));
        fs::write(
            profile,
            format!(
                "# mock profile (rendered again)\ntitle_id\t{tid}\nmem_reserve\t0x140000000\nmem_alias\t0x118000000\n"
            ),
        )?;
        Ok(())
    }
}

/// "Rewrites" a module: the output is a pure function of the input bytes, so a cached run is byte-identical to a fresh one.
#[derive(Default)]
pub struct MockRewriter {
    pub calls: Mutex<Vec<(String, usize)>>,
}

impl Rewriter for MockRewriter {
    fn rewrite(&self, req: &RewriteRequest<'_>, _v: &View<'_>) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push((req.module.to_string(), req.jobs));
        let src = fs::read(req.src)?;
        let m = req.module;
        let mut out = b"PATCHED:".to_vec();
        out.extend_from_slice(&src);
        fs::write(req.out_dir.join(format!("{m}.patched.bin")), &out)?;
        fs::write(
            req.out_dir.join(format!("{m}.fixups.tsv")),
            format!("# RENDER_SCALE\t1\n# module\t{m}\n"),
        )?;
        fs::write(req.out_dir.join(format!("{m}.gates.tsv")), "gate\t0\n")?;
        Ok(())
    }
}

pub struct MockAssembler;
impl Assembler for MockAssembler {
    fn assemble(&self, req: &AssembleRequest<'_>, _v: &View<'_>) -> Result<AssembleInfo> {
        let mut h = Sha256::new();
        for m in req.modules {
            h.update(fs::read(req.flat.join(format!("{m}.patched.bin")))?);
        }
        let host = fs::read(req.host).unwrap_or_default();
        let mut fw = b"MOCK-FRAMEWORK\n".to_vec();
        fw.extend_from_slice(&host);
        fw.extend_from_slice(
            format!(
                "\nmodules-sha256 {}\n",
                h.finalize()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            )
            .as_bytes(),
        );
        fs::write(req.out, fw)?;
        Ok(AssembleInfo { ncmds: Some(7) })
    }
}

pub struct MockSigner {
    pub rc: i32,
}
impl Signer for MockSigner {
    fn sign(&self, framework_dir: &Path, exe: &str, _v: &View<'_>) -> SignOutcome {
        if self.rc == 0 {
            use std::io::Write;
            if let Ok(mut f) = fs::OpenOptions::new()
                .append(true)
                .open(framework_dir.join(exe))
            {
                let _ = f.write_all(b"MOCK-SIGNATURE\n");
            }
        }
        SignOutcome {
            rc: self.rc,
            mode: "mock-adhoc".into(),
            identifier: self.identifier(),
            err: if self.rc == 0 {
                String::new()
            } else {
                "mock refusal".into()
            },
        }
    }
}

/// A signer that finds no identity (`rc`, e.g. 12) and has an ad hoc mode (`adhoc_rc`): stage 4b's export-only fallback. `adhoc_calls` counts the ad hoc signatures.
#[derive(Default)]
pub struct MockAdhocSigner {
    pub rc: i32,
    pub adhoc_rc: i32,
    pub adhoc_calls: std::sync::atomic::AtomicUsize,
}
impl Signer for MockAdhocSigner {
    fn sign(&self, framework_dir: &Path, exe: &str, v: &View<'_>) -> SignOutcome {
        MockSigner { rc: self.rc }.sign(framework_dir, exe, v)
    }
    fn sign_adhoc(&self, framework_dir: &Path, exe: &str, _v: &View<'_>) -> Option<SignOutcome> {
        self.adhoc_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.adhoc_rc == 0 {
            use std::io::Write;
            if let Ok(mut f) = fs::OpenOptions::new()
                .append(true)
                .open(framework_dir.join(exe))
            {
                let _ = f.write_all(b"MOCK-ADHOC-SIGNATURE\n");
            }
        }
        Some(SignOutcome {
            rc: self.adhoc_rc,
            mode: "mock-adhoc(export only)".into(),
            identifier: self.identifier(),
            err: if self.adhoc_rc == 0 {
                String::new()
            } else {
                "mock ad hoc refusal".into()
            },
        })
    }
}

/// A shader backend whose "translation" is one tiny file per shader.
#[derive(Default)]
pub struct MockShaders {
    pub shaders: usize,
    pub splits: AtomicUsize,
    pub builds: AtomicUsize,
    pub fail_build: bool,
}

impl ShaderBackend for MockShaders {
    fn split_version(&self) -> String {
        "mocksplit001".into()
    }
    fn split(&self, req: &SplitRequest<'_>, log: &dyn Log) -> Result<SplitResult> {
        self.splits.fetch_add(1, Ordering::SeqCst);
        fs::create_dir_all(req.pkg_dir)?;
        fs::create_dir_all(req.tables_dir)?;
        fs::write(req.pkg_dir.join("ps_a.bin"), b"x")?;
        for t in [
            "shader_map.tsv",
            "shader_facts.tsv",
            "shader_pairs.tsv",
            "pso_pairs.tsv",
        ] {
            let rows = if t == "shader_map.tsv" {
                "main\t0\tvs\tabcdef0123456789\tx\n"
            } else {
                ""
            };
            fs::write(
                req.tables_dir.join(t),
                format!("package\tindex\tstage\tdm_ctl_sha16\tsource\n{rows}"),
            )?;
        }
        log.line(&format!("progress: {}/{} files", 1, 1));
        Ok(SplitResult {
            shaders: self.shaders,
            jobs_reused: 0,
            jobs_ran: 1,
        })
    }
    fn build(&self, req: &BuildRequest<'_>, progress: &mut dyn FnMut(&str)) -> Result<PathBuf> {
        self.builds.fetch_add(1, Ordering::SeqCst);
        if self.fail_build {
            return Err(PipelineError::failed("mock translation failure"));
        }
        let n = self.shaders;
        // the host's native tools are reachable from the worker, through the gate (the C ABI test passes C callbacks here)
        let x = (req.tools.xlate)(&[
            "dmsh_xlate".to_string(),
            "job.json".to_string(),
            "outdir".to_string(),
        ]);
        let c = (req.tools.spirv_cross)(&["spirv-cross".to_string(), "--msl".to_string()]);
        progress(&format!(
            "  tools: xlate rc={} out='{}' | spirv-cross rc={} out='{}'",
            x.rc, x.stdout, c.rc, c.stdout
        ));
        for i in (0..=n).step_by(2.max(n / 4)) {
            progress(&format!("  translating: {i}/{n} shaders"));
        }
        progress(&format!("  translating: {n}/{n} shaders"));
        fs::create_dir_all(req.out_dir.join("_work").join("profile").join(req.game_id))?;
        let img = req.out_dir.join(format!("b-{}.img", req.chain));
        fs::write(&img, b"MOCK-IMAGE")?;
        Ok(img)
    }
    fn live_translate(&self, req: &LiveRequest<'_>, _log: &dyn Log) -> Result<LiveResult> {
        let mut r = LiveResult::default();
        for e in req.entries {
            r.ok.insert(e.key.clone(), "vs".into());
            r.files.insert(format!("{}.metal", e.key), e.code.clone());
        }
        Ok(r)
    }
}

pub fn mock_tools() -> ShaderTools {
    let t: ToolFn = Arc::new(|argv: &[String]| ToolOutput {
        rc: 0,
        stdout: argv.join(" "),
        stderr: String::new(),
    });
    ShaderTools {
        xlate: t.clone(),
        spirv_cross: t,
    }
}

/// Records what it was asked to boot and returns `(0, 0)`.
#[derive(Default)]
pub struct MockBoot {
    pub requests: Mutex<Vec<String>>,
    pub stage: i32,
}
impl BootHost for MockBoot {
    fn boot(&self, req: &BootRequest<'_>, v: &View<'_>) -> BootResult {
        self.requests.lock().unwrap().push(format!(
            "{}|{}|{}|{}|{}",
            req.framework.display(),
            req.patched.display(),
            req.modules,
            req.profile.display(),
            req.seconds
        ));
        v.chain_log
            .line("[gameapp] mock boot: the guest ran and came back");
        BootResult {
            stage: self.stage,
            guest: if self.stage == 0 { 0 } else { -1 },
        }
    }
}

/// A settings generator that offers two rows (one of them the Metal 4 row the pipeline drops without pulling libraries).
pub struct MockLauncher;
impl LauncherBase for MockLauncher {
    fn base_document(
        &self,
        _v: &View<'_>,
        tid: &str,
        _main: &Path,
        _control: &Path,
        gfxapi: &str,
        _derived: Option<&Path>,
    ) -> Result<serde_json::Value> {
        let mut d = crate::settings::empty_document(tid);
        d["settings"] = serde_json::json!([
            {"env": "DM_FPS_TARGET", "kind": "choice", "section": "Game", "label": "Frame rate", "note": "", "default": "30", "options": [["30", "30 fps"], ["60", "60 fps"]]},
            {"env": "DM_PSO_MTL4", "kind": "toggle", "section": "Performance", "label": "Metal 4 pipelines", "note": "", "default": "1", "on": "1", "off": "0"},
            {"env": "DM_SHOW_FPS", "kind": "toggle", "section": "Game", "label": "Frame-rate counter", "note": "", "default": "0", "on": "1", "off": "0"}
        ]);
        let _ = gfxapi;
        Ok(d)
    }
}

/// The full set of mocks.
pub fn mock_adapters(title_id: &str) -> Adapters {
    Adapters {
        nup: None,
        profile: Arc::new(MockProfile::new(title_id, "main subsdk0 sdk")),
        rewriter: Arc::new(MockRewriter::default()),
        shaders: Some(Arc::new(MockShaders {
            shaders: 5,
            ..Default::default()
        })),
        tools: Some(mock_tools()),
        assembler: Arc::new(MockAssembler),
        signer: Arc::new(MockSigner { rc: 0 }),
        boot: Some(Arc::new(MockBoot::default())),
        launcher: Some(Arc::new(MockLauncher)),
        mods: None,
    }
}
