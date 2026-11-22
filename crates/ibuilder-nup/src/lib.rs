//! The NUP container (one `.nup` file, https://github.com/a55uka/nup SPEC v1): a Switch 2 game as the base game, its updates by
//! version and its DLC by title ID, each an `exefs/` and `romfs/` tree, every file stored raw or as ONE zstd frame, with a SHA-256
//! per file. Rust port of the engine's `scripts/pack_nup.py` (the reader the Python app runs as `extract_nup`):
//!
//! | Python | here |
//! | --- | --- |
//! | `Nup`, `Package` (`_read`, `_acyclic`, `children`, `child`, `tree_bytes`, `copy`, `read`, `head`) | [`Nup`], [`Package`] (`nup.rs`) |
//! | `scan`, `program_id`, `nacp_ids`, `sibling_romfs`, `romfs_bytes`, `romfs_of` | [`scan`], [`Scan`] (`scan.rs`) |
//! | `exefs`, `control`, `localized`, `pick`, `pack_version`, `pack_dlc` | [`extract`] (`extract.rs`) |
//! | `romfs_image.{entries, layout, write, stamp, reusable}`, `entries`, `verified`, `pack` | [`romfs`] (`romfs.rs`) |
//! | `gameapp_pipeline.extract_nup`, the free-space check | [`extract_application`] |
//! | (new) the cover of a `.nup` before any build: its icon and title ID from the head of the file | [`application_icon`] (`icon.rs`) |
//! | `gameapp/nup_zstd.py` (the app's streaming zstd binding) | [`ZstdBackend`] (`zstd.rs`): the C libzstd, the pure-Rust `ruzstd`, or a callback the app supplies |
//!
//! What differs from the Python, on purpose: positioned reads on the file (`pread`, no memory map of 25 GB), the RomFS image is
//! written by several threads (every file's offset in the image is known from the container's sizes before the first byte is
//! written: output identical for any thread count), progress callbacks and a cancellation flag, and the image's reuse stamp is
//! this crate's own (the Python's hashes the text of its own scripts; a stamp it does not know makes it check the image against the
//! container's SHA-256 once, which is what this crate's [`romfs::verify_image`] does as well).
//!
//! A NUP is not encrypted: no keys are needed. A file's zstd frame has no dictionary: the `.zs` files inside the RomFS (the game's own
//! compression, with `Pack/ZsDic.pack.zs` dictionaries) are copied as the bytes they are.
//!
//! iOS: no `fork`/`exec`, no `std::process`; threads through rayon; `unsafe` only in `sys.rs` (`statvfs`).

#![deny(unsafe_code)]

mod error;
pub mod extract;
mod icon;
mod io;
mod nup;
pub mod romfs;
mod scan;
mod sys;
mod zstd;

pub use error::{Error, Result};
pub use extract::{
    Extracted, How, Options, Progress, ProgressFn, check_space, extract_application, pack_dlc,
    pack_exefs_only, pack_version, romfs_bytes, verify_container, verify_image,
};
pub use icon::{AppIcon, META_MAX, application_icon, application_icon_from, icon_of};
pub use io::{FileSource, MemSource, ReadAt};
pub use nup::{
    ADDON, BASE, Child, KINDS, NO_DIR, Nup, Package, STANDALONE, SYSTEM, UPDATE, is_nup,
};
pub use scan::{NO_BASE, Scan, Sibling, nacp_ids, program_id, scan};
pub use sys::{file_identity, free_bytes};
#[cfg(feature = "native-zstd")]
pub use zstd::NativeZstd;
pub use zstd::{CallbackZstd, Ruzstd, ZstdBackend, default_backend};
