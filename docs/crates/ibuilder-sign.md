# ibuilder-sign: in-process ad-hoc Mach-O signature

Port of `iOSBuilder/pipeline/adhoc_sign.py` (225 lines; main and the 2026-11-17 worktree copy are byte-identical, ported
either). Crate: `crates/ibuilder-sign` (`#![forbid(unsafe_code)]`, deps: `sha2`, `rayon`; no process is spawned).

iOS has no `codesign` and `dyld` refuses an unsigned dylib (`Trying to load an unsigned library`, `docs/SIGN-IN-APP.md`
section 3), so the toolchain signs the assembled game framework itself. This crate writes exactly what `codesign -s -`
and `ld` write for a plain dylib: a SuperBlob holding one CodeDirectory (v0x20400, SHA-256, 4096-byte pages, no special
slots), `LC_CODE_SIGNATURE` pointing at it, and `__LINKEDIT` grown to contain it.

## Source -> module

| Python (`adhoc_sign.py`) | Rust |
|---|---|
| `_load_commands`, `_inspect`, `_slack` | `src/macho.rs` (`load_commands`, `inspect`, `slack`) |
| `sign()` steps 1-5 (LC slot, LINKEDIT grow, CodeDirectory, SuperBlob) | `src/adhoc.rs` `plan` / `plan_with` -> `Plan`, then `Plan::apply_to_vec` / `apply_to_file`; `sign_bytes`, `sign_file` |
| `verify()` | `src/adhoc.rs` `verify_source`, `verify_bytes`, `verify_file` |
| `__main__` (sign then verify, `%r` dict lines) | `src/bin/ibuilder-sign.rs` |
| (new) the seam for the real signer | `src/signer.rs` `CodeSigner`, `AdhocSigner`, `CallbackSigner` |

Design differences that do not change the output:

* **Plan, then apply.** The whole signature is computed before anything is written. For a file, hashing reads it with
  positional reads in 1 MiB chunks, in parallel (rayon), through a "virtual view" (patched header, then the file, then zero
  padding), so a 190 MB framework is signed with ~24 MiB RSS instead of the Python's two full copies (`bytearray(read())`).
  `apply_to_file` truncates, zero-extends to `codeLimit`, writes the blob, and writes the header last, so an interrupted run
  never leaves a header pointing at a missing blob (the Python truncates the file with `open(.., 'wb')` first).
* Header parsing works on the first MiB of the file (more when `sizeofcmds` needs it, refused above 64 MiB).

## How to run

```
export CARGO_TARGET_DIR=$SCRATCH_OLD/target-<you>
cargo build --release -p ibuilder-sign
target/release/ibuilder-sign <dylib> [identifier]        # sign, then verify (what `python adhoc_sign.py` prints)
target/release/ibuilder-sign sign <dylib> [identifier]
target/release/ibuilder-sign verify <dylib>
target/release/ibuilder-sign --shrink-linkedit <dylib>   # see "A defect found in the Python"
```
Library: `ibuilder_sign::{sign_file, sign_bytes, verify_file, plan, AdhocSigner, SignInfo, VerifyInfo}`. Identifier
precedence is the Python's: argument (if non-empty), then the dylib's `LC_ID_DYLIB` name, then the file name.

## Verification (commands and results, this Mac, 2026-11-19)

`cargo test --release -p ibuilder-sign`: 9 unit tests (hand-made Mach-O: layout field by field, idempotent re-sign, identifier
precedence, unaligned length, tampering detected, the Python's error messages, in-memory == streamed file, shrink option,
`CodeSigner` trait), 4 oracle tests, 1 doc-test: all pass. The oracle tests (`tests/oracle.rs`) sign a copy with the Python
and a copy with Rust, require **byte-identical files**, then require `codesign --verify --strict -vvv` to accept the Rust file.
Output of `cargo test --release -p ibuilder-sign --test oracle -- --nocapture`:

