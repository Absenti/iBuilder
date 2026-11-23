//! `ibuilder-shadersplit`: the shader split of the engine (`gfx/tools/shader_split.py`) and the phone's wrapper of it (`gameapp/shader_device.py`: `split`, `JobMemo`) in Rust.
//!
//! Input: the game's RomFS (a `DMROMFS1` image or a folder), its `main` NSO, the add-on content roots, the game's profile folder. Output: `pkg/{ps,vs,cs,gs}_{data,main}.bin`
//! (`BTCDBTCD` records: key, control section, code area) and `tables/{shader_map,shader_facts,shader_pairs,pso_pairs}.tsv`, byte-identical to the Python's.
//! See `docs/shaders/SHADERSPLIT-RUST.md` for the Python-to-Rust table, what is verified and the containers this port does not read ([`error::Failure::Unsupported`]).

#![forbid(unsafe_code)]

pub mod aes;
pub mod apf;
pub mod archive;
pub mod bytes;
pub mod casc;
pub mod cpk;
pub mod cra;
pub mod ctl;
pub mod easf;
pub mod error;
pub mod falcom;
pub mod frostbite;
pub mod game;
pub mod glslc;
pub mod image;
pub mod inflate;
pub mod jobs;
pub mod kit;
pub mod live;
pub mod lspk;
pub mod lzma1;
pub mod memo;
pub mod mvgl;
pub mod nvn;
pub mod nxcx;
pub mod nxsd;
pub mod pdat;
pub mod pyjson;
pub mod rage_wad;
pub mod record;
pub mod redengine;
pub mod rpf6;
pub mod rt;
pub mod scan;
pub mod scan_types;
pub mod sharc;
pub mod smash_arc;
pub mod source;
pub mod spill;
pub mod split;
pub mod telltale;
pub mod trinity;
pub mod ttdat;
pub mod uepak;
pub mod uestore;
pub mod unity;
pub mod vbf;
// generated file, kept exactly as tools/gen_known_trees.py writes it (rustfmt would reflow it, so a regeneration would not reproduce the committed text)
#[rustfmt::skip]
mod unity_trees;
pub mod zlibx;
pub mod zstd;

pub use error::{Failure, SplitError};
pub use split::{JobGate, NoGate, SplitOptions, SplitStats, split, split_version};
pub use zstd::{Ruzstd, Zstd};
