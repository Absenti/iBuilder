//! IPS, IPS32 and pchtxt: the groups of writes a mod's exefs files make, and which module they belong to (by build id).

use crate::error::{ModsError, Result};
use crate::pyfmt::{self, int_base0, int_base16, repr};
use crate::store::{ModState, enabled_mods, files, game_dir, mods_dir, read_state};
use crate::{NSO_HEADER, STATE_NAME};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

/// One group of writes of a mod: a whole IPS file, or one `@enabled` / `@disabled` block of a pchtxt. Offsets are Python ints (`i128`, saturated far
/// beyond any module): a negative offset is a write into the NSO header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patch {
    pub mod_name: String,
    pub source: String,
    pub name: String,
    pub bid: String,
    pub on: bool,
    pub writes: Vec<(i128, Vec<u8>)>,
}

impl Patch {
    pub fn new(
        mod_name: &str,
        source: &str,
        name: &str,
        bid: &str,
        on: bool,
        writes: Vec<(i128, Vec<u8>)>,
    ) -> Patch {
        Patch {
            mod_name: mod_name.into(),
            source: source.into(),
            name: name.into(),
            bid: bid.into(),
            on,
            writes,
        }
    }

    /// `Patch.digest()`. `struct.pack('<QI', off, len)` raises `struct.error` for an offset below 0 or above 2^64 - 1 (an IPS offset under 0x100, a pchtxt
    /// address under 0x100 with no `offset_shift`): in Python that propagates out of `plan`, and so it does here ([`ModsError::Unhandled`]).
    pub fn digest(&self) -> Result<String> {
        let mut h = Sha256::new();
        h.update(
            format!(
                "{}\0{}\0{}\0{}\0",
                self.mod_name, self.source, self.name, self.bid
            )
            .as_bytes(),
        );
        for (off, data) in &self.writes {
            if *off < 0 || *off > u64::MAX as i128 {
                return Err(ModsError::unhandled("error", "argument out of range"));
            }
            h.update((*off as u64).to_le_bytes());
            h.update((data.len() as u32).to_le_bytes());
            h.update(data);
        }
        Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
    }

    /// `'%s/%s [%s]' % (mod, source, name)`
    pub fn label(&self) -> String {
        format!("{}/{} [{}]", self.mod_name, self.source, self.name)
    }
}

/// `parse_ips`: `[(ips offset, bytes)]` of an IPS (`PATCH` ... `EOF`) or IPS32 (`IPS32` ... `EEOF`) file. An RLE record (size 0) is expanded. A truncated
/// record is a `ModError`, never a half-applied patch.
pub fn parse_ips(data: &[u8]) -> Result<Vec<(i128, Vec<u8>)>> {
    let (wide, end): (bool, &[u8]) = if data.starts_with(b"PATCH") {
        (false, b"EOF")
    } else if data.starts_with(b"IPS32") {
        (true, b"EEOF")
    } else {
        return Err(ModsError::modl(
            "not an IPS file (no PATCH or IPS32 header)",
        ));
    };
    let ow = if wide { 4 } else { 3 };
    let mut at = 5usize;
    let mut out = Vec::new();
    loop {
        if data.get(at..at + end.len()) == Some(end) {
            return Ok(out);
        }
        if at + ow + 2 > data.len() {
            return Err(ModsError::modl("the IPS file is cut short (no end marker)"));
        }
        let off = data[at..at + ow]
            .iter()
            .fold(0u64, |a, b| (a << 8) | *b as u64);
        let size = u16::from_be_bytes([data[at + ow], data[at + ow + 1]]) as usize;
        at += ow + 2;
        if size == 0 {
            if at + 3 > data.len() {
                return Err(ModsError::modl(
                    "the IPS file is cut short in an RLE record",
                ));
            }
            let count = u16::from_be_bytes([data[at], data[at + 1]]) as usize;
            let value = data[at + 2];
            at += 3;
            out.push((off as i128, vec![value; count]));
        } else {
            if at + size > data.len() {
                return Err(ModsError::modl(format!(
                    "the IPS file is cut short in a record at 0x{off:X}"
                )));
            }
            out.push((off as i128, data[at..at + size].to_vec()));
            at += size;
        }
    }
}

