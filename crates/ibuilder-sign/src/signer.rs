//! The seam for swapping the signer.
//!
//! The toolchain signs a framework twice in different ways (see `docs/crates/ibuilder-sign.md`):
//! * **ad-hoc** (simulator, tests, `codesign -s -` equivalent): [`AdhocSigner`], this crate, pure Rust;
//! * **device identity** (an iPhone): the user's `.p12` + the app's `embedded.mobileprovision`, signed by zsign
//!   (C++/OpenSSL) through `nxpocket_zsign_sign_memory` in `sign/inapp_shim.cpp`. Out of scope to port; an app
//!   plugs it in with [`CallbackSigner`] (or its own `impl CodeSigner`) without this crate depending on zsign.
//!
//! Both take a framework folder + executable + identifier, which is what `gameapp_shim.c` `py_sign(framework, exe)`
//! hands to either `nxpocket_sign_adhoc` or `device_identity_sign`.
use crate::{Error, SignInfo, SignOptions, sign_file_with};
use std::path::Path;

/// What to sign.
#[derive(Clone, Copy, Debug)]
pub struct SignTarget<'a> {
    /// The `.framework` folder (zsign hashes its `Info.plist` and writes `_CodeSignature/CodeResources` there;
    /// the ad-hoc signer ignores it). `None` for a bare dylib.
    pub framework: Option<&'a Path>,
    /// The Mach-O to sign, in place (the framework's `CFBundleExecutable`).
    pub executable: &'a Path,
    /// The CodeDirectory identifier: the running app's own (iOS accepts an image outside the bundle only with the
    /// loading app's identifier). `None` keeps the dylib's own `LC_ID_DYLIB` name.
    pub identifier: Option<&'a str>,
}

/// The outcome, shaped like what `py_sign` returns `(rc, mode, identifier, error)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignReport {
    /// e.g. `"adhoc(ibuilder-sign, no certificate)"`, `"device(zsign, in-memory .p12)"`.
    pub mode: String,
    pub identifier: String,
    /// Present when the signer can say (the ad-hoc one does).
    pub info: Option<SignInfo>,
}

pub trait CodeSigner {
    fn mode(&self) -> &str;
    fn sign(&self, target: &SignTarget<'_>) -> Result<SignReport, Error>;
}

/// The pure-Rust ad-hoc signer.
#[derive(Clone, Copy, Debug, Default)]
pub struct AdhocSigner {
    pub options: SignOptions,
}

impl CodeSigner for AdhocSigner {
    fn mode(&self) -> &str {
        "adhoc(ibuilder-sign, no certificate)"
    }
    fn sign(&self, target: &SignTarget<'_>) -> Result<SignReport, Error> {
        let info = sign_file_with(target.executable, target.identifier, &self.options)?;
        Ok(SignReport {
            mode: self.mode().to_string(),
            identifier: info.identifier.clone(),
            info: Some(info),
        })
    }
}

/// Wrap any function as a signer: the hook for the zsign FFI in the app, or for a test double.
///
/// ```
/// use ibuilder_sign::{CallbackSigner, CodeSigner, SignReport, SignTarget};
/// let s = CallbackSigner::new("device(zsign, in-memory .p12)", |t: &SignTarget<'_>| {
///     // here: call nxpocket_zsign_sign_memory(p12, pass, prov, ents, framework, exe, identifier, err)
///     Ok(SignReport { mode: "device".into(), identifier: t.identifier.unwrap_or("").into(), info: None })
/// });
/// assert_eq!(s.mode(), "device(zsign, in-memory .p12)");
/// ```
pub struct CallbackSigner<F> {
    mode: String,
    f: F,
}

impl<F> CallbackSigner<F>
where
    F: Fn(&SignTarget<'_>) -> Result<SignReport, Error>,
{
    pub fn new(mode: impl Into<String>, f: F) -> Self {
        CallbackSigner {
            mode: mode.into(),
            f,
        }
    }
}

impl<F> CodeSigner for CallbackSigner<F>
where
    F: Fn(&SignTarget<'_>) -> Result<SignReport, Error>,
{
    fn mode(&self) -> &str {
        &self.mode
    }
    fn sign(&self, target: &SignTarget<'_>) -> Result<SignReport, Error> {
        (self.f)(target)
    }
}
