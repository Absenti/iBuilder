//! What package B needs for live shaders (`shader_device.live_package`): the key, the stage, the record format and the facts of a shader the host met in play with no translation.
//!
//! The Python imports `skey`, `glslc_stage`, `STAGE_NAME`, `nvn2_ctl` and `CTL_MAGIC` from `shader_split`; here they are the functions of [`crate::ctl`], re-exported, and
//! [`package_entry`] / [`package`] are `live_package` itself: each entry is checked as the split checks what it reads (its key is the host's, its stage is known, the record cannot be cut
//! wrongly) and written in the split's own record format to `<pkg_dir>/<st>_live.bin`, with its row in `<tables_dir>/live_map.tsv` and a compute shader's thread group in `live_facts.tsv`.

pub use crate::ctl::{
    CS_MAGIC, CTL_MAGIC, GFX_MAGIC, KeyRules, St, compute_facts, glslc_stage, nvn2_ctl, record,
    skey, stage_name,
};
use crate::memo::write_atomic;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

/// One checked live shader: its stage, its record in the package and, for a compute shader, its thread group and shared memory
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveEntry {
    pub stage: St,
    pub record: Vec<u8>,
    pub facts: Option<([u32; 3], u64)>,
}

/// The checks of `live_package` for one entry. `Err` is the Python's `bad[key]` text.
pub fn package_entry(
    rules: &KeyRules,
    key: &str,
    ctl: &[u8],
    code: &[u8],
) -> Result<LiveEntry, String> {
    if ctl.len() < 0x80 || code.len() < 0x10 {
        return Err(format!(
            "control section of {} bytes, code area of {}",
            ctl.len(),
            code.len()
        ));
    }
    let k = skey(rules, ctl, code).map_err(|e| e.to_string())?;
    if k != key {
        return Err(format!("its bytes give key {k}, not this one"));
    }
    let st = match glslc_stage(ctl, code).map_err(|e| e.to_string())? {
        Some(s) => s,
        None => return Err("stage not recognised (None)".to_string()),
    };
    let rec = record(key, ctl, code).map_err(|e| {
        // the Python's message without the key prefix `shader_split.main` puts on it
        e.split_once(": ").map(|x| x.1.to_string()).unwrap_or(e)
    })?;
    let facts = if st == St::Cs {
        Some(compute_facts(ctl).map_err(|e| {
            if e.starts_with("thread group is 0") {
                "compute shader with a thread group of 0".to_string()
            } else {
                e
            }
        })?)
    } else {
        None
    };
    Ok(LiveEntry {
        stage: st,
        record: rec,
        facts,
    })
}