/// `re.sub(r'/\*.*?\*/', '', text, flags=re.S)`
fn strip_block_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("/*") {
        match rest[i + 2..].find("*/") {
            Some(j) => {
                out.push_str(&rest[..i]);
                rest = &rest[i + 2 + j + 2..];
            }
            None => break,
        }
    }
    out.push_str(rest);
    out
}

/// `parse_pchtxt`: the patch groups of an IPSwitch text patch, each with `.bid` (the `@nsobid-` hex), `.name` (the comment above the block, else
/// "patch N"), `.on` (`@enabled` or `@disabled`) and `.writes` `[(module offset, bytes)]`.
///
/// Supported: `@nsobid-<hex>`, `@flag offset_shift <n>`, `@enabled`, `@disabled`, `@stop` (ends that build's part; the next `@nsobid-` starts another), `//`
/// comments and `/* */` blocks, `<hex address> <hex bytes>` lines. Other `@` lines (`@title`, `@version`, other flags) are ignored. A quoted-string value is
/// refused (a `ModError`): IPSwitch's string form and its terminator are not pinned down well enough to write them into code blindly.
pub fn parse_pchtxt(text: &str, mod_name: &str, source: &str) -> Result<Vec<Patch>> {
    let mut patches: Vec<Patch> = Vec::new();
    let mut bid: Option<String> = None;
    let mut shift: i128 = 0;
    let mut comment = String::new();
    let mut cur: Option<usize> = None;
    let mut count = 0usize;
    let text = strip_block_comments(text);
    for raw in pyfmt::splitlines(&text) {
        let line = pyfmt::strip(raw);
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("//") {
            let note = pyfmt::strip(pyfmt::strip_chars(pyfmt::strip(rest), "-=*# "));
            if !note.is_empty() {
                comment = note.to_string();
            }
            continue;
        }
        let line = pyfmt::strip(line.split("//").next().unwrap_or(""));
        if let Some(after) = line.strip_prefix('@') {
            let word = pyfmt::split_ws_once(after);
            let key = word.first().map(|w| w.to_lowercase()).unwrap_or_default();
            if let Some(hex) = key.strip_prefix("nsobid-") {
                bid = Some(hex.to_uppercase());
                cur = None;
            } else if key == "flag" && word.len() > 1 {
                let parts = pyfmt::split_ws(word[1]);
                if parts.len() >= 2 && parts[0] == "offset_shift" {
                    match int_base0(parts[1]) {
                        Some(v) => shift = v,
                        None => {
                            return Err(ModsError::modl(format!(
                                "bad offset_shift {}",
                                repr(parts[1])
                            )));
                        }
                    }
                }
            } else if key == "enabled" || key == "disabled" {
                count += 1;
                let name = if comment.is_empty() {
                    format!("patch {count}")
                } else {
                    comment.clone()
                };
                patches.push(Patch {
                    mod_name: mod_name.into(),
                    source: source.into(),
                    name,
                    bid: bid.clone().unwrap_or_else(|| "None".into()),
                    on: key == "enabled",
                    writes: vec![],
                });
                cur = Some(patches.len() - 1);
                comment.clear();
            } else if key == "stop" {
                cur = None;
                bid = None;
            }
            continue;
        }
        if bid.is_none() {
            continue;
        }
        let parts = pyfmt::split_ws(line);
        if parts.len() < 2 {
            continue;
        }
        if line.contains('"') {
            return Err(ModsError::modl(
                "a quoted-string patch value (not supported)",
            ));
        }
        let addr = parts[0];
        let value: String = parts[1..].concat();
        let plain = addr.replace("0x", "").replace("0X", "");
        if !pyfmt::hex_match(&plain) || !pyfmt::hex_match(&value) || value.chars().count() % 2 == 1
        {
            continue; // prose that happens to sit between blocks
        }
        let idx = match cur {
            Some(i) => i,
            None => {
                // addresses before any @enabled: one implicit, enabled block
                count += 1;
                let name = if comment.is_empty() {
                    format!("patch {count}")
                } else {
                    comment.clone()
                };
                patches.push(Patch {
                    mod_name: mod_name.into(),
                    source: source.into(),
                    name,
                    bid: bid.clone().unwrap(),
                    on: true,
                    writes: vec![],
                });
                comment.clear();
                cur = Some(patches.len() - 1);
                patches.len() - 1
            }
        };
        let a = match int_base16(addr) {
            Some(a) => a,
            // `int('0x0x12', 16)`: HEX matched what was left after removing every `0x`, but the token itself is not a number
            None => {
                return Err(ModsError::unhandled(
                    "ValueError",
                    format!("invalid literal for int() with base 16: {}", repr(addr)),
                ));
            }
        };
        let bytes: Vec<u8> = (0..value.len() / 2)
            .map(|i| u8::from_str_radix(&value[2 * i..2 * i + 2], 16).unwrap())
            .collect();
        patches[idx]
            .writes
            .push((a + shift - NSO_HEADER as i128, bytes));
    }
    patches.retain(|p| !p.writes.is_empty());
    Ok(patches)
}

