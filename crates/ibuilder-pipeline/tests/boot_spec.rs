//! Stage 5 for real: `sys::boot::DlopenBoot` dlopens a host built by clang in this test (a stand-in for the engine's host-prebuilt.dylib that exports the same entry points) and calls its
//! `dm_host_probe_boot`. Checks what the shim's `boot_framework` did: the guest's fd 1/2 land in `gameapp.guest.log`, the `[gameapp]` lines of the chain log, the environment the host reads
//! (`PROBE_MODE`, `DM_PROFILE`, `DM_ROMFS`, `DM_SAVE` from `set_game_env`), the CAMetalLayer handed to `dm_front_set_layer`, the live-shader callback (`nvn_live_set`) answered by the live
//! thread, the boot request consumed when the boot comes back, and the boot's time limit standing down. One test function: it changes the real process environment.
//! Skipped when clang is not installed.

mod common;

use ibuilder_pipeline::boot::boot_only;
use ibuilder_pipeline::chain::load_config;
use ibuilder_pipeline::ctx::Ctx;
use ibuilder_pipeline::env::{Env, ProcessEnv};
use ibuilder_pipeline::log::MemLog;
use ibuilder_pipeline::mock::{MockShaders, mock_tools};
use ibuilder_pipeline::sys::boot::{DlopenBoot, set_game_layer};
use ibuilder_pipeline::testutil::{TempDir, write};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

const HOST_C: &str = r#"
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
typedef int (*live_handler_t)(const char *, const void *, unsigned, const void *, unsigned, char *, int);
static live_handler_t g_handler;
static void *g_layer;
static int g_ready;
static char g_prof_at_layer[512];
/* the host's first profile read can come from here (between the dlopen and the boot): the variable must already be set */
void dm_front_set_layer(void *l) { g_layer = l; const char *p = getenv("DM_PROFILE"); snprintf(g_prof_at_layer, sizeof g_prof_at_layer, "%s", p ? p : "(unset)"); }
void nvn_live_set(live_handler_t h) { g_handler = h; }
void nvn_live_ready(void) { __atomic_fetch_add(&g_ready, 1, __ATOMIC_SEQ_CST); }
int dm_host_probe_boot(const char *patched, const char *modules, const char *profile) {
    printf("[dm] >>> nnMain patched=%s modules=%s profile=%s\n", patched, modules, profile);
    printf("[dm] env PROBE_MODE=%s DM_PROFILE=%s DM_ROMFS=%s DM_SAVE=%s\n", getenv("PROBE_MODE"), getenv("DM_PROFILE"), getenv("DM_ROMFS"), getenv("DM_SAVE"));
    printf("[dm] layer=%p\n", g_layer);
    printf("[dm] profile seen when the layer was handed over: %s\n", g_prof_at_layer);
    if (g_handler) {
        char stage[24] = {0};
        int first = g_handler("0123456789abcdef", "ctl", 3, "code", 4, stage, (int) sizeof stage);
        int again = g_handler("0123456789abcdef", "ctl", 3, "code", 4, stage, (int) sizeof stage);
        /* LiveQueue::done wakes the asking thread before it calls ready() on the live thread (live.rs): the call comes, but may come after the answer; wait for it (at most 5 s) */
        for (int i = 0; i < 5000 && __atomic_load_n(&g_ready, __ATOMIC_SEQ_CST) == 0; i++) usleep(1000);
        printf("[dm] live first=%d again=%d stage=%s ready_calls=%d\n", first, again, stage, __atomic_load_n(&g_ready, __ATOMIC_SEQ_CST));
    } else {
        printf("[dm] live: no handler\n");
    }
    printf("[dm] graceful exit done\n");
    fflush(stdout);
    return 7;
}
"#;

