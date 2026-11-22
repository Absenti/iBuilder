//! `pack_nup.scan`: what a NUP holds: (title ID, base package or None, {update version: package}, {DLC title ID: package}), with the
//! checks `pack_dump.scan` makes. The title ID is the program ID of the program packed. One application can hold several programs
//! (a Switch 2 Edition of a Switch 1 game), each its own base package and updates; they are told apart by their NPDM's program ID,
//! whose low 12 bits are the program index, and belong to one application when their NACPs name it (SaveDataOwnerId), else when
//! their program IDs agree above those bits.

use crate::error::{Result, bail};
use crate::nup::{ADDON, BASE, NO_DIR, Nup, Package, STANDALONE, UPDATE};
use std::collections::BTreeMap;

/// Per program index: (key, own base, the add-on bases by index).
type Layout = BTreeMap<u32, (u64, Option<usize>, BTreeMap<u32, usize>)>;

pub const NO_BASE: &str = "no base game in this NUP: its update is taken as the whole game (if the game then stops on missing files, the update holds only what changed: pack a NUP with the base game)";

const T_NACP: u16 = 2;
const NACP_AOC_BASE: usize = 0x3070;
const NACP_SAVE_OWNER: usize = 0x3078;

/// One of the other programs of the application: (program ID, its base package, {version: update package}); packages are indexes into `Nup::packages`
#[derive(Debug, Clone)]
pub struct Sibling {
    pub program_id: u64,
    pub base: Option<usize>,
    pub updates: BTreeMap<u32, usize>,
}

#[derive(Debug, Clone)]
pub struct Scan {
    /// the application's ID (the NACP's SaveDataOwnerId, else the program ID with its low 12 bits cleared)
    pub application: u64,
    /// the program packed (NPDM program ID)
    pub program_id: u64,
    /// a DLC's index is its title ID minus this (the NACP's AddOnContentBaseId, else the application's + 0x1000)
    pub aoc_base: u64,
    pub base: Option<usize>,
    pub versions: BTreeMap<u32, usize>,
    pub dlc: BTreeMap<u64, usize>,
    /// the other programs {index: ...}, whose RomFS `pack_version` packs beside this one's
    pub siblings: BTreeMap<u32, Sibling>,
}

