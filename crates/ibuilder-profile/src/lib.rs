//! Rust port of the iOS app's profile derivation, `gameapp/profile_derive.py`, with the engine modules it runs
//! in-process ported as well, so no Python (and no `runpy`) is needed on the phone:
//!
//! | Python | module |
//! |---|---|
//! | `gameapp/profile_derive.py` (`derive`, `title_id`, `write_provisional_profile`, `ipa_name`, `EMPTY_TABLES`) | [`derive`] |
//! | `dm/games/profile_guess.py` (`facts_of`, `guess`, `engine`, `kind`, CLI) | [`facts`], [`guess`] |
//! | `dm/tools/nso.py` (`load`) | [`nso`] |
//! | `dm/tools/nso_syms.py` (`dynsyms`, `area`, `host_hooks`) | [`syms`] |
//! | `dm/tools/nacp.py` (`name`, `version`) | [`nacp`] |
//! | `dm/tools/romfs.py` + `romfs_files` (manifest / DMROMFS1 image / folder) | [`romfs`] |
//! | `dm/games/game.py` (`Game`, `header_one`, the `profile` subcommand) | [`game`] |
//! | `gameapp/compare_profile.py` | [`compare`] |
//!
//! `#![forbid(unsafe_code)]`, no process is started, the NSO modules are processed one at a time and in parallel
//! (rayon) and dropped as soon as their facts are extracted.
#![forbid(unsafe_code)]

pub mod compare;
pub mod derive;
pub mod error;
pub mod facts;
pub mod game;
pub mod guess;
pub mod nacp;
pub mod nso;
pub mod pyutil;
pub mod romfs;
pub mod search;
pub mod syms;

pub use error::Error;
pub use facts::{Facts, facts_of};
pub use guess::{Guess, guess};
pub use syms::HookSet;
