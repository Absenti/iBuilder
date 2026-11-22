//! # ibuilder-mods
//!
//! Atmosphere-style mods and cheats for the build chain: a port of `gameapp/mods.py` (docs/DECISIONS.md 89 and 90 of the Python app).
//!
//! The app's importer (C#, not ported: it is UI) keeps what belongs to a game in ONE canonical store, `Documents/games/<id>/mods/<mod name>/`:
//!
//! ```text
//! romfs/...               files that replace or add RomFS files          -> DM_ROMFS_OVERLAY at the boot
//! exefs/<name>.ips        IPS / IPS32 patch, named by the NSO's build id  -> the NSO, before stage 2
//! exefs/<name>.pchtxt     IPSwitch text patch (60 fps mods), keyed inside by @nsobid-<build id>
//! cheats/<build id>.txt   Atmosphere cheat file (dmnt opcodes)            -> DM_CHEATS at the boot (the engine's VM)
//! ```
//!
//! and the player's choices in `Documents/games/<id>/mods.tsv` (`mod`, `patch`, `cheat` rows). What this crate does, in the Python's order:
//!
//! * [`store`]: `read_state`, `list_mods`, `enabled_mods`, `exefs_signature` (the "Rebuild to apply" text the C# computes alike);
//! * [`patch`]: `parse_ips` (IPS and IPS32), `parse_pchtxt` (`@nsobid-`, `@flag offset_shift`, `@enabled` / `@disabled`, `@stop`, comments), `bid_matches`,
//!   `exefs_patches` (the 0x100 header offset rule: a module offset is the IPS offset minus 0x100);
//! * [`nso`]: the NSO image (LZ4 segments), `apply_writes`, `build_nso` (stored plain, flags cleared), `patch_nso`;
//! * [`chain`]: the hooks stage 2 calls: `plan`, `apply_module`, `remove_patched`, `mark_applied`, `record_build_ids`;
//! * [`boot`]: `apply_boot` (RomFS overlay -> `DM_ROMFS_OVERLAY`, cheat files -> `cheats-run` -> `DM_CHEATS`).
//!
//! No process is spawned, no `unsafe`, nothing that is not in `std` besides `sha2`: the crate builds for iOS. What the Python raises is a
//! [`ModsError`]: either its own `ModError` (a message the log shows) or an exception the Python does *not* catch and that therefore
//! propagates to the chain (kept as it is: [`ModsError::Unhandled`]; see `docs/crates/MODS-RUST.md` for the list).

#![forbid(unsafe_code)]

pub mod boot;
pub mod chain;
pub mod error;
pub mod lz4;
pub mod nso;
pub mod patch;
pub mod pyfmt;
// generated file, kept exactly as its CPython generator writes it (rustfmt would reflow it, so a regeneration would not reproduce the committed text)
#[rustfmt::skip]
mod pytables;
pub mod store;

pub use boot::{
    Environ, apply_boot, cheat_file_bid, cheat_sections, romfs_overlays, write_cheats_folder,
};
pub use chain::{
    ModPlan, apply_module, is_module_name, mark_applied, plan, record_build_ids, remove_patched,
};
pub use error::{ModsError, Result};
pub use nso::{Patched, apply_writes, build_nso, nso_build_id, nso_image, patch_nso};
pub use patch::{Patch, bid_matches, exefs_patches, parse_ips, parse_pchtxt, stem_bid};
pub use store::{ModState, enabled_mods, exefs_signature, list_mods, read_state};

/// `NSO_HEADER`: Atmosphere's IPS offsets count the 0x100-byte NSO header, so the module image's offset is `ips_offset - 0x100`.
pub const NSO_HEADER: usize = 0x100;
/// NSO flag bit 7: the compressed segments are ZBIC (zstd), not LZ4.
pub const NSO_ZSTD: u32 = 1 << 7;
pub const STATE_NAME: &str = "mods.tsv";
pub const APPLIED_NAME: &str = "mods.applied";
pub const BUILD_IDS_NAME: &str = "build_ids.txt";
/// The patched copies of the modules, only while stage 2 reads them.
pub const PATCHED_DIR: &str = "mods-exefs";
/// What `DM_CHEATS` names: regenerated at each boot from the store and `mods.tsv`.
pub const CHEATS_DIR: &str = "cheats-run";
