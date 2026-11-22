//! In-process ad-hoc Mach-O code signature: a Rust port of iOSBuilder's `pipeline/adhoc_sign.py`.
//!
//! iOS has no `codesign`, and `dyld` refuses an unsigned dylib, so the toolchain signs the game framework itself.
//! This crate writes exactly what `codesign -s -` (and `ld`) write for a plain dylib: a SuperBlob holding one
//! CodeDirectory (v0x20400, SHA-256, 4096-byte pages, no special slots), `LC_CODE_SIGNATURE` pointing at it, and a
//! `__LINKEDIT` grown to contain the blob. The output is byte-identical to the Python's on the same input
//! (`tests/oracle.rs`) and is accepted by `codesign --verify --strict`.
//!
//! Design points
//! * `#![forbid(unsafe_code)]`, no `fork`/`exec`: it runs inside the iOS app (the CLI / tests may call `codesign`).
//! * [`Source`] abstracts the image: an in-memory `Vec<u8>` or a [`std::fs::File`] signed **in place**. For a file the
//!   hashing is streamed (1 MiB chunks, rayon), so signing a 190 MB framework needs about one chunk per thread of
//!   RAM instead of the Python's two full copies (`bytearray(fh.read())`).
//! * [`CodeSigner`] is the seam for the real signer (zsign with the user's `.p12`): see `docs/crates/ibuilder-sign.md`.
#![forbid(unsafe_code)]

mod adhoc;
mod error;
mod macho;
mod signer;
mod source;

pub use adhoc::{
    CodeDirectoryView, Plan, SignInfo, SignOptions, VerifyInfo, plan, plan_with, sign_bytes,
    sign_bytes_with, sign_file, sign_file_with, verify_bytes, verify_file, verify_source,
};
pub use error::Error;
pub use signer::{AdhocSigner, CallbackSigner, CodeSigner, SignReport, SignTarget};
pub use source::Source;

/// Hash page size of the CodeDirectory (`pageSize` field 12).
pub const PAGE: usize = 4096;
pub const PAGE_LOG2: u8 = 12;
pub const CS_ADHOC: u32 = 0x2;
pub const HASH_TYPE_SHA256: u8 = 2;
pub const CSMAGIC_EMBEDDED_SIGNATURE: u32 = 0xFADE_0CC0;
pub const CSMAGIC_CODEDIRECTORY: u32 = 0xFADE_0C02;
/// CodeDirectory version written (supports scatter, team, codeLimit64 and the exec segment fields).
pub const CD_VERSION: u32 = 0x20400;
/// Size of the fixed part of the v0x20400 CodeDirectory.
pub const CD_FIXED: usize = 88;
