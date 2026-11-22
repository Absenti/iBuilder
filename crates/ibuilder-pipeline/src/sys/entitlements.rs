//! This app's own signature, read out of its own Mach-O: the CodeDirectory identifier (the framework is signed *as this app*, docs/SIGN-IN-APP.md §2) and the entitlements
//! blob (is Extended Virtual Addressing on? the increased-memory-limit entitlement? `get-task-allow`?). Port of `read_code_directory`, `self_identifier`, `extended_va_init`
//! of `gameapp_shim.c`. An entitlement cannot be switched on from inside the app: it is part of the signature the installer wrote, and iOS starts the app only if the
//! provisioning profile allows it, so the app READS it (DECISIONS trap 44).
//!
//! The parser is plain safe Rust over bytes (it lives in `sys` only because it reads the process's own executable path).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::OnceLock;

const FAT_MAGIC: u32 = 0xcafe_babe;
const FAT_MAGIC_64: u32 = 0xcafe_babf;
const MH_MAGIC_64: u32 = 0xfeed_facf;
const CPU_TYPE_ARM64: u32 = 0x0100_000c;
const LC_CODE_SIGNATURE: u32 = 0x1d;
const CSMAGIC_EMBEDDED_SIGNATURE: u32 = 0xfade_0cc0;
const CSMAGIC_CODEDIRECTORY: u32 = 0xfade_0c02;
const CSMAGIC_EMBEDDED_ENTITLEMENTS: u32 = 0xfade_7171;
const CSSLOT_CODEDIRECTORY: u32 = 0;
const CSSLOT_ENTITLEMENTS: u32 = 5;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CodeSignature {
    pub identifier: String,
    pub flags: u32,
    /// the XML entitlements blob, empty when none
    pub entitlements: String,
    pub extended_va: bool,
    pub increased_memory: bool,
    pub get_task_allow: bool,
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// `plist_key_true`: `<key>NAME</key>` followed (after whitespace) by `<true/>`.
pub fn plist_key_true(xml: &str, key: &str) -> bool {
    let want = format!("<key>{key}</key>");
    let Some(p) = xml.find(&want) else {
        return false;
    };
    xml[p + want.len()..].trim_start().starts_with("<true/>")
}

/// Parse a CodeSignature SuperBlob (what `LC_CODE_SIGNATURE` points at).
pub fn parse_super_blob(sb: &[u8]) -> Option<CodeSignature> {
    if be32(sb, 0)? != CSMAGIC_EMBEDDED_SIGNATURE {
        return None;
    }
    let count = be32(sb, 8)? as usize;
    let mut out = CodeSignature::default();
    let mut found_cd = false;
    for i in 0..count {
        let at = 12 + 8 * i;
        let (ty, off) = (be32(sb, at)?, be32(sb, at + 4)? as usize);
        match ty {
            CSSLOT_ENTITLEMENTS if be32(sb, off) == Some(CSMAGIC_EMBEDDED_ENTITLEMENTS) => {
                let len = be32(sb, off + 4)? as usize;
                if len >= 8 {
                    let body = sb.get(off + 8..off + len)?;
                    out.entitlements =
                        String::from_utf8_lossy(&body[..body.len().min(16383)]).into_owned();
                }
            }
            CSSLOT_CODEDIRECTORY => {
                if be32(sb, off)? != CSMAGIC_CODEDIRECTORY {
                    return None;
                }
                out.flags = be32(sb, off + 12)?;
                let ident_off = be32(sb, off + 20)? as usize;
                let start = off + ident_off;
                let tail = sb.get(start..)?;
                let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
                out.identifier = String::from_utf8_lossy(&tail[..end]).into_owned();
                found_cd = true;
            }
            _ => {}
        }
    }
    if !found_cd {
        return None;
    }
    out.extended_va = plist_key_true(
        &out.entitlements,
        "com.apple.developer.kernel.extended-virtual-addressing",
    );
    out.increased_memory = plist_key_true(
        &out.entitlements,
        "com.apple.developer.kernel.increased-memory-limit",
    );
    out.get_task_allow = plist_key_true(&out.entitlements, "get-task-allow");
    Some(out)
}

/// `read_code_directory(path)`: the signature of the executable at `path` (thin arm64, or the arm64 slice of a fat file).
pub fn read_code_signature(path: &Path) -> Option<CodeSignature> {
    let mut f = File::open(path).ok()?;
    let mut head = [0u8; 8];
    f.read_exact(&mut head).ok()?;
    let mut base = 0u64;
    if be32(&head, 0)? == FAT_MAGIC || be32(&head, 0)? == FAT_MAGIC_64 {
        let is64 = be32(&head, 0)? == FAT_MAGIC_64;
        let n = be32(&head, 4)? as usize;
        let esz = if is64 { 32 } else { 20 };
        let mut arches = vec![0u8; n.min(64) * esz];
        f.read_exact(&mut arches).ok()?;
        for i in 0..n.min(64) {
            let a = &arches[i * esz..];
            if be32(a, 0)? == CPU_TYPE_ARM64 {
                base = if is64 {
                    ((be32(a, 8)? as u64) << 32) | be32(a, 12)? as u64
                } else {
                    be32(a, 8)? as u64
                };
                break;
            }
        }
    }
    f.seek(SeekFrom::Start(base)).ok()?;
    let mut mh = [0u8; 32];
    f.read_exact(&mut mh).ok()?;
    if le32(&mh, 0)? != MH_MAGIC_64 {
        return None;
    }
    let (ncmds, sizeofcmds) = (le32(&mh, 16)? as usize, le32(&mh, 20)? as usize);
    let mut cmds = vec![0u8; sizeofcmds.min(4 << 20)];
    f.read_exact(&mut cmds).ok()?;
    let mut p = 0usize;
    for _ in 0..ncmds {
        let (cmd, size) = (le32(&cmds, p)?, le32(&cmds, p + 4)? as usize);
        if size == 0 {
            break;
        }
        if cmd == LC_CODE_SIGNATURE {
            let (dataoff, datasize) = (le32(&cmds, p + 8)? as u64, le32(&cmds, p + 12)? as usize);
            f.seek(SeekFrom::Start(base + dataoff)).ok()?;
            let mut blob = vec![0u8; datasize.min(8 << 20)];
            f.read_exact(&mut blob).ok()?;
            return parse_super_blob(&blob);
        }
        p += size;
    }
    None
}

/// This executable's own signature, read once.
pub fn own_entitlements() -> Option<CodeSignature> {
    static SIG: OnceLock<Option<CodeSignature>> = OnceLock::new();
    SIG.get_or_init(|| read_code_signature(&std::env::current_exe().ok()?))
        .clone()
}

/// `self_identifier()`: this app's CodeDirectory identifier, or the safe fallback.
pub fn self_identifier() -> String {
    own_entitlements()
        .map(|s| s.identifier)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "dev.ibuilder.app".to_string())
}

