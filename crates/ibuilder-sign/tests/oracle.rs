//! Oracle tests: the Python `pipeline/adhoc_sign.py` and Apple's `codesign` are the judges.
//! Every case signs a copy with the Python and a copy with this crate and requires **byte-identical** files, then
//! requires `codesign --verify --strict -vvv` to accept the Rust output. A missing oracle or fixture skips the case
//! with a message (it never fails). Set `IBUILDER_SIGN_BIG=1` to also run the 190 MB framework fixtures.
use ibuilder_sign::{sign_file, verify_file};
use ibuilder_testkit::{IOSBUILDER, SCRATCH_OLD as SCRATCH, compare_files};
use std::path::{Path, PathBuf};
use std::process::Command;

fn python() -> Option<PathBuf> {
    let p = std::env::var("IBUILDER_ORACLE_PYTHON")
        .map(PathBuf::from)
        .unwrap_or_else(|_| format!("{SCRATCH}/ref/venv/bin/python").into());
    p.exists().then_some(p)
}

fn adhoc_py() -> Option<PathBuf> {
    let a = std::env::var("IBUILDER_ORACLE_ADHOC_SIGN")
        .map(PathBuf::from)
        .unwrap_or_else(|_| format!("{SCRATCH}/oracle/pipeline/adhoc_sign.py").into());
    if a.exists() {
        return Some(a);
    }
    let b = PathBuf::from(format!("{IOSBUILDER}/pipeline/adhoc_sign.py"));
    b.exists().then_some(b)
}

fn work() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("sign-oracle");
    std::fs::create_dir_all(d.join("py")).unwrap();
    std::fs::create_dir_all(d.join("rs")).unwrap();
    d
}

fn have(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").output().is_ok()
        || Command::new("/usr/bin/which")
            .arg(cmd)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
}

/// Sign `input` both ways under the name `tag` and compare; returns false when skipped.
fn check(tag: &str, input: &Path, ident: Option<&str>) -> bool {
    let (Some(py), Some(script)) = (python(), adhoc_py()) else {
        ibuilder_testkit::skip(&format!(
            "{tag}: Python oracle (ref/venv + adhoc_sign.py) not found"
        ));
        return false;
    };
    if !input.exists() {
        ibuilder_testkit::skip(&format!("{tag}: fixture {} absent", input.display()));
        return false;
    }
    let w = work();
    let (a, b) = (w.join("py").join(tag), w.join("rs").join(tag));
    std::fs::copy(input, &a).unwrap();
    std::fs::copy(input, &b).unwrap();
    let mut cmd = Command::new(py);
    cmd.arg("-B")
        .arg(&script)
        .arg(&a)
        .env("PYTHONDONTWRITEBYTECODE", "1");
    if let Some(i) = ident {
        cmd.arg(i);
    }
    let out = cmd.output().expect("run python");
    assert!(
        out.status.success(),
        "{tag}: python failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let info = sign_file(&b, ident).unwrap_or_else(|e| panic!("{tag}: rust failed: {e}"));
    let v = verify_file(&b).unwrap();
    assert!(
        v.matches && v.pages_ok == v.n_code,
        "{tag}: own verify failed {v:?}"
    );
    compare_files(&a, &b).unwrap_or_else(|e| panic!("{tag}: NOT byte-identical: {e}"));
    // python's own report line must agree with ours on the numbers
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&format!("'code_limit': {}", info.code_limit))
            && stdout.contains(&format!("'n_code': {}", info.n_code)),
        "{tag}: {stdout}"
    );
    if have("codesign") {
        let c = Command::new("codesign")
            .args(["--verify", "--strict", "-vvv"])
            .arg(&b)
            .output()
            .unwrap();
        assert!(
            c.status.success(),
            "{tag}: codesign rejected the Rust output: {}",
            String::from_utf8_lossy(&c.stderr)
        );
        eprintln!(
            "OK   {tag}: byte-identical to Python ({} B), codesign --verify --strict ok",
            info.filelen
        );
    } else {
        eprintln!(
            "OK   {tag}: byte-identical to Python ({} B); codesign not available: not verified with codesign",
            info.filelen
        );
    }
    true
}

fn clang_dylib(name: &str, extra: &[&str]) -> Option<PathBuf> {
    if !have("clang") {
        ibuilder_testkit::skip(&format!("{name}: clang not available"));
        return None;
    }
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("sign-oracle-src");
    std::fs::create_dir_all(&dir).unwrap();
    let c = dir.join("t.c");
    std::fs::write(&c, "#include <stdio.h>\nconst char dmgame_image_marker[] = \"DmGame\";\nint add(int a, int b) { return a + b; }\nvoid hello(void) { puts(\"hello\"); }\n").unwrap();
    let out = dir.join(name);
    let st = Command::new("clang")
        .args([
            "-arch",
            "arm64",
            "-dynamiclib",
            "-O1",
            "-install_name",
            "@rpath/Test.framework/Test",
        ])
        .args(extra)
        .arg("-o")
        .arg(&out)
        .arg(&c)
        .status()
        .ok()?;
    st.success().then_some(out)
}

#[test]
fn clang_unsigned_dylib_with_headerpad() {
    if let Some(p) = clang_dylib(
        "unsigned.dylib",
        &["-Wl,-headerpad,0x8000", "-Wl,-no_adhoc_codesign"],
    ) {
        check("clang-unsigned.dylib", &p, None);
        check("clang-unsigned-ident.dylib", &p, Some("dev.ibuilder.app"));
    }
}

#[test]
fn clang_linker_signed_dylib_is_resigned() {
    // ld's own linker-signed signature (flags 0x20002) is dropped and replaced by the ad-hoc one
    if let Some(p) = clang_dylib("ldsigned.dylib", &[]) {
        check("clang-ldsigned.dylib", &p, None);
    }
}

#[test]
fn iosbuilder_small_fixtures() {
    for (tag, rel) in [
        ("unsigned-probe.dylib", "out-sign/unsigned-probe.dylib"),
        (
            "host-sim.dylib",
            "out-sign/host-prebuilt-loadable-sim.dylib",
        ),
        (
            "host-ios.dylib",
            "out-load/host-prebuilt-loadable-ios.dylib",
        ),
    ] {
        check(tag, Path::new(&format!("{IOSBUILDER}/{rel}")), None);
    }
}

#[test]
fn iosbuilder_assembled_framework() {
    // The assembled game framework (190 MB): carries the host's CodeDirectory, which is dropped and replaced.
    check(
        "DmGameProbe-sign",
        Path::new(&format!(
            "{IOSBUILDER}/out-sign/DmGameProbe.framework/DmGameProbe"
        )),
        None,
    );
    if std::env::var("IBUILDER_SIGN_BIG").is_ok() {
        check(
            "DmGameProbe-asm",
            Path::new(&format!(
                "{IOSBUILDER}/out-asm/DmGameProbe.framework/DmGameProbe"
            )),
            Some("dev.ibuilder.app"),
        );
    }
}
