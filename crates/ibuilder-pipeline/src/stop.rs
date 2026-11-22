//! The stop request: a build is paused or cancelled by another thread (the app's UI) while the chain runs on a thread of its own.
//!
//! One process-wide byte, `0` none, `1` pause, `2` cancel. Every long loop of the chain (the stages, the modules of the rewrite, the shaders of stage 3, the waits of the memory and thermal
//! gates) reads it at its safe points and ends the work in hand, never starting a new unit. A **cancel overrides a pause, never the other way** (a build the player gave up on must not
//! turn back into one that can be resumed). The chain clears the byte when a run starts (`chain::run_chain`): a request is for the run that is going, and one that came before a run or
//! after the last one ended must not stop the next build. The reading costs one relaxed load, so a build that is never stopped does exactly what it did before.
//!
//! The two kinds end the run differently (`chain::RC_PAUSED` / `chain::RC_CANCELLED`); what is left on disk is the same: nothing is ever written under its final name until it is whole, so the
//! work that was finished (the shader cache, the rewrite cache, the split's jobs) stays and a later run continues from it.

use std::sync::atomic::{AtomicU8, Ordering};

/// no request
pub const NONE: u8 = 0;
/// stop at the next safe point and keep everything: the run ends with `RC_PAUSED` and can be continued
pub const PAUSE: u8 = 1;
/// stop at the next safe point exactly like a pause; the run ends with `RC_CANCELLED` (the caller then removes the partial build)
pub const CANCEL: u8 = 2;

static REQUEST: AtomicU8 = AtomicU8::new(NONE);

/// Raise a request: `PAUSE` (1) or `CANCEL` (2); any other value clears. A cancel replaces a pause, a pause never replaces a cancel. Returns the request in effect afterwards.
pub fn request(kind: u8) -> u8 {
    match kind {
        PAUSE => REQUEST.fetch_max(PAUSE, Ordering::SeqCst).max(PAUSE),
        CANCEL => REQUEST.fetch_max(CANCEL, Ordering::SeqCst).max(CANCEL),
        _ => {
            REQUEST.store(NONE, Ordering::SeqCst);
            NONE
        }
    }
}

/// The request in effect: `NONE`, `PAUSE` or `CANCEL`. One relaxed load: cheap enough for every shader.
pub fn requested() -> u8 {
    REQUEST.load(Ordering::Relaxed)
}

/// Withdraw whatever was asked (what a run does when it starts).
pub fn clear() {
    REQUEST.store(NONE, Ordering::SeqCst);
}

/// "pause", "cancel" or "none", for log lines.
pub fn name(kind: u8) -> &'static str {
    match kind {
        PAUSE => "pause",
        CANCEL => "cancel",
        _ => "none",
    }
}
