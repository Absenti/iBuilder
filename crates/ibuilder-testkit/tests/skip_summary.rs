//! `skip()` says so when the test binary ends (review r2, N9): libtest has no end-of-run hook and hides the stderr of a passing test, so a plain `cargo test` printed "ok" for tests that compared
//! nothing and nothing at the end. The summary is written to fd 2 by an `atexit` handler, which libtest's output capture does not cover.
//!
//! The child is this same binary, run for one helper test with `IBUILDER_TESTKIT_SUMMARY_CHILD=1` (without the variable the helper does nothing).

use std::process::Command;

#[test]
fn child_that_skips_twice_in_one_test() {
    if std::env::var_os("IBUILDER_TESTKIT_SUMMARY_CHILD").is_none() {
        return;
    }
    ibuilder_testkit::skip("the first fixture is not here");
    ibuilder_testkit::skip("the second fixture is not here");
}

#[test]
fn a_binary_whose_tests_skipped_says_how_many_when_it_ends() {
    let exe = std::env::current_exe().unwrap();
    let log = std::env::temp_dir().join(format!(
        "ibuilder-testkit-summary-{}.log",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&log);
    let run = |strict: bool| {
        let mut c = Command::new(&exe);
        c.args(["--exact", "child_that_skips_twice_in_one_test"])
            .env("IBUILDER_TESTKIT_SUMMARY_CHILD", "1")
            .env(ibuilder_testkit::SKIP_LOG_ENV, &log);
        if strict {
            c.env(ibuilder_testkit::STRICT_ENV, "1");
        } else {
            c.env_remove(ibuilder_testkit::STRICT_ENV);
        }
        c.output().unwrap()
    };
    // plain run: the test passes and the last thing the binary says is how many tests compared nothing
    let out = run(false);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(
        err.contains("1 test(s) skipped (2 skip call(s))"),
        "no summary at the end of the binary:\n{err}"
    );
    assert!(err.contains("IBUILDER_STRICT_FIXTURES=1"), "{err}");
    // strict run: the test fails, and the summary still tells why the binary failed
    let out = run(true);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "a strict skip must fail the child: {err}"
    );
    assert!(err.contains("skipped"), "{err}");
    // a binary that never skipped says nothing: the child with the helper filtered out
    let mut c = Command::new(&exe);
    let out = c
        .args(["--exact", "a_test_that_does_not_exist_so_nothing_runs"])
        .env(ibuilder_testkit::SKIP_LOG_ENV, &log)
        .output()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("skipped ("),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_file(&log);
}