#[test]
fn dlopen_boot_runs_the_guest_in_this_process() {
    if Command::new("clang").arg("--version").output().is_err() {
        ibuilder_testkit::skip("clang is not installed");
        return;
    }
    let d = TempDir::new("boot-dlopen");
    let docs = d.path().to_path_buf();
    write(docs.join("fakehost.c"), HOST_C);
    let host = docs.join("fakehost.dylib");
    let cc = Command::new("clang")
        .args(["-dynamiclib", "-o"])
        .arg(&host)
        .arg(docs.join("fakehost.c"))
        .output()
        .unwrap();
    assert!(
        cc.status.success(),
        "{}",
        String::from_utf8_lossy(&cc.stderr)
    );

    // the game: a profile, its derived table, a shader pack (so live shaders have a library to complete), the flat/ the framework was built from
    let tid = "0100DFA027504000";
    write(docs.join("profile.tsv"), format!("title_id\t{tid}\n"));
    write(
        docs.join(format!("profile-derived/{tid}/game.tsv")),
        format!("title_id\t{tid}\nmodules\tmain\n"),
    );
    write(
        docs.join(format!("shaders/{tid}/shader_map.tsv")),
        "package\tindex\tstage\tdm_ctl_sha16\tsource\nmain\t0\tvs\tabcdef0123456789\tx\n",
    );
    // and its library: a pack with a map and no library is a game with no shader library, whose boot is refused (review r4 §4.1); this fixture booted one
    write(
        docs.join(format!("shaders/{tid}/b-cc0f2129e1de.img")),
        "LIBRARY",
    );
    std::fs::create_dir_all(docs.join("extract/flat")).unwrap();
    write(
        docs.join("gameapp.boot-only"),
        "boot the framework that is already built and signed\n",
    );
    write(docs.join("gameapp.running"), "marker");

    let env = Arc::new(ProcessEnv::new());
    let log = MemLog::new();
    let chain_log = MemLog::new();
    let mut ctx: Ctx = Ctx::builder(&docs)
        .env(env.clone())
        .log(log.clone())
        .chain_log(chain_log.clone())
        .build();
    ctx.adapters.boot = Some(Arc::new(DlopenBoot::new(ctx.live.clone())));
    ctx.adapters.shaders = Some(Arc::new(MockShaders {
        shaders: 1,
        ..Default::default()
    }));
    ctx.adapters.tools = Some(mock_tools());
    ctx.cfg = ibuilder_pipeline::config::Config::parse(&format!(
        "extract_out={0}/extract\nframework_out={1}\nprofile={0}/profile.tsv\nromfs={0}/extract/RomFS.img\nmodules=main\nshader_chain=cc0f2129e1de\nboot_seconds=2\n",
        docs.display(),
        host.display()
    ));
    let _ = load_config; // the config is set directly
    set_game_layer(0x1234 as *mut std::ffi::c_void);

    let rc = boot_only(&mut ctx);
    assert_eq!(rc, 0, "{:?}", log.lines());

    // the boot ran in this process and came back with the guest's own return value
    assert!(
        log.contains("stage 5 boot-only returned 0, the guest returned 7"),
        "{:?}",
        log.lines()
    );
    let c = chain_log.lines().join("\n");
    for want in [
        "stage 5 boot: dlopen",
        "dlopen OK",
        "the game layer 0x1234 was handed to the host",
        "dm_host_probe_boot(patched=",
        "dm_host_probe_boot returned 7",
        "translated while the game runs",
    ] {
        assert!(c.contains(want), "chain log lacks {want:?}:\n{c}");
    }
    // the guest's fd 1/2 went to Documents/gameapp.guest.log, not the console
    let guest = std::fs::read_to_string(docs.join("gameapp.guest.log")).unwrap();
    assert!(
        guest.contains("==== guest output, pid ")
            && guest.contains("[dm] >>> nnMain patched=")
            && guest.trim_end().ends_with("[dm] graceful exit done"),
        "{guest}"
    );
    // what the host read from the environment
    let env_line = guest.lines().find(|l| l.starts_with("[dm] env ")).unwrap();
    assert!(
        env_line.contains("PROBE_MODE=boot")
            && env_line.contains(&format!("DM_PROFILE={}/profile.tsv", docs.display())),
        "{env_line}"
    );
    assert!(
        env_line.contains(&format!("DM_SAVE={}/games/{tid}/save", docs.display())),
        "set_game_env's folder reached the host: {env_line}"
    );
    assert!(guest.contains("[dm] layer=0x1234"), "{guest}");
    assert!(
        guest.contains(&format!(
            "[dm] profile seen when the layer was handed over: {}/profile.tsv",
            docs.display()
        )),
        "DM_PROFILE must be set before the host is loaded: {guest}"
    );
    // the live shader was queued by the host's callback, translated by the live thread, answered (1 = by this ask, 2 = by an earlier one), and kept for the next starts
    assert!(
        guest.contains("[dm] live first=1 again=2 stage=vs ready_calls=1"),
        "{guest}"
    );
    assert!(
        docs.join(format!(
            "games/{tid}/learned_shaders/b-cc0f2129e1de/learned_map.tsv"
        ))
        .is_file()
    );
    assert!(
        std::fs::read_to_string(docs.join(format!(
            "games/{tid}/learned_shaders/b-cc0f2129e1de/learned_map.tsv"
        )))
        .unwrap()
        .contains("0123456789abcdef")
    );
    assert!(
        docs.join(format!(
            "games/{tid}/learned_shaders/raw/0123456789abcdef.code"
        ))
        .is_file()
    );
    assert!(
        log.contains("live shaders: this session translated 1 shader(s)"),
        "{:?}",
        log.lines()
    );
    // the request was honoured: consumed; the marker cleared
    assert!(!docs.join("gameapp.boot-only").exists() && !docs.join("gameapp.running").exists());
    assert!(log.contains("Documents/gameapp.boot-only consumed: the boot came back"));
    // the time limit of 2 s stood down because the boot returned first: no SIGTERM (this process is still here to see it)
    std::thread::sleep(Duration::from_millis(1600));
    assert!(
        chain_log
            .lines()
            .iter()
            .any(|l| l.contains("boot time limit of 2 s disarmed at 1 s")),
        "{:?}",
        chain_log.lines()
    );
    assert!(
        !chain_log
            .lines()
            .iter()
            .any(|l| l.contains("sending SIGTERM"))
    );
    // the real process environment was touched only by the commit
    assert_eq!(std::env::var("PROBE_MODE").as_deref(), Ok("boot"));
    env.remove("DM_SAVE");
}