/// the ACI0 program ID of an NPDM
pub fn program_id(npdm: &[u8], whence: &str) -> Result<u64> {
    if npdm.get(..4) != Some(b"META") {
        bail!("{whence}: not an NPDM");
    }
    let rd = |o: usize| -> Option<u32> {
        npdm.get(o..o + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    };
    let (Some(off), Some(size)) = (rd(0x70), rd(0x74)) else {
        bail!("{whence}: no ACI0 section")
    };
    let off = off as usize;
    if npdm.get(off..off + 4) != Some(b"ACI0") || size < 0x18 {
        bail!("{whence}: no ACI0 section");
    }
    match npdm.get(off + 0x10..off + 0x18) {
        Some(b) => Ok(u64::from_le_bytes(b.try_into().unwrap())),
        None => bail!("{whence}: no ACI0 section"),
    }
}

/// (application ID, add-on content base ID) from the package's NACP; None for each it does not give
pub fn nacp_ids(nup: &Nup, k: &Package) -> Result<(Option<u64>, Option<u64>)> {
    let meta = nup.meta(k)?;
    let Some((_, b)) = meta.iter().find(|(t, _)| *t == T_NACP) else {
        return Ok((None, None));
    };
    if b.len() < NACP_SAVE_OWNER + 8 {
        return Ok((None, None));
    }
    let aoc = u64::from_le_bytes(b[NACP_AOC_BASE..NACP_AOC_BASE + 8].try_into().unwrap());
    let owner = u64::from_le_bytes(b[NACP_SAVE_OWNER..NACP_SAVE_OWNER + 8].try_into().unwrap());
    Ok(((owner != 0).then_some(owner), (aoc != 0).then_some(aoc)))
}

/// `program`: the index of the program to pack, default the highest (a Switch 2 Edition's Switch 2 program)
pub fn scan(n: &Nup, program: Option<u32>) -> Result<Scan> {
    let path = n.path.display().to_string();
    let bases: Vec<usize> = n
        .packages
        .iter()
        .filter(|k| k.kind == BASE)
        .map(|k| k.index)
        .collect();
    let updates: Vec<usize> = n
        .packages
        .iter()
        .filter(|k| k.kind == UPDATE)
        .map(|k| k.index)
        .collect();
    if bases.is_empty()
        && (updates.is_empty()
            || updates
                .iter()
                .any(|&u| n.packages[u].flags & STANDALONE == 0))
    {
        bail!(
            "{path}: no base game: pack the base game and its update into one NUP, or use updates marked standalone_update with complete exefs/ and romfs/"
        );
    }
    // a base record with no files is no base game
    let real: Vec<usize> = bases
        .iter()
        .copied()
        .filter(|&k| n.tree_bytes(n.packages[k].root) != 0)
        .collect();
    // program ID -> its packages, base first (a Vec keeps the Python dict's insertion order); base packages without code
    let mut progs: Vec<(u64, Vec<usize>)> = vec![];
    let mut loose: Vec<usize> = vec![];
    for &ki in real.iter().chain(updates.iter()) {
        let k = &n.packages[ki];
        let npdm = if k.exefs != NO_DIR {
            n.child(k.exefs, b"main.npdm")
        } else {
            None
        };
        let Some(i) = npdm else {
            if k.kind == BASE {
                loose.push(ki);
            }
            continue;
        };
        let pid = program_id(&n.read(i)?, &format!("{} exefs/main.npdm", k.name))?;
        match progs.iter_mut().find(|(p, _)| *p == pid) {
            Some((_, v)) => v.push(ki),
            None => progs.push((pid, vec![ki])),
        }
    }
    if progs.is_empty() {
        bail!("{path}: no exefs/main.npdm in the base or updates");
    }
    if !loose.is_empty() && (progs.len() > 1 || loose.len() > 1) {
        bail!(
            "{path}: a base package without exefs/main.npdm among several programs: which one it belongs to is unknown"
        );
    }
    if let Some(&l) = loose.first() {
        progs[0].1.insert(0, l);
    }
    let mut owner: Vec<(u64, u64)> = vec![];
    for (pid, ks) in &progs {
        let mut found = None;
        for &k in ks {
            if let Some(a) = nacp_ids(n, &n.packages[k])?.0 {
                found = Some(a);
                break;
            }
        }
        owner.push((*pid, found.unwrap_or(*pid & !0xFFF)));
    }
    let mut owners: Vec<u64> = owner.iter().map(|o| o.1).collect();
    owners.sort_unstable();
    owners.dedup();
    if owners.len() > 1 {
        let mut o = owner.clone();
        o.sort_unstable();
        let list: Vec<String> = o
            .iter()
            .map(|(pid, a)| format!("program {pid:016X} of {a:016X}"))
            .collect();
        bail!(
            "{path}: packages of different applications: {}",
            list.join(", ")
        );
    }
    let mut idxs: Vec<u64> = progs.iter().map(|(p, _)| p & 0xFFF).collect();
    idxs.sort_unstable();
    idxs.dedup();
    if idxs.len() != progs.len() {
        let mut ids: Vec<String> = progs.iter().map(|(p, _)| format!("{p:016X}")).collect();
        ids.sort();
        bail!("{path}: two programs of index {}", ids.join(", "));
    }
    let mut layout: Layout = BTreeMap::new();
    for (pid, ks) in &progs {
        let own_bases: Vec<usize> = ks
            .iter()
            .copied()
            .filter(|&k| n.packages[k].kind == BASE)
            .collect();
        if own_bases.len() > 1 {
            bail!(
                "{path}: {} base packages of program {pid:016X}",
                own_bases.len()
            );
        }
        let mut vers: BTreeMap<u32, usize> = BTreeMap::new();
        for &ki in ks {
            let k = &n.packages[ki];
            if k.kind != UPDATE {
                continue;
            }
            if vers.contains_key(&k.version) {
                bail!(
                    "{path}: two updates of version {}{}",
                    k.version,
                    if progs.len() > 1 {
                        format!(" of program {pid:016X}")
                    } else {
                        String::new()
                    }
                );
            }
            if k.version == 0 {
                bail!("{path}: an update of version 0");
            }
            vers.insert(k.version, ki);
        }
        layout.insert(
            (*pid & 0xFFF) as u32,
            (*pid, own_bases.first().copied(), vers),
        );
    }
    let program = match program {
        Some(p) => p,
        None => *layout.keys().next_back().unwrap(),
    };
    if !layout.contains_key(&program) {
        let have: Vec<String> = layout
            .iter()
            .map(|(i, (pid, _, _))| format!("{i} ({pid:016X})"))
            .collect();
        bail!("{path}: no program {program}; it holds {}", have.join(", "));
    }
    let (app_pid, base, vers) = layout.remove(&program).unwrap();
    let application = owner
        .iter()
        .find(|(p, _)| *p == app_pid)
        .map(|o| o.1)
        .unwrap();
    // newest first: this program's updates by descending version, its base, then the other programs' packages
    let mut newest: Vec<usize> = vers.values().rev().copied().collect();
    newest.extend(base);
    // the other programs in the order their packages were met (the Python dict's insertion order)
    for (pid, _) in &progs {
        if let Some((_, b, vs)) = layout.get(&((*pid & 0xFFF) as u32)) {
            newest.extend(vs.values().copied());
            newest.extend(*b);
        }
    }
    let mut aoc_base = None;
    for &k in &newest {
        if let Some(a) = nacp_ids(n, &n.packages[k])?.1 {
            aoc_base = Some(a);
            break;
        }
    }
    let aoc_base = aoc_base.unwrap_or(application.wrapping_add(0x1000));
    let mut dlc: BTreeMap<u64, usize> = BTreeMap::new();
    for k in &n.packages {
        if k.kind != ADDON {
            continue;
        }
        if !(k.title_id > aoc_base && k.title_id - aoc_base <= 2000) {
            bail!(
                "{path}: {:016X} is not a DLC of {application:016X} (its add-on content IDs start at {aoc_base:016X})",
                k.title_id
            );
        }
        if k.romfs == NO_DIR {
            bail!("{path}: DLC {:016X} has no romfs/", k.title_id);
        }
        if dlc.contains_key(&k.title_id) {
            bail!("{path}: DLC {:016X} more than once", k.title_id);
        }
        dlc.insert(k.title_id, k.index);
    }
    let siblings = layout
        .into_iter()
        .map(|(i, (program_id, base, updates))| {
            (
                i,
                Sibling {
                    program_id,
                    base,
                    updates,
                },
            )
        })
        .collect();
    Ok(Scan {
        application,
        program_id: app_pid,
        aoc_base,
        base,
        versions: vers,
        dlc,
        siblings,
    })
}