#[cfg(test)]
pub(crate) fn build_test_macho(identifier: &str, entitlements: &str) -> Vec<u8> {
    // a SuperBlob with a CodeDirectory (identifier only) and an entitlements blob, inside a minimal arm64 Mach-O with one LC_CODE_SIGNATURE
    let mut cd = vec![0u8; 44];
    cd[0..4].copy_from_slice(&CSMAGIC_CODEDIRECTORY.to_be_bytes());
    cd[12..16].copy_from_slice(&0x0002_0002u32.to_be_bytes()); // flags
    cd[20..24].copy_from_slice(&44u32.to_be_bytes()); // identOffset
    cd.extend_from_slice(identifier.as_bytes());
    cd.push(0);
    let cd_len = cd.len() as u32;
    cd[4..8].copy_from_slice(&cd_len.to_be_bytes());
    let mut ent = Vec::new();
    ent.extend_from_slice(&CSMAGIC_EMBEDDED_ENTITLEMENTS.to_be_bytes());
    ent.extend_from_slice(&((entitlements.len() + 8) as u32).to_be_bytes());
    ent.extend_from_slice(entitlements.as_bytes());
    let header = 12 + 16;
    let mut sb = Vec::new();
    sb.extend_from_slice(&CSMAGIC_EMBEDDED_SIGNATURE.to_be_bytes());
    sb.extend_from_slice(&0u32.to_be_bytes());
    sb.extend_from_slice(&2u32.to_be_bytes());
    sb.extend_from_slice(&CSSLOT_CODEDIRECTORY.to_be_bytes());
    sb.extend_from_slice(&(header as u32).to_be_bytes());
    sb.extend_from_slice(&CSSLOT_ENTITLEMENTS.to_be_bytes());
    sb.extend_from_slice(&((header + cd.len()) as u32).to_be_bytes());
    sb.extend_from_slice(&cd);
    sb.extend_from_slice(&ent);
    let total = (sb.len()) as u32;
    sb[4..8].copy_from_slice(&total.to_be_bytes());
    let mut m = vec![0u8; 32];
    m[0..4].copy_from_slice(&MH_MAGIC_64.to_le_bytes());
    m[4..8].copy_from_slice(&CPU_TYPE_ARM64.to_le_bytes());
    m[16..20].copy_from_slice(&1u32.to_le_bytes()); // ncmds
    m[20..24].copy_from_slice(&16u32.to_le_bytes()); // sizeofcmds
    let mut lc = Vec::new();
    lc.extend_from_slice(&LC_CODE_SIGNATURE.to_le_bytes());
    lc.extend_from_slice(&16u32.to_le_bytes());
    lc.extend_from_slice(&(32u32 + 16 + 16).to_le_bytes()); // dataoff (after a 16-byte pad)
    lc.extend_from_slice(&(sb.len() as u32).to_le_bytes());
    m.extend_from_slice(&lc);
    m.extend_from_slice(&[0u8; 16]);
    m.extend_from_slice(&sb);
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{TempDir, write};

    const EA: &str = "<?xml version=\"1.0\"?><plist><dict><key>get-task-allow</key>\n<true/>\n<key>com.apple.developer.kernel.extended-virtual-addressing</key><true/><key>com.apple.developer.kernel.increased-memory-limit</key><false/></dict></plist>";

    #[test]
    fn plist_key_true_reads_the_value_after_the_key() {
        assert!(plist_key_true(EA, "get-task-allow"));
        assert!(plist_key_true(
            EA,
            "com.apple.developer.kernel.extended-virtual-addressing"
        ));
        assert!(!plist_key_true(
            EA,
            "com.apple.developer.kernel.increased-memory-limit"
        ));
        assert!(!plist_key_true(EA, "absent"));
    }

    #[test]
    fn reads_identifier_flags_and_entitlements_of_a_thin_macho() {
        let d = TempDir::new("ent");
        let p = d.join("app");
        write(&p, build_test_macho("dev.ibuilder.app.ABCDE12345", EA));
        let s = read_code_signature(&p).unwrap();
        assert_eq!(s.identifier, "dev.ibuilder.app.ABCDE12345");
        assert_eq!(s.flags, 0x0002_0002);
        assert!(s.extended_va && s.get_task_allow && !s.increased_memory);
        assert!(read_code_signature(&d.join("missing")).is_none());
        write(d.join("junk"), b"not a macho file at all");
        assert!(read_code_signature(&d.join("junk")).is_none());
    }

    #[test]
    fn reads_the_arm64_slice_of_a_fat_file() {
        let thin = build_test_macho("fat.id", "");
        let mut fat = Vec::new();
        fat.extend_from_slice(&FAT_MAGIC.to_be_bytes());
        fat.extend_from_slice(&2u32.to_be_bytes());
        // first arch: x86_64 (ignored), second: arm64 at offset 256
        for (cpu, off) in [(0x0100_0007u32, 128u32), (CPU_TYPE_ARM64, 256u32)] {
            fat.extend_from_slice(&cpu.to_be_bytes());
            fat.extend_from_slice(&0u32.to_be_bytes());
            fat.extend_from_slice(&off.to_be_bytes());
            fat.extend_from_slice(&(thin.len() as u32).to_be_bytes());
            fat.extend_from_slice(&0u32.to_be_bytes());
        }
        fat.resize(256, 0);
        fat.extend_from_slice(&thin);
        let d = TempDir::new("ent-fat");
        write(d.join("fat"), &fat);
        assert_eq!(
            read_code_signature(&d.join("fat")).unwrap().identifier,
            "fat.id"
        );
    }

    #[test]
    fn this_test_binary_reads_without_panicking() {
        // an ad-hoc signed Rust test binary on macOS/arm64: a signature exists, with no entitlements; elsewhere it may be None
        let _ = own_entitlements();
        assert!(!self_identifier().is_empty());
    }
}