/// `bid_matches(wanted_hex, nso_bid)`: does a file's build id (at least 16 hex digits: cheat sites name theirs by the first 8 bytes, Atmosphere's own files
/// carry the whole id, a trailing run of zero bytes is dropped by some tools) name this NSO's build id (0x20 bytes)?
pub fn bid_matches(wanted_hex: &str, nso_bid: &[u8]) -> bool {
    let w = wanted_hex.to_uppercase();
    let w = w.trim_end_matches('0');
    let full = pyfmt::hex_upper(nso_bid);
    wanted_hex.chars().count() >= 16
        && pyfmt::hex_match(wanted_hex)
        && !w.is_empty()
        && full.starts_with(w)
}

pub fn stem_bid(filename: &str) -> Option<String> {
    let stem = pyfmt::splitext_stem(pyfmt::basename(filename));
    if stem.chars().count() >= 16 && pyfmt::hex_match(stem) {
        Some(stem.to_uppercase())
    } else {
        None
    }
}

/// A file that cannot be used: `(mod, file, why)`.
pub type Unusable = (String, String, String);

/// `exefs_patches(docs, tid, state)`: every patch of the ENABLED mods, in a fixed order (mod name, file name, block order), with the player's block choices
/// applied: the groups to apply, and `[(mod, file, why)]` for files that cannot be used.
pub fn exefs_patches(
    docs: &Path,
    tid: &str,
    state: Option<&ModState>,
) -> Result<(Vec<Patch>, Vec<Unusable>)> {
    let owned;
    let state = match state {
        Some(s) => s,
        None => {
            owned = read_state(&game_dir(docs, tid).join(STATE_NAME))?;
            &owned
        }
    };
    let (mut out, mut skipped) = (Vec::new(), Vec::new());
    for m in enabled_mods(docs, tid)? {
        let base = mods_dir(docs, tid).join(&m).join("exefs");
        let rels = if base.is_dir() { files(&base) } else { vec![] };
        for rel in rels {
            let path = base.join(&rel);
            let low = rel.to_lowercase();
            let data = fs::read(&path).map_err(|e| ModsError::Os(pyfmt::os_error(&e, &path)))?;
            let result: Result<()> = (|| {
                if low.ends_with(".ips") {
                    let Some(bid) = stem_bid(&rel) else {
                        return Err(ModsError::modl(
                            "an IPS file is applied by its build id and this name is not one",
                        ));
                    };
                    let writes = parse_ips(&data)?
                        .into_iter()
                        .map(|(o, d)| (o - NSO_HEADER as i128, d))
                        .collect();
                    out.push(Patch {
                        mod_name: m.clone(),
                        source: rel.clone(),
                        name: "ips".into(),
                        bid,
                        on: true,
                        writes,
                    });
                } else if low.ends_with(".pchtxt") {
                    for mut p in parse_pchtxt(&String::from_utf8_lossy(&data), &m, &rel)? {
                        p.on = state
                            .patch
                            .get(&(m.clone(), p.name.clone()))
                            .copied()
                            .unwrap_or(p.on);
                        out.push(p);
                    }
                }
                Ok(())
            })();
            match result {
                Ok(()) => {}
                Err(ModsError::Mod(why)) => skipped.push((m.clone(), rel.clone(), why)),
                Err(e) => return Err(e),
            }
        }
    }
    Ok((out, skipped))
}
