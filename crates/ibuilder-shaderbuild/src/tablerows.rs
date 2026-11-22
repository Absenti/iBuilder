//! `c1_guard.py` and `cb_extent.py` one shader at a time. Both tools answer from one shader's own files (`c1_guard.unproven`: its `.metal` and its `_b.json`;
//! `cb_extent.row`: its `_b.json` and `_b.metal`), so the answer can be computed when the shader's outputs exist, stored in the shader's cache entry, and the tables are
//! then only the rows put together in the library's name order: no second pass over a 1.7 GB image, no image held in memory, and a cache hit brings its rows with it.
//!
//! The analysis itself is `ibuilder_shader::tables::{unproven_in, cb_tokens}` (ported and proven identical to the Python tools); what is here is the file selection
//! of `c1_guard.main` / `cb_extent.main` (which names count, which side's libraries) and the table assembly. `tests/` checks the assembled tables against
//! `ibuilder_shader::tables::{c1_guard, cb_extent}` over the finished image.
use crate::cache::Meta;
use ibuilder_shader::tables::{cb_tokens, unproven_in};
use std::borrow::Cow;

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// `c1_guard.OUTPUT` fullmatch (`[0-9a-f]{16}_b\w*\.(metal|json)`) -> the extension
fn output_ext(name: &str) -> Option<&'static str> {
    let b = name.as_bytes();
    if b.len() < 18
        || !b[..16]
            .iter()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c))
        || !(b[16] == b'_' && b[17] == b'b')
    {
        return None;
    }
    let (stem, ext) = name[18..].rsplit_once('.')?;
    if !stem.bytes().all(is_word) {
        return None;
    }
    match ext {
        "metal" => Some("metal"),
        "json" => Some("json"),
        _ => None,
    }
}

/// The tables are made from the Mac side's folder, which in source mode holds none of the iOS-only files (`<key>_bx` / `_bf` / `_bv` / `_bp`)
fn mac_side(name: &str) -> bool {
    let stem = &name[18..name.rfind('.').unwrap_or(name.len())];
    !matches!(stem, "x" | "f" | "v" | "p")
}

/// `c1_guard.text_of`: UTF-8 with universal newlines
fn text_of(data: &[u8]) -> Option<Cow<'_, str>> {
    let s = std::str::from_utf8(data).ok()?;
    if s.contains('\r') {
        Some(Cow::Owned(s.replace("\r\n", "\n").replace('\r', "\n")))
    } else {
        Some(Cow::Borrowed(s))
    }
}

/// The rows one shader contributes to the two tables, from its library files (name -> bytes)
pub fn rows_of(files: &[(String, Vec<u8>)]) -> Meta {
    let find = |n: &str| files.iter().find(|f| f.0 == n).map(|f| f.1.as_slice());
    let mut c1 = Vec::new();
    let mut cb = None;
    for (name, bytes) in files {
        match output_ext(name) {
            Some("metal") if mac_side(name) => {
                let key = name
                    .strip_suffix("_b.metal")
                    .unwrap_or(&name[..name.len() - 6]);
                let reflection = find(&format!("{key}_b.json"));
                if let Some(src) = text_of(bytes)
                    && unproven_in(&src, reflection)
                {
                    c1.push(name.clone());
                }
            }
            Some("json") if name.ends_with("_b.json") => {
                let key = &name[..name.len() - 7];
                let metal = find(&format!("{key}_b.metal"));
                let answer = cb_tokens(Some(bytes), metal);
                if !answer.is_empty() {
                    cb = Some((key.to_string(), answer));
                }
            }
            _ => {}
        }
    }
    c1.sort();
    Meta {
        c1,
        cb,
        tables: true,
        locs: Vec::new(),
    }
}

/// `c1_full.tsv`'s key lines from every shader's unproven `.metal` names, in the library's name order
pub fn c1_lines(all: impl Iterator<Item = Vec<String>>) -> Vec<String> {
    let mut names: Vec<String> = all.flatten().collect();
    names.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    names
        .into_iter()
        .map(|n| {
            n.strip_suffix("_b.metal")
                .map(str::to_string)
                .unwrap_or_else(|| n[..n.len() - 6].to_string())
        })
        .collect()
}

/// `cb_extent.tsv`'s lines `key\ttokens` in key order (the order of the `_b.json` names)
pub fn cb_lines(all: impl Iterator<Item = Option<(String, String)>>) -> Vec<String> {
    let mut rows: Vec<(String, String)> = all.flatten().collect();
    rows.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    rows.into_iter().map(|(k, t)| format!("{k}\t{t}")).collect()
}

/// `shader_lib.table_head(lib)` + the stamp the tools write: `# chain b-<chain> inputs <digest> <iso time>`
pub fn table_head(chain: &str, inputs: &str, iso: &str) -> String {
    format!("# chain b-{chain} inputs {inputs} {iso}\n")
}

pub const CB_COMMENT: &str = "# shader key, then reg:bytes for each uniform slot whose reads stay inside its first bytes, and s<reg> for each storage slot it never writes (cb_extent.py)\n";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_filters() {
        assert_eq!(output_ext("0123456789abcdef_b.metal"), Some("metal"));
        assert_eq!(output_ext("0123456789abcdef_bg0.json"), Some("json"));
        assert_eq!(output_ext("0123456789abcdef_b.metallib"), None);
        assert_eq!(output_ext("c1_full.tsv"), None);
        assert!(!mac_side("0123456789abcdef_bx.metal"));
        assert!(mac_side("0123456789abcdef_bg0.metal"));
        assert!(mac_side("0123456789abcdef_b.json"));
    }

    #[test]
    fn line_order() {
        let v = c1_lines(
            vec![
                vec!["bb00000000000000_b.metal".to_string()],
                vec![
                    "aa00000000000000_bg0.metal".to_string(),
                    "aa00000000000000_b.metal".to_string(),
                ],
            ]
            .into_iter(),
        );
        assert_eq!(
            v,
            vec![
                "aa00000000000000",
                "aa00000000000000_bg0",
                "bb00000000000000"
            ]
        );
    }
}
