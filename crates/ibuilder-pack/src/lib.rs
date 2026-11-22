//! Rust port of iOSBuilder's `tools/pack_nxgame.py` and `tools/compare_nxgame.py`.
//!
//! * [`pack`] builds a `<title ID>.nxgame` package: the framework folder, `game.manifest.json` (the engine's
//!   `gen_manifest.py` schema), `package.json` (the Mac app's `PackageWriter` shape, Swift pretty-printer
//!   spelling), `romfs.img`, `aoc/`, with the SHA-256 of every framework file listed in `package.json`.
//! * [`compare`] reads two manifests and reports every key (equal / different / only one side / type mismatch).
//!
//! Output is byte-identical to the Python's (`tests/oracle.rs`). JSON is hand-rolled ([`json`]) to reproduce
//! Python's `json.dumps` spellings exactly. No process is spawned: APFS `--clone` uses `clonefile(2)` through
//! `libc` (the only `unsafe`, isolated in [`clone`]).
#![deny(unsafe_code)]

pub mod clone;
pub mod compare;
pub mod json;
pub mod pack;
pub mod profile;
pub mod pyutil;
pub mod romfs_id;

pub use compare::{CompareReport, compare, compare_files_cli, find_manifest};
pub use pack::{PackArgs, PackError, PackOutcome, pack};
pub use romfs_id::{romfs_identity_of_image, romfs_identity_short};
