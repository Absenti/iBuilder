//! The shim's signer rc is the cause, and every rc it can return has its own sentence (not "zsign refused the framework" for a failure that happened before zsign ran).
use ibuilder_pipeline::assemble::sign_failure_reason;

#[test]
fn every_rc_the_shim_returns_names_its_cause() {
    for (rc, word) in [
        (10, "embedded.mobileprovision"),
        (11, "could not be read"),
        (12, "Keychain"),
        (13, "no .p12"),
        (14, "passphrase"),
    ] {
        let s = sign_failure_reason(rc, "ignored");
        assert!(
            s.contains(word) && !s.contains("zsign refused"),
            "rc {rc}: {s}"
        );
    }
    assert_eq!(
        sign_failure_reason(20, "bad things"),
        "zsign refused the framework: bad things"
    );
    assert_eq!(
        sign_failure_reason(20, ""),
        "zsign refused the framework: (no message)"
    );
}