```
OK   unsigned-probe.dylib: byte-identical to Python (16920 B), codesign --verify --strict ok
OK   host-sim.dylib: byte-identical to Python (2897456 B), codesign --verify --strict ok
OK   clang-unsigned.dylib: byte-identical to Python (66504 B), codesign --verify --strict ok
OK   clang-ldsigned.dylib: byte-identical to Python (33480 B), codesign --verify --strict ok
OK   clang-unsigned-ident.dylib: byte-identical to Python (66504 B), codesign --verify --strict ok
OK   host-ios.dylib: byte-identical to Python (4800288 B), codesign --verify --strict ok
OK   DmGameProbe-sign: byte-identical to Python (192306624 B), codesign --verify --strict ok
```
Inputs: arm64 dylibs built by the test with `clang` (with `-Wl,-headerpad,0x8000 -Wl,-no_adhoc_codesign`, and a plain `ld`-signed
one that gets re-signed), plus read-only fixtures `out-sign/unsigned-probe.dylib`, `out-sign/host-prebuilt-loadable-sim.dylib`,
`out-load/host-prebuilt-loadable-ios.dylib` (platform iOS) and the 190,838,288-byte assembled framework
`out-sign/DmGameProbe.framework/DmGameProbe`. With `IBUILDER_SIGN_BIG=1` the test also signs `out-asm/DmGameProbe.framework/DmGameProbe`
(191 MB, explicit identifier `dev.nxpocket.gameapp`): `OK DmGameProbe-asm: byte-identical to Python (192288328 B), codesign ... ok`.
By hand (`cp -c` copies, Python vs `ibuilder-sign`, `cmp`) `out-load/DmGame0100DFA027504000.framework/DmGame0100DFA027504000`
(195 MB, previously signed by zsign) is also byte-identical (but see the defect below).
Controls: flipping one byte of a signed page makes `codesign` say `invalid signature (code or signature have been modified)`;
the unsigned input says `code object is not signed at all`.

### A defect found in the Python (and an opt-in fix)

For `out-load/DmGame0100DFA027504000.framework/...` (already signed by zsign with a larger signature) the Python's output, and
therefore this crate's default output, is **rejected by `codesign`** (`main executable failed strict validation`):
`adhoc_sign.py` only ever grows `__LINKEDIT` (`max(old filesize, needed)`), so after dropping a *larger* old signature the
segment still reaches past the end of the file (`filesize 3755184 (past end of file)` in `otool -l`). The default stays
byte-identical to the Python; `--shrink-linkedit` / `SignOptions { shrink_linkedit: true }` also shrinks `filesize` to the new end
of file and the result passes `codesign --verify --strict -vvv` (checked on that file). The pipeline's normal inputs (unsigned
or host-signed assembly output, signature not larger than the new one) are not affected.

## Numbers (indicative: shared Mac, M1 Pro; `/usr/bin/time -l`, median of 7, fresh APFS clone of the 190.8 MB framework per run, includes the verify pass)

| | wall | max RSS | peak footprint |
|---|---|---|---|
| `python adhoc_sign.py` | 0.380 s | 384.5 MiB | 376.2 MiB |
| `ibuilder-sign` | 0.060 s | 23.6 MiB | 23.0 MiB |

