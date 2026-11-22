//! Numbers that come from `gameapp.config` (or `--set k=v`) are the player's, not ours: a hand-edited value must not wrap, divide by zero or ask for `usize::MAX` threads
//! (review finding 4.4). The same arithmetic in `shaders.rs` is listed as deferred in docs/REVIEW-TRIAGE-2026-11-19.md.

use ibuilder_pipeline::log::NullLog;
use ibuilder_pipeline::memory::{FakeMem, shader_jobs_for_memory};
use ibuilder_pipeline::util::{MIB, mib_bytes};

/// `per_job` is a divisor: 0 must not divide by zero (a panic, an abort in a release build)
#[test]
fn a_zero_per_job_is_one_not_a_division_by_zero() {
    let mem = FakeMem::new(4000 * MIB);
    assert_eq!(
        shader_jobs_for_memory(6, &mem, &NullLog, 0, "translation"),
        6, // all six jobs: the 2999 MB budget is far above need
        "per_job 0 is read as 1 byte: every job fits"
    );
    let mem = FakeMem::new(1024 * MIB);
    assert_eq!(
        shader_jobs_for_memory(6, &mem, &NullLog, 0, "translation"),
        1,
        "nothing above the 1 GiB kept free: one job, never zero"
    );
    // the ordinary case is as it was
    let mem = FakeMem::new(2400 * MIB);
    assert_eq!(
        shader_jobs_for_memory(6, &mem, &NullLog, 450 * MIB, "translation"),
        3
    );
}

/// a MiB count from the config as bytes: negative is 0, and a huge one saturates instead of wrapping to a small number (`i64::MAX as u64 * MIB` wrapped)
#[test]
fn mib_values_from_the_config_saturate() {
    assert_eq!(mib_bytes(2400), 2400 * MIB);
    assert_eq!(mib_bytes(0), 0);
    assert_eq!(mib_bytes(-5), 0);
    assert_eq!(mib_bytes(i64::MAX), u64::MAX);
    assert_eq!(mib_bytes((u64::MAX / MIB) as i64), (u64::MAX / MIB) * MIB);
    assert_eq!(mib_bytes((u64::MAX / MIB) as i64 + 1), u64::MAX);
    assert!(
        mib_bytes(i64::MAX) > mib_bytes(2400),
        "the larger request stays the larger one"
    );
}
