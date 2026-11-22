//! The mesh-related parts of the build driver (round 3): the pair-level `mesh_finish` of `dmsh_build.py` (l.1080-1117: `<vertex key>_<geometry key>_m3`, `dmsh_mesh.compose`) and
//! the pixel shaders' SPH input maps a mesh shader's `partner_reads` needs.
//!
//! `mesh_finish(pair)` takes the vertex shader's `<vk>_b.metal`, the geometry shader's lean kernel `<gk>_bl3.metal` and its `<gk>_bl3.json` (`geometry.capture_reads`), composes the
//! mesh library and writes it under the library name `<vk>_<gk>_m3.metal` (iOS, source mode). A pair that lacks one of the three counts as `missing a library`, a pair
//! `dmsh_mesh.compose` refuses as `not composed` (`[RED] mesh <vk> <gk>: <message>`); the others are `built`.
use crate::cache::Stored;
use crate::jobs::{CTL_MAGIC, find, is_nvn2};
use crate::library::Source;
use crate::records::Packages;
use ibuilder_shader::finish::mesh::{MeshErr, compose};
use ibuilder_shader::finish::meshstage::pixel_reads;
use ibuilder_shader::finish::{Partners, SphReads};
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// What the finisher may ask of the paired shaders while finishing a mesh shader: the `user(locnN)` numbers of a finished pixel shader (the build keeps them) and the SPH input map of
/// its record (`dmsh_meshstage.pixel_reads`, read from the package when asked)
pub struct RecordPartners<'a> {
    pub packs: &'a Packages,
    /// shader key -> package name (the shader map's `loc`)
    pub package_of: &'a dyn Fn(&str) -> Option<String>,
    pub locs: &'a Mutex<HashMap<String, BTreeSet<u32>>>,
}

