//! Proves the C ABI: builds `libibuilder_ffi.a` with the stand-in stage workers, compiles `tests/c/test_abi.c` with clang against `include/ibuilder.h`, links the static library and
//! runs the program over a prepared Documents folder. Skipped (with a message) when clang is not installed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The cargo target folder: `CARGO_TARGET_DIR` (a relative one is the workspace root's, as cargo reads it: review r4 B7, the inner `cargo build` found it there and this test looked
/// for the library beside the crate), else the folder this test binary is in.
fn target_dir() -> PathBuf {
    if let Some(t) = std::env::var_os("CARGO_TARGET_DIR") {
        return resolve_target(Path::new(&t), &workspace_root());
    }
    // <target>/<profile>/deps/<test exe>
    let exe = std::env::current_exe().unwrap();
    exe.parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn resolve_target(t: &Path, workspace: &Path) -> PathBuf {
    if t.is_absolute() {
        t.to_path_buf()
    } else {
        workspace.join(t)
    }
}

#[test]
fn a_relative_target_dir_is_the_workspace_roots() {
    let ws = Path::new("/w/s");
    assert_eq!(
        resolve_target(Path::new("target/x"), ws),
        Path::new("/w/s/target/x")
    );
    assert_eq!(resolve_target(Path::new("/abs/t"), ws), Path::new("/abs/t"));
}

fn prepare_docs(docs: &Path) {
    for m in ["main", "subsdk0", "sdk"] {
        write(&docs.join("extract/ExeFS").join(m), &format!("NSO:{m}"));
    }
    write(&docs.join("extract/RomFS.img"), "rom");
    write(&docs.join("extract/control.nacp"), "nacp");
    write(&docs.join("extract/icon.jpg"), "JPEG");
    write(&docs.join("host-prebuilt.dylib"), "HOST");
    let d = docs.display();
    write(
        &docs.join("gameapp.config"),
        &format!(
            "extract_out={d}/extract\nexe_dir={d}/extract/ExeFS\nmodules=main subsdk0 sdk\npatched_dir={d}/extract/rewrite\nhost_dylib={d}/host-prebuilt.dylib\nframework_out={d}/DmGameProbe.framework/DmGameProbe\nprofile={d}/profile.tsv\nromfs={d}/extract/RomFS.img\nboot_seconds=0\napp_build=B1\nshader_chain=cc0f2129e1de\ngame=Grizzy.nsp\nshader_free_gib=0\nkeep_exefs=1\n"
        ),
    );
    // keep_exefs=1: every run of the test assembles (the first one dropped extract/ExeFS, and the later runs assembled nothing and returned 0, which is 5 now)
    write(&docs.join("gameapp.running"), "marker"); // the run's own marker, cleared by the chain
}

fn write(p: &Path, text: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

#[test]
fn the_c_program_drives_the_whole_chain_through_the_abi() {
    if Command::new("clang").arg("--version").output().is_err() {
        ibuilder_testkit::skip("clang is not installed");
        return;
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest.parent().unwrap().parent().unwrap();
    let target = target_dir();
    let tmp = target.join("ibuilder-c-abi-test");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();

    // 1. the static library, with the mock stage workers
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let build = Command::new(&cargo)
        .current_dir(workspace)
        .env("CARGO_TARGET_DIR", &target)
        .args([
            "build",
            "--release",
            "-p",
            "ibuilder-ffi",
            "--features",
            "mock-stages,early-init",
        ])
        .output()
        .expect("run cargo build");
    assert!(
        build.status.success(),
        "cargo build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let lib = target.join("release/libibuilder_ffi.a");
    assert!(lib.is_file(), "{} was not built", lib.display());

    // 2. the C program
    let exe = tmp.join("test_abi");
    let mut cc = Command::new("clang");
    cc.args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-I"])
        .arg(manifest.join("include"))
        .arg(manifest.join("tests/c/test_abi.c"))
        .arg(&lib)
        .arg("-o")
        .arg(&exe);
    if cfg!(target_vendor = "apple") {
        cc.args(["-framework", "CoreFoundation", "-lc", "-lm"]);
    } else {
        cc.args(["-lpthread", "-ldl", "-lm"]);
    }
    let out = cc.output().expect("run clang");
    assert!(
        out.status.success(),
        "clang failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    // 3. a Documents folder the mock chain can run on
    let docs = tmp.join("Documents");
    prepare_docs(&docs);
    let launch = tmp.join("LaunchDocuments");
    fs::create_dir_all(&launch).unwrap();

    // 4. run it
    let run = Command::new(&exe)
        .arg(&docs)
        .arg(&launch)
        .output()
        .expect("run test_abi");
    let text = String::from_utf8_lossy(&run.stdout);
    println!("{text}");
    eprintln!("{}", String::from_utf8_lossy(&run.stderr));
    assert!(
        run.status.success() && text.contains("0 failure(s)"),
        "the C test failed:\n{text}"
    );
    assert!(text.matches("PASS ").count() >= 40, "{text}");

    // 5. a panic under panic = "abort" (this is the release profile): the child aborts, the message is in the chain log
    let mut cc = Command::new("clang");
    let pexe = tmp.join("panic_abort");
    cc.args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-I"])
        .arg(manifest.join("include"))
        .arg(manifest.join("tests/c/panic_abort.c"))
        .arg(&lib)
        .arg("-o")
        .arg(&pexe);
    if cfg!(target_vendor = "apple") {
        cc.args(["-framework", "CoreFoundation", "-lc", "-lm"]);
    } else {
        cc.args(["-lpthread", "-ldl", "-lm"]);
    }
    let out = cc.output().expect("run clang");
    assert!(
        out.status.success(),
        "clang failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let pdocs = tmp.join("PanicDocuments");
    fs::create_dir_all(&pdocs).unwrap();
    let run = Command::new(&pexe)
        .arg(&pdocs)
        .output()
        .expect("run panic_abort");
    let ptext = String::from_utf8_lossy(&run.stdout).into_owned();
    println!("{ptext}{}", String::from_utf8_lossy(&run.stderr));
    assert!(
        run.status.success() && ptext.matches("PASS").count() == 2,
        "panic_abort failed:\n{ptext}"
    );

    // 6. the early initializer (a Mach-O __mod_init_func): with a boot request on disk it takes its decision before main() and says so in the chain log
    let home = tmp.join("Home");
    write(
        &home.join("Documents/gameapp.boot-only"),
        "boot the framework that is already built and signed\n",
    );
    let docs2 = tmp.join("Documents2");
    prepare_docs(&docs2);
    let run = Command::new(&exe)
        .arg(&docs2)
        .env("HOME", &home)
        .env("IBUILDER_EARLY_INIT_FORCE", "1")
        .output()
        .expect("run test_abi with early init");
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stdout)
    );
    let log = fs::read_to_string(home.join("Documents/gameapp.chain.log")).unwrap_or_default();
    println!(
        "--- {}/Documents/gameapp.chain.log (written by the initializer before main)\n{log}",
        home.display()
    );
    if cfg!(target_vendor = "apple") {
        assert!(
            log.contains("as-census at launch, before the .NET runtime"),
            "{log}"
        );
        assert!(
            log.contains("as-reserve: deferred"),
            "no ceiling here (a Mac): the block would follow the dlopen: {log}"
        );
    }

    // 7. the boot's time limit, end to end: a host that runs 5 s, boot_seconds=1: the library ends its own process with SIGTERM after logging the cause and consuming the request
    let bdocs = tmp.join("BootDocuments");
    write(
        &bdocs.join("fakehost.c"),
        "#include <stdio.h>\n#include <unistd.h>\nint dm_host_probe_boot(const char *a, const char *m, const char *p) { printf(\"[dm] >>> nnMain %s %s %s\\n\", a, m, p); fflush(stdout); sleep(5); printf(\"[dm] graceful exit done\\n\"); return 0; }\n",
    );
    let host = bdocs.join("fakehost.dylib");
    let out = Command::new("clang")
        .args(["-dynamiclib", "-o"])
        .arg(&host)
        .arg(bdocs.join("fakehost.c"))
        .output()
        .expect("clang host");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    fs::create_dir_all(bdocs.join("extract/flat")).unwrap();
    let bd = bdocs.display();
    write(
        &bdocs.join("gameapp.config"),
        &format!(
            "extract_out={bd}/extract\nframework_out={}\nprofile={bd}/profile.tsv\nromfs=\nmodules=main\nboot_seconds=1\n",
            host.display()
        ),
    );
    write(
        &bdocs.join("gameapp.boot-only"),
        "boot the framework that is already built and signed\n",
    );
    let bexe = tmp.join("boot_limit");
    let mut cc = Command::new("clang");
    cc.args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-I"])
        .arg(manifest.join("include"))
        .arg(manifest.join("tests/c/boot_limit.c"))
        .arg(&lib)
        .arg("-o")
        .arg(&bexe);
    if cfg!(target_vendor = "apple") {
        cc.args(["-framework", "CoreFoundation", "-lc", "-lm"]);
    } else {
        cc.args(["-lpthread", "-ldl", "-lm"]);
    }
    let out = cc.output().expect("run clang");
    assert!(
        out.status.success(),
        "clang failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bexe)
        .arg(&bdocs)
        .output()
        .expect("run boot_limit");
    let btext = String::from_utf8_lossy(&run.stdout).into_owned();
    println!("{btext}{}", String::from_utf8_lossy(&run.stderr));
    assert!(
        run.status.success() && btext.matches("PASS").count() == 5,
        "boot_limit failed:\n{btext}"
    );
}