/// `live_package(entries, ...)`: writes `<pkg_dir>/<st>_live.bin`, `<tables_dir>/live_map.tsv` and `live_facts.tsv`.
/// Returns `({key: stage name}, {key: why refused})`.
pub fn package(
    rules: &KeyRules,
    entries: &[(String, Vec<u8>, Vec<u8>)],
    pkg_dir: &Path,
    tables_dir: &Path,
) -> std::io::Result<(BTreeMap<String, String>, BTreeMap<String, String>)> {
    std::fs::create_dir_all(pkg_dir)?;
    std::fs::create_dir_all(tables_dir)?;
    let (mut ok, mut bad) = (BTreeMap::new(), BTreeMap::new());
    let mut recs: BTreeMap<String, Vec<Vec<u8>>> = BTreeMap::new();
    let mut rows: Vec<(String, usize, &'static str, String)> = Vec::new();
    let mut facts: BTreeMap<String, ([u32; 3], u64)> = BTreeMap::new();
    for (key, ctl, code) in entries {
        match package_entry(rules, key, ctl, code) {
            Err(why) => {
                bad.insert(key.clone(), why);
            }
            Ok(e) => {
                let name = format!("{}_live.bin", e.stage.short());
                let list = recs.entry(name.clone()).or_default();
                rows.push((name, list.len(), e.stage.name(), key.clone()));
                list.push(e.record);
                if let Some(f) = e.facts {
                    facts.insert(key.clone(), f);
                }
                ok.insert(key.clone(), e.stage.name().to_string());
            }
        }
    }
    for (name, rr) in &recs {
        write_atomic(&pkg_dir.join(name), |w| {
            rr.iter().try_for_each(|r| w.write_all(r))
        })?;
    }
    write_atomic(&tables_dir.join("live_map.tsv"), |f| {
        for (name, i, st, key) in &rows {
            writeln!(f, "{name}\t{i}\t{st}\t{key}\tlive")?;
        }
        Ok(())
    })?;
    write_atomic(&tables_dir.join("live_facts.tsv"), |f| {
        for (k, ([x, y, z], sh)) in &facts {
            writeln!(f, "{k}\t{{\"local\":[{x},{y},{z}],\"shared\":{sh}}}")?;
        }
        Ok(())
    })?;
    Ok((ok, bad))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(cs: bool) -> (Vec<u8>, Vec<u8>) {
        let mut ctl = vec![0u8; 0x850];
        ctl[..4].copy_from_slice(&CTL_MAGIC);
        ctl[0x20..0x24].copy_from_slice(&0x850u32.to_le_bytes());
        ctl[0x704..0x708].copy_from_slice(&0x40u32.to_le_bytes());
        let mut code = vec![3u8; 0x40];
        if cs {
            code[..4].copy_from_slice(&CS_MAGIC);
            ctl[0x748..0x754].copy_from_slice(&[32, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0]);
            ctl[0x754..0x758].copy_from_slice(&17u32.to_le_bytes());
        } else {
            code[..4].copy_from_slice(&GFX_MAGIC);
            code[0x30..0x34].copy_from_slice(&0x461u32.to_le_bytes());
        }
        (ctl, code)
    }

    #[test]
    fn an_entry_is_checked_as_the_python_checks_it() {
        let rules = KeyRules::default();
        let (ctl, code) = pair(false);
        let key = skey(&rules, &ctl, &code).unwrap();
        let e = package_entry(&rules, &key, &ctl, &code).unwrap();
        assert_eq!((e.stage, e.facts), (St::Vs, None));
        assert_eq!(e.record, record(&key, &ctl, &code).unwrap());
        assert_eq!(
            package_entry(&rules, "0000000000000000", &ctl, &code).unwrap_err(),
            format!("its bytes give key {key}, not this one")
        );
        assert_eq!(
            package_entry(&rules, &key, &ctl[..0x7f], &code).unwrap_err(),
            "control section of 127 bytes, code area of 64"
        );
        let (cctl, ccode) = pair(true);
        let ckey = skey(&rules, &cctl, &ccode).unwrap();
        let e = package_entry(&rules, &ckey, &cctl, &ccode).unwrap();
        assert_eq!((e.stage, e.facts), (St::Cs, Some(([32, 1, 1], 32))));
    }

    #[test]
    fn the_files_are_the_split_s_format() {
        let rules = KeyRules::default();
        let dir =
            std::env::temp_dir().join(format!("ibuilder-shadersplit-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (a, b) = (pair(false), pair(true));
        let entries = vec![
            (skey(&rules, &a.0, &a.1).unwrap(), a.0.clone(), a.1.clone()),
            (skey(&rules, &b.0, &b.1).unwrap(), b.0.clone(), b.1.clone()),
            ("1".repeat(16), a.0.clone(), a.1.clone()),
        ];
        let (ok, bad) = package(&rules, &entries, &dir.join("pkg"), &dir.join("tables")).unwrap();
        assert_eq!(ok.len(), 2);
        assert_eq!(bad.len(), 1);
        let map = std::fs::read_to_string(dir.join("tables/live_map.tsv")).unwrap();
        assert_eq!(map.lines().count(), 2);
        assert!(
            map.lines()
                .next()
                .unwrap()
                .starts_with("vs_live.bin\t0\tvertex\t")
        );
        assert!(map.lines().nth(1).unwrap().ends_with("\tlive"));
        assert_eq!(
            std::fs::read_to_string(dir.join("tables/live_facts.tsv"))
                .unwrap()
                .trim_end()
                .split('\t')
                .nth(1)
                .unwrap(),
            "{\"local\":[32,1,1],\"shared\":32}"
        );
        assert_eq!(
            std::fs::read(dir.join("pkg/vs_live.bin")).unwrap().len(),
            0x38 + 0x40 + 0x850
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// every file under `dir`, as (path relative to it, bytes)
    fn tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            for (n, b) in if p.is_dir() {
                tree(&p)
            } else {
                vec![(String::new(), std::fs::read(&p).unwrap())]
            } {
                out.push((
                    format!(
                        "{}{}{n}",
                        p.file_name().unwrap().to_string_lossy(),
                        if n.is_empty() { "" } else { "/" }
                    ),
                    b,
                ));
            }
        }
        out.sort();
        out
    }

    #[test]
    fn a_write_that_fails_leaves_the_old_file_and_no_part_behind() {
        // review r2 N7: a file a later step reads is replaced whole or not at all. The failure is made with a DIRECTORY named like the `.part` the file is written to (it fails for a root too)
        let rules = KeyRules::default();
        let (a, b) = (pair(false), pair(true));
        let (mut a2, mut b2) = (pair(false), pair(true));
        a2.1[0x3f] = 9; // other bytes, other keys: every file the second package writes differs from the first one's
        b2.1[0x3f] = 9;
        let entry =
            |p: &(Vec<u8>, Vec<u8>)| (skey(&rules, &p.0, &p.1).unwrap(), p.0.clone(), p.1.clone());
        for blocked in [
            "pkg/vs_live.bin",
            "tables/live_map.tsv",
            "tables/live_facts.tsv",
        ] {
            let dir = std::env::temp_dir().join(format!(
                "ibuilder-shadersplit-live-part-{}-{}",
                std::process::id(),
                blocked.replace('/', "_")
            ));
            let _ = std::fs::remove_dir_all(&dir);
            let (pkg, tables) = (dir.join("pkg"), dir.join("tables"));
            package(&rules, &[entry(&a), entry(&b)], &pkg, &tables).unwrap();
            let old = tree(&dir);
            let part = dir.join(format!("{blocked}.part"));
            std::fs::create_dir(&part).unwrap();
            assert!(
                package(
                    &rules,
                    &[entry(&a), entry(&a2), entry(&b), entry(&b2)],
                    &pkg,
                    &tables
                )
                .is_err(),
                "{blocked}"
            );
            // the file that failed is as it was, and nothing named `.part` is left but the directory that was put in the way
            let now = tree(&dir);
            let before = old.iter().find(|(n, _)| n == blocked).unwrap();
            assert!(now.contains(before), "{blocked} was changed");
            assert!(
                now.iter().all(|(n, _)| !n.ends_with(".part")),
                "{:?}",
                now.iter().map(|x| &x.0).collect::<Vec<_>>()
            );
            std::fs::remove_dir(&part).unwrap();
            // and without the obstacle the same call writes the new files, whole
            package(
                &rules,
                &[entry(&a), entry(&a2), entry(&b), entry(&b2)],
                &pkg,
                &tables,
            )
            .unwrap();
            assert_eq!(
                std::fs::read(pkg.join("vs_live.bin")).unwrap().len(),
                2 * (0x38 + 0x40 + 0x850)
            );
            assert!(tree(&dir).iter().all(|(n, _)| !n.ends_with(".part")));
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