impl Partners for RecordPartners<'_> {
    fn locs(&self, ps: &str) -> Option<BTreeSet<u32>> {
        self.locs.lock().unwrap().get(ps).cloned()
    }

    /// `partner_reads`'s `if ps in loc: rec = rec_of(ps)[1]; ci = rec.find(...); code = rec[0x38:ci]; pixel_reads(code[:0x80] if is_nvn2(rec[ci:]) else code[0x30:0x80])`
    fn sph_reads(&self, ps: &str) -> SphReads {
        let Some(name) = (self.package_of)(ps) else {
            return SphReads::Reads(BTreeMap::new());
        };
        let rec = match self.packs.record(&name, ps) {
            Ok(r) => r,
            Err(e) => return SphReads::Failed(e),
        };
        let Some(ci) = find(&rec, &CTL_MAGIC, 0x38) else {
            return SphReads::Failed("the record holds no control section".into());
        };
        let code = &rec[0x38..ci];
        let sph: &[u8] = if is_nvn2(&rec[ci..]) {
            &code[..code.len().min(0x80)]
        } else {
            code.get(0x30..code.len().min(0x80)).unwrap_or(&[])
        };
        match pixel_reads(sph) {
            Ok(m) => SphReads::Reads(m),
            Err(e) => SphReads::Failed(e),
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct MeshStats {
    pub built: usize,
    pub missing: usize,
    pub not_composed: usize,
    /// an exception the Python does not catch (its build stops there): the pair gets no library
    pub crashed: usize,
    /// a library composed but not written to its file (a Rust-only outcome: the Python keeps it in memory): its own count, not `crashed`
    pub write_failed: usize,
    order: Vec<&'static str>,
}

impl MeshStats {
    fn bump(&mut self, k: &'static str) {
        if !self.order.contains(&k) {
            self.order.push(k);
        }
        match k {
            "built" => self.built += 1,
            "missing a library" => self.missing += 1,
            "not composed" => self.not_composed += 1,
            "failed to write" => self.write_failed += 1,
            _ => self.crashed += 1,
        }
    }

    fn get(&self, k: &str) -> usize {
        match k {
            "built" => self.built,
            "missing a library" => self.missing,
            "not composed" => self.not_composed,
            "failed to write" => self.write_failed,
            _ => self.crashed,
        }
    }

    /// the summary line (`'mesh libraries (<vertex>_<geometry>_m3, dmsh_mesh.py): ' + (', '.join(f'{k} {v}' ...) or 'none')`)
    pub fn line(&self) -> String {
        let body: Vec<String> = self
            .order
            .iter()
            .map(|k| format!("{k} {}", self.get(k)))
            .collect();
        format!(
            "mesh libraries (<vertex>_<geometry>_m3, dmsh_mesh.py): {}",
            if body.is_empty() {
                "none".to_string()
            } else {
                body.join(", ")
            }
        )
    }
}

enum One {
    Built(String, Vec<u8>),
    Missing,
    NotComposed(String),
    Crash(String),
}

fn mesh_one(stored: &HashMap<String, Arc<Stored>>, vk: &str, gk: &str) -> One {
    let read = |key: &str, name: String| -> Option<Vec<u8>> {
        stored
            .get(key)
            .and_then(|s| s.read(&name))
            .and_then(|r| r.ok())
    };
    let (Some(vs), Some(gs), Some(gj)) = (
        read(vk, format!("{vk}_b.metal")),
        read(gk, format!("{gk}_bl3.metal")),
        read(gk, format!("{gk}_bl3.json")),
    ) else {
        return One::Missing;
    };
    let (Ok(vs), Ok(gs)) = (String::from_utf8(vs), String::from_utf8(gs)) else {
        return One::Crash("UnicodeDecodeError".into());
    };
    let reads: Option<Vec<i64>> = serde_json::from_slice::<serde_json::Value>(&gj)
        .ok()
        .and_then(|j| {
            j.get("geometry")?
                .get("capture_reads")?
                .as_array()
                .map(|a| a.iter().filter_map(|x| x.as_i64()).collect())
        });
    let Some(reads) = reads else {
        return One::Crash("KeyError: 'capture_reads'".into());
    };
    match compose(&vs, &gs, &reads) {
        Ok(msl) => One::Built(format!("{vk}_{gk}_m3.metal"), msl.into_bytes()),
        Err(MeshErr::Value(e)) => One::NotComposed(e),
        Err(MeshErr::Crash(e)) => One::Crash(e),
    }
}

/// `mesh_finish` over every pair (sorted): the library files (written under `dir`, referenced as ranges so the image's writer streams them) and the counts
pub fn build_mesh_libs(
    stored: &HashMap<String, Arc<Stored>>,
    pairs: &BTreeSet<(String, String)>,
    dir: &Path,
    pool: &rayon::ThreadPool,
    log: &(dyn Fn(&str) + Sync),
) -> (Vec<(String, Source)>, MeshStats) {
    let mut stats = MeshStats::default();
    let mut files: Vec<(String, Source)> = Vec::new();
    if pairs.is_empty() {
        return (files, stats);
    }
    let _ = std::fs::create_dir_all(dir);
    let list: Vec<&(String, String)> = pairs.iter().collect();
    let results: Vec<One> = pool.install(|| {
        list.par_iter()
            .map(|(v, g)| mesh_one(stored, v, g))
            .collect()
    });
    for ((vk, gk), r) in list.iter().zip(results) {
        match r {
            One::Built(name, bytes) => {
                let path = dir.join(&name);
                match std::fs::write(&path, &bytes) {
                    Ok(()) => {
                        files.push((
                            name,
                            Source::Range {
                                path,
                                off: 0,
                                len: bytes.len() as u64,
                            },
                        ));
                        stats.bump("built");
                    }
                    Err(e) => {
                        log(&format!(
                            "[RED] mesh {vk} {gk}: could not write the library ({e})"
                        ));
                        stats.bump("failed to write");
                    }
                }
            }
            One::Missing => stats.bump("missing a library"),
            One::NotComposed(e) => {
                log(&format!("[RED] mesh {vk} {gk}: {e}"));
                stats.bump("not composed");
            }
            One::Crash(e) => {
                log(&format!(
                    "[RED] mesh {vk} {gk}: {e} (the Python's build stops here)"
                ));
                stats.bump("crashed");
            }
        }
    }
    (files, stats)
}

#[cfg(test)]
mod stats_tests {
    use super::*;

    #[test]
    fn a_library_not_written_has_its_own_count() {
        let mut st = MeshStats::default();
        st.bump("built");
        st.bump("crashed");
        st.bump("failed to write");
        // each outcome is counted in its own bucket: a write failure was counted as `crashed` (and "failed to write" printed the crash count)
        assert_eq!((st.built, st.crashed, st.write_failed), (1, 1, 1));
        assert_eq!(
            st.line(),
            "mesh libraries (<vertex>_<geometry>_m3, dmsh_mesh.py): built 1, crashed 1, failed to write 1"
        );
    }
}
