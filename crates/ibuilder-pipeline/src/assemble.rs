//! Stage 4: write the framework with no linker (4a, `assemble.py`'s job, behind `Assembler`), then sign it in process (4b, behind `Signer`; zsign on the phone).

use crate::adapters::{AssembleRequest, SignOutcome};
use crate::ctx::Ctx;
use crate::error::{PipelineError, Result};
use crate::markers::write_framework_stamp;
use crate::memory::{mem_line, release_memory};
use crate::rewrite::patched_dir;
use crate::stage::write_stage;
use crate::util::{is_dir, is_file, list_names, rmtree};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// zsign hashes the framework's Info.plist into the signature and writes the resource seal, so the framework dir needs one.
pub const INFO_PLIST: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>CFBundleExecutable</key><string>DmGameProbe</string><key>CFBundleIdentifier</key><string>dev.nxpocket.DmGameProbe</string><key>CFBundlePackageType</key><string>FMWK</string></dict></plist>\n";

/// `flat/`: `<name>.patched.bin` + `.fixups.tsv` per module, and the rewriter's other tables (`.gates.tsv` and the rest -- without them the host reports "gates 0" and aborts with "unknown gate").
/// Rebuilt fresh so no stale hard-link survives. Hard links, not copies: nothing writes these files in place (the boot reads them, a new chain removes flat/ first), and a copy kept a second full
/// set of the rewritten modules on the phone (a large main is over 100 MB). Returns the number of files linked.
pub fn make_flat(patched: &Path, flat: &Path, modules: &[String]) -> Result<usize> {
    rmtree(flat);
    fs::create_dir_all(flat)
        .map_err(|e| PipelineError::io(format!("create {}", flat.display()), e))?;
    let mut linked = 0;
    for m in modules {
        let d = patched.join(m);
        if !is_dir(&d) {
            return Err(PipelineError::missing(format!(
                "the rewrite wrote no {m} in {}",
                d.display()
            )));
        }
        for name in list_names(&d) {
            let to = flat.join(&name);
            if fs::hard_link(d.join(&name), &to).is_err() {
                fs::copy(d.join(&name), &to)
                    .map_err(|e| PipelineError::io(format!("copy {name} into flat/"), e))?;
            }
            linked += 1;
        }
    }
    let missing: Vec<String> = modules
        .iter()
        .flat_map(|m| [".patched.bin", ".fixups.tsv", ".gates.tsv"].map(|s| format!("{m}{s}")))
        .filter(|n| !is_file(&flat.join(n)))
        .collect();
    if !missing.is_empty() {
        return Err(PipelineError::missing(format!(
            "flat/ is missing {missing:?}"
        )));
    }
    Ok(linked)
}

/// What the signer's rc means. The shim's rc is the CAUSE, not zsign's verdict: 10 no embedded.mobileprovision, 11 unreadable, 12 no certificate in this app's Keychain, 13 the item carries no .p12,
/// 14 the .p12's passphrase does not fit the shim's buffer, 20 zsign itself refused (err then holds its message). Saying "zsign refused" for 12 named the wrong component for a whole device run.
pub fn sign_failure_reason(rc: i32, err: &str) -> String {
    match rc {
        10 => "this bundle has no embedded.mobileprovision (an unsigned build, or a simulator install)".into(),
        11 => "embedded.mobileprovision could not be read".into(),
        12 => "no signing certificate in THIS app's Keychain -- import your .p12 with \"Certificate (.p12)\" and run again".into(),
        13 => "the Keychain item holds no .p12".into(),
        14 => "the .p12's passphrase is longer than 511 bytes (UTF-8): the app cannot hand it to the signer whole -- re-import the certificate with a shorter passphrase".into(),
        _ => format!("zsign refused the framework: {}", if err.is_empty() { "(no message)" } else { err }),
    }
}

/// What the framework stamp says (`signature=`) of a framework signed AD HOC because this install had no signing identity: built for export only (Export as its own app; the
/// installer re-signs the IPA). The app shows such a build with Export IPA and no Play, and a boot refuses it (iOS would refuse to map it from Documents).
pub const EXPORT_ONLY_SIGNATURE: &str = "adhoc-export";

/// The signer's rc that means "no signing identity on this install", not "a broken one": 10 no embedded.mobileprovision, 12 no certificate in this app's Keychain, 13 the
/// item carries no .p12. Stage 4b then signs the framework ad hoc (`Signer::sign_adhoc`) instead of failing: the game can be exported (the installer re-signs every executable
/// of an IPA), and importing the .p12 later lets the app sign it again to play. 11 (unreadable), 14 (a passphrase that does not fit) and 20 (zsign refused) are a .p12 that is
/// there and could not be used: those still fail, so the player fixes the certificate instead of getting an export-only build without being told why.
pub fn adhoc_fallback(rc: i32) -> bool {
    matches!(rc, 10 | 12 | 13)
}

/// The log line of a sign outcome (`stage 4b sign rc=... mode=... identifier=...`), `what` is "sign" or "sign ad hoc".
fn outcome_line(what: &str, r: &SignOutcome) -> String {
    format!(
        "stage 4b {what} rc={} mode={} identifier={}{}",
        r.rc,
        r.mode,
        r.identifier,
        if r.err.is_empty() {
            String::new()
        } else {
            format!(": {}", r.err)
        }
    )
}

