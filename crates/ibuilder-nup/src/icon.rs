//! A `.nup`'s cover before any build: the icon of the application's package and its title ID, read from the container's HEAD only.
//!
//! What is read, and nothing else: the 0x80-byte superblock, the table of contents (every open of a container reads it whole and checks its SHA-256: 15 MB for the real
//! Ocarina container's 186,583 + 186,583 files, against 25.6 GB), the application package's meta block (a TLV list: the NACP, the icon, the localized icons; some kilobytes)
//! and, to name the title the way the build does, the `main.npdm` of each package (a few hundred bytes to a few kilobytes each). No game file is read, no RomFS entry, no zstd
//! frame of the game's content. A container is NOT encrypted: no `prod.keys` are needed, none are read.
//!
//! The icon is chosen by the very function the build uses to write `icon.jpg` (`extract::choose_icon`): the icon record, else the `en-US` localized one, else the first localized one,
//! of the application package (`extract::app_package`: the base package, else the first). When that package carries none, the icon the BUILD writes is the cover: the very records
//! it writes from (`extract::control_meta`) of the package it packs by default (`newest_version` / `package_of`: the newest update, else the base; the base's records under them when
//! there is no base game). The real Ocarina container's base has none (see `AppIcon::icon`); an update package of a container may carry one. Some kilobytes more are read then.
//!
//! Never panics on a damaged or truncated head: every length and offset is bounds-checked by [`Nup::from_source`] (the same checks the build makes), the meta block and the NPDMs
//! are refused above a size no real container has, and what cannot be read is an `Err`.

use crate::error::{Result, bail};
use crate::extract::{app_package, choose_icon, control_meta, newest_version, package_of};
use crate::io::{FileSource, ReadAt};
use crate::nup::{NO_DIR, Nup};
use crate::scan::{program_id, scan};
use std::path::Path;

/// The largest meta block read. The control data of a game with an icon in every language is under a megabyte; a header that claims more is damaged, and a damaged length must not
/// become an allocation (on an iPhone the process is killed for its memory, not told).
pub const META_MAX: u32 = 32 << 20;

/// The largest `main.npdm` read to name the title (the real one is 1,828 bytes).
const NPDM_MAX: u64 = 1 << 20;

/// TLV type 1 of a package's meta block: the NPDM (the real container's base package carries it, 1,828 bytes).
const T_NPDM: u16 = 1;

/// What the head of a `.nup` says about its cover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppIcon {
    /// The image bytes `control()` would write to `icon.jpg` for this package; None when the package carries none (the real Ocarina container does not: its base package's meta has
    /// the NPDM and three small records, no NACP and no icon).
    pub icon: Option<Vec<u8>>,
    /// The title ID the build names the game by (the program ID of the program it packs, `scan`'s answer); when `scan` refuses the container, the program ID in the package's own
    /// NPDM record; None when neither can be read.
    pub title_id: Option<u64>,
    /// The package the icon was read from, as the build prints it (`Game.nup (base)`).
    pub package: String,
}

/// The cover of the `.nup` at `path`: [`application_icon_from`] on the file.
pub fn application_icon(path: &Path) -> Result<AppIcon> {
    let src = FileSource::open(path)
        .map_err(|e| crate::Error::Msg(format!("{}: {e}", path.display())))?;
    application_icon_from(Box::new(src), path)
}

/// The cover of a container read through `src` (a counting reader in the tests; `path` is only a name for the messages).
pub fn application_icon_from(src: Box<dyn ReadAt>, path: &Path) -> Result<AppIcon> {
    let n = Nup::from_source(src, path)?;
    icon_of(&n)
}

/// The cover of an opened container.
pub fn icon_of(n: &Nup) -> Result<AppIcon> {
    let k = app_package(n);
    if k.meta_size > META_MAX {
        bail!(
            "{}: the meta block of {} is {} bytes (implausible, a damaged file?)",
            n.path.display(),
            k.name,
            k.meta_size
        );
    }
    let meta = n.meta(k)?;
    // `scan` is the build's own answer; it reads each package's NPDM, so it is asked only when none of them claims a size no real NPDM has
    let sc = if npdms_are_small(n) {
        scan(n, None).ok()
    } else {
        None
    };
    // the application package's own icon (as before); when it carries none, what the build writes to icon.jpg: the package it packs by default (the newest update), its records
    // over the base's when there is no base (a meta block of an implausible size is skipped)
    let mut icon = choose_icon(&meta);
    let mut package = k.name.clone();
    if icon.is_none()
        && let Some(ki) = sc.as_ref().and_then(|s| package_of(s, newest_version(s)))
    {
        let built = &n.packages[ki];
        let fallback = if sc.as_ref().is_some_and(|s| s.base.is_none()) {
            Some(k)
        } else {
            None
        };
        if built.meta_size <= META_MAX {
            icon = choose_icon(&control_meta(n, built, fallback)?);
            if icon.is_some() {
                package = built.name.clone();
            }
        }
    }
    let title_id = sc.map(|sc| sc.program_id).or_else(|| {
        meta.iter()
            .find(|(t, _)| *t == T_NPDM)
            .and_then(|(_, b)| program_id(b, "meta").ok())
    });
    Ok(AppIcon {
        icon,
        title_id,
        package,
    })
}

/// Every `exefs/main.npdm` of the container is of a plausible size (a damaged file record must not make `scan` read gigabytes to name a title).
fn npdms_are_small(n: &Nup) -> bool {
    n.packages.iter().all(|k| {
        k.exefs == NO_DIR
            || n.child(k.exefs, b"main.npdm")
                .is_none_or(|i| n.file_size(i) <= NPDM_MAX)
    })
}