(The Python is faster here than the 1.5 s of `docs/SIGN-IN-APP.md` because hashlib uses the CPU's SHA-2 instructions; so does `sha2`.)

## iOS portability

`cargo build --release --target aarch64-apple-ios` and `--target aarch64-apple-ios-sim` (rustup keg on `PATH`) both build the lib
and the CLI. The simulator binary was run inside the booted iPhone 17 simulator
(`xcrun simctl spawn <SIMULATOR-UDID> .../aarch64-apple-ios-sim/release/ibuilder-sign <dylib>`): it signed the
16 KB test dylib and the 190.8 MB framework in the simulator process, and the results are **byte-identical** to the host-signed
files and pass `codesign --verify --strict -vvv` on the Mac (the 190.8 MB run took 0.21 s wall as seen from `simctl`; its RSS
figure is the wrapper's, not the sim process's). Simulator timings are the Mac's CPU. Not run on a physical iPhone.

## The real (device identity) signer and how this crate is swappable

Out of scope to port: zsign (C++/OpenSSL) with the user's `.p12`. What the app does today (`gameapp/gameapp_shim.c` `py_sign`):
read its own CodeDirectory identifier (`self_identifier()`, so the framework is signed *as this app*; iOS accepts an image outside the
bundle only with the loading app's identifier); if `<app>/embedded.mobileprovision` exists call `device_identity_sign`, else call the
ad-hoc entry. The C ABI (`sign/inapp_shim.h`, implementation `sign/inapp_shim.cpp` over iPorterNX's pinned zsign plus
`patches/zsign/0001-dylib-entitlements.patch` and `sign/patches/zsign/0002-in-memory-asset.patch`):

| entry | inputs | note |
|---|---|---|
| `nxpocket_sign_adhoc(frameworkPath, executableName, identifier, err, errLen)` | framework folder, its `CFBundleExecutable`, identifier | zsign's ad-hoc mode: same CodeDirectory shape, plus it hashes `Info.plist` and writes `_CodeSignature/CodeResources` |
| `nxpocket_zsign_sign_memory(p12, p12Len, passphrase, provision, provisionLen, entitlements, entLen, frameworkPath, executableName, identifier, err, errLen)` | `.p12` bytes + password (read from the app's own Keychain item `dev.nxpocket.certificate` / `signing.identity`, never a file), the app's `embedded.mobileprovision` bytes, optional entitlements plist | the device path; returns 0 on success |
| `nxpocket_zsign_sign(p12Path, passphrase, provisionPath, entitlementsPath, ...)` | same, as files | iPorterNX's certified reference |
| `nxpocket_p12_certificate(...)` | `.p12` bytes, password | validates a `.p12` with the parser that will sign |

Swap point: `ibuilder_sign::CodeSigner { fn mode(&self) -> &str; fn sign(&self, &SignTarget) -> Result<SignReport> }` with
`SignTarget { framework: Option<&Path>, executable: &Path, identifier: Option<&str> }`, the same triple
`(framework, exe, identifier)` that `py_sign` hands to either path. `AdhocSigner` is the pure-Rust implementation (it ignores
`framework`; unlike zsign's ad-hoc mode it does not write `CodeResources` or bind `Info.plist`). The device signer is plugged
without this crate depending on zsign:

```rust
let device = CallbackSigner::new("device(zsign, in-memory .p12)", |t| {
    // extern "C" nxpocket_zsign_sign_memory(p12, .., t.framework, exe_name, t.identifier, err, 1024) -> rc
    /* map rc != 0 to Err(Error::Format(err)) */
    Ok(SignReport { mode: "device".into(), identifier: id, info: None })
});
let signer: &dyn CodeSigner = if has_embedded_provision { &device } else { &AdhocSigner::default() };
```
A pure-Rust ad-hoc signer cannot replace the device path: the device path needs a CMS signature, the team ID and entitlements from the
profile (see `docs/SIGN-IN-APP.md` sections 1-2, 5).

## Not done / not verified

* Ad-hoc only: no entitlements, requirements, CMS slot, `Info.plist` / `CodeResources` binding (zsign's ad-hoc mode adds the last two for a
  framework folder; the Python did not, and neither does this).
* 64-bit little-endian thin Mach-O only (the Python's limit): fat/universal and 32-bit inputs are refused.
* Not verified: acceptance by a physical iPhone's kernel (the simulator `dyld` checks that a signature covers the file but does not re-hash
  pages, `docs/SIGN-IN-APP.md` section 4); a device run is the missing proof, as for the Python.
* `--shrink-linkedit` is verified with `codesign` on one file (the zsign-signed 195 MB framework), not against a Python output (the Python has the defect).
