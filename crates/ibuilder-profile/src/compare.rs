//! `gameapp/compare_profile.py`: compare a profile the app derived in-process against the one the Mac-side
//! orchestrator wrote, field by field.
//!
//! Reports fields EQUAL / DIFFERENT / NOT DERIVED (a key the orchestrator's file has that the app's derivation
//! cannot produce) / EXTRA. Keys may repeat (`build`, `display_version`), so every row of a key is kept; comments
//! are provenance and are not compared. The output text is the Python's, including its `%r` spellings.
use crate::error::Result;
use crate::pyutil::{py_lstrip, py_repr, py_strip};
use std::path::Path;

/// `{key: [row, ...]}` in first-seen key order; a row is its value fields, tab-split.
pub type Rows = Vec<(String, Vec<Vec<String>>)>;

pub fn rows_of(text: &str) -> Rows {
    let norm = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut out: Rows = Vec::new();
    for s in norm.split('\n') {
        if py_strip(s).is_empty() || py_lstrip(s).starts_with('#') {
            continue;
        }
        let c: Vec<&str> = s.split('\t').collect();
        let row: Vec<String> = c[1..].iter().map(|x| x.to_string()).collect();
        match out.iter_mut().find(|(k, _)| k == c[0]) {
            Some(slot) => slot.1.push(row),
            None => out.push((c[0].to_string(), vec![row])),
        }
    }
    out
}

pub fn rows(path: &Path) -> Result<Rows> {
    let text = String::from_utf8(std::fs::read(path)?)
        .map_err(|e| crate::Error::Format(format!("{}: {e}", path.display())))?;
    Ok(rows_of(&text))
}

fn get<'a>(r: &'a Rows, k: &str) -> Option<&'a Vec<Vec<String>>> {
    r.iter().find(|(kk, _)| kk == k).map(|(_, v)| v)
}

fn sorted(v: &[Vec<String>]) -> Vec<Vec<String>> {
    let mut s = v.to_vec();
    s.sort();
    s
}

/// Python's repr of a list of tuples of str.
fn repr_rows(v: &[Vec<String>]) -> String {
    let t = |r: &Vec<String>| match r.len() {
        0 => "()".to_string(),
        1 => format!("({},)", py_repr(&r[0])),
        _ => format!(
            "({})",
            r.iter().map(|x| py_repr(x)).collect::<Vec<_>>().join(", ")
        ),
    };
    format!("[{}]", v.iter().map(t).collect::<Vec<_>>().join(", "))
}

#[derive(Debug, Default)]
pub struct Comparison {
    pub equal: Vec<String>,
    pub different: Vec<String>,
    pub not_derived: Vec<String>,
    pub extra: Vec<String>,
    pub text: String,
}

/// `compare(label, a, b)`: `a` = the app's rows, `b` = the orchestrator's. Returns the printed text and the lists.
pub fn compare(label: &str, a: &Rows, b: &Rows) -> Comparison {
    let mut equal = Vec::new();
    let mut different = Vec::new();
    let mut not_derived = Vec::new();
    for (k, bv) in b {
        match get(a, k) {
            Some(av) if sorted(av) == sorted(bv) => equal.push(k.clone()),
            Some(_) => different.push(k.clone()),
            None => not_derived.push(k.clone()),
        }
    }
    let extra: Vec<String> = a
        .iter()
        .filter(|(k, _)| get(b, k).is_none())
        .map(|(k, _)| k.clone())
        .collect();
    let mut t = String::new();
    t.push_str(&format!("== {label} ==\n"));
    t.push_str(&format!("fields equal        : {}\n", equal.len()));
    let mut eq_sorted = equal.clone();
    eq_sorted.sort();
    t.push_str(&format!("   {}\n", eq_sorted.join(", ")));
    t.push_str(&format!("fields different    : {}\n", different.len()));
    for k in &different {
        t.push_str(&format!(
            "   {k}: app {} / orchestrator {}\n",
            repr_rows(get(a, k).unwrap()),
            repr_rows(get(b, k).unwrap())
        ));
    }
    t.push_str(&format!("fields not derivable: {}\n", not_derived.len()));
    for k in &not_derived {
        t.push_str(&format!(
            "   {k} (orchestrator: {})\n",
            repr_rows(get(b, k).unwrap())
        ));
    }
    let mut ex_sorted = extra.clone();
    ex_sorted.sort();
    t.push_str(&format!(
        "fields extra (app)  : {} {}\n",
        extra.len(),
        if extra.is_empty() {
            String::new()
        } else {
            format!(
                "[{}]",
                ex_sorted
                    .iter()
                    .map(|x| py_repr(x))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    ));
    Comparison {
        equal,
        different,
        not_derived,
        extra,
        text: t,
    }
}

/// `main(argv)`: `<app game.tsv> <orchestrator game.tsv> [<app profile.tsv> <harness profile.tsv>]`; returns the
/// text the script prints.
pub fn main_text(argv: &[String]) -> Result<String> {
    let (app, orch) = (rows(Path::new(&argv[0]))?, rows(Path::new(&argv[1]))?);
    let mut out = compare(
        &format!("game.tsv data rows -- {} vs {}", argv[0], argv[1]),
        &app,
        &orch,
    )
    .text;
    if argv.len() >= 4 {
        out.push('\n');
        let (a, b) = (rows(Path::new(&argv[2]))?, rows(Path::new(&argv[3]))?);
        let c = compare(
            &format!(
                "profile.tsv data rows (what the host read) -- {} vs {}",
                argv[2], argv[3]
            ),
            &a,
            &b,
        );
        out.push_str(&c.text);
        if c.different.is_empty() && c.not_derived.is_empty() {
            out.push_str("   the boot read byte-identical data rows\n");
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compare_lists() {
        let a = rows_of("# c\nname\tX\nbuild\tv0\t1\nbuild\tv1\t2\nonly_app\t1\nsame\t5\n");
        let b = rows_of("name\tY\nbuild\tv1\t2\nbuild\tv0\t1\nsame\t5\nnot_there\t7\t8\n");
        let c = compare("t", &a, &b);
        assert_eq!(c.equal, vec!["build", "same"]);
        assert_eq!(c.different, vec!["name"]);
        assert_eq!(c.not_derived, vec!["not_there"]);
        assert_eq!(c.extra, vec!["only_app"]);
        assert!(
            c.text
                .contains("   name: app [('X',)] / orchestrator [('Y',)]\n")
        );
        assert!(
            c.text
                .contains("   not_there (orchestrator: [('7', '8')])\n")
        );
        assert!(c.text.ends_with("fields extra (app)  : 1 ['only_app']\n"));
    }
}