/// `do_assemble`: stage 4a assembles, 4b signs. Returns `(framework, flat)`.
pub fn do_assemble(ctx: &mut Ctx) -> Result<(PathBuf, PathBuf)> {
    let modules = ctx.cfg.modules();
    let patched = patched_dir(ctx);
    let host = crate::adapters::host_dylib(&ctx.cfg)?;
    let out = PathBuf::from(
        ctx.cfg
            .get("framework_out")
            .ok_or_else(|| PipelineError::missing("framework_out is not in the config"))?,
    );
    if let Some(p) = out.parent() {
        fs::create_dir_all(p)
            .map_err(|e| PipelineError::io(format!("create {}", p.display()), e))?;
    }
    ctx.log(&format!(
        "stage 4a assemble: host {} + {} -> {}",
        host.display(),
        patched.display(),
        out.display()
    ));
    write_stage(
        &ctx.docs,
        4,
        5,
        "assembling the framework (host + modules, no linker)",
        Some(&*ctx.log),
    );
    let flat = Path::new(ctx.cfg.get_or("extract_out", "")).join("flat");
    let linked = make_flat(&patched, &flat, &modules)?;
    ctx.log(&format!(
        "stage 4a flat/: {linked} files from {}",
        patched.display()
    ));
    mem_line("before-assemble", &*ctx.log, &*ctx.mem);
    let t0 = Instant::now();
    let asm = ctx.adapters.assembler.clone();
    let info = asm.assemble(
        &AssembleRequest {
            host: &host,
            out: &out,
            flat: &flat,
            modules: &modules,
        },
        &ctx.view(),
    )?;
    let size = fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    ctx.log(&format!(
        "stage 4a assemble ok: {size} bytes, {} load commands ({:.1}s)",
        info.ncmds
            .map(|n| n.to_string())
            .unwrap_or_else(|| "?".into()),
        t0.elapsed().as_secs_f64()
    ));
    mem_line("after-assemble", &*ctx.log, &*ctx.mem);

    // 4b: sign, IN-PROCESS. zsign hashes the framework's Info.plist into the signature and writes the resource seal, so the framework dir needs one.
    let framework_dir = out.parent().unwrap_or(Path::new("")).to_path_buf();
    let exe = out
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let plist = framework_dir.join("Info.plist");
    if !is_file(&plist) {
        fs::write(&plist, INFO_PLIST)
            .map_err(|e| PipelineError::io(format!("write {}", plist.display()), e))?;
        ctx.log(&format!(
            "stage 4b framework Info.plist written ({} bytes)",
            INFO_PLIST.len()
        ));
    }
    // 4b, not 5: the boot is step 5. Both read 5/5 once, which parked the UI's progress bar at the boot's mark through the signing.
    write_stage(
        &ctx.docs,
        4,
        5,
        "signing the framework in-process (zsign, no codesign)",
        Some(&*ctx.log),
    );
    ctx.log(&format!(
        "stage 4b sign (in-process zsign): {}/{exe}",
        framework_dir.display()
    ));
    let t0 = Instant::now();
    let signer = ctx.adapters.signer.clone();
    let mut r = signer.sign(&framework_dir, &exe, &ctx.view());
    ctx.log(&outcome_line("sign", &r));
    // No identity on this install (no .p12 imported): the framework is signed ad hoc instead of failing the build, and stamped "export only" (the app offers Export IPA, whose
    // installer re-signs it; importing the .p12 later offers "Sign again to play"). A signer with no ad hoc mode keeps the failure below. With an identity nothing here runs.
    if r.rc != 0
        && adhoc_fallback(r.rc)
        && let Some(a) = signer.sign_adhoc(&framework_dir, &exe, &ctx.view())
    {
        ctx.log(&format!(
            "stage 4b no signing identity here (rc={}: {}): the framework is signed AD HOC instead -- built for export only: Export as its own app works now (the installer re-signs the IPA); to play inside iBuilder, import your .p12 and sign it again",
            r.rc,
            sign_failure_reason(r.rc, &r.err)
        ));
        ctx.log(&outcome_line("sign ad hoc", &a));
        if a.rc == 0 {
            ctx.export_only = true;
            r = a;
        } else {
            let why = format!(
                "{}; the ad hoc signature (export only) failed too: {}",
                sign_failure_reason(r.rc, &r.err),
                sign_failure_reason(20, &a.err)
            );
            ctx.log(&format!("stage 4b sign FAILED (rc={}): {why}", r.rc));
            return Err(PipelineError::failed(format!(
                "stage 4b sign failed (rc={}): {why}",
                r.rc
            )));
        }
    }
    let dt = t0.elapsed().as_secs_f64();
    let size = fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    if r.rc != 0 {
        let why = sign_failure_reason(r.rc, &r.err);
        ctx.log(&format!("stage 4b sign FAILED (rc={}): {why}", r.rc));
        return Err(PipelineError::failed(format!(
            "stage 4b sign failed (rc={}): {why}",
            r.rc
        )));
    }
    ctx.log(&format!(
        "stage 4b signed {} in {dt:.1}s ({:.1} MB) -> {size} bytes",
        r.mode,
        size as f64 / 1048576.0
    ));
    // only now: a framework that was not signed is not one to boot again; nor one whose game has no shader library (review r4 §4.1: the app reads a stamped framework as "Ready")
    if let Some(why) = crate::chain::shader_library_failure(&ctx.status) {
        ctx.log(&format!("framework stamp: NOT written: stage 3 could not build the shader library ({why}), so this build is not one to play (the app shows the game as not built; a rebuild tries again)"));
    } else {
        write_framework_stamp(ctx);
    }
    release_memory("after-sign", &*ctx.log, &*ctx.mem);
    Ok((out, flat))
}
