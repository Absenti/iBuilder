//! The input paths as the Python's `romfs.os` sees them: a `DMROMFS1` image or a folder is a tree (`os.walk`), any other file is a file (`os.path.isfile`: the main NSO).
//!
//! [`walk`] gives `os.walk(root)` top-down: each folder, then its sub-folders one by one; the files of a folder come as the Python iterates them (`for fn in sorted(fs)`).
//! An image lists its folders sorted (`romfs.py` refuses an unsorted index), and so are a real folder's here; Python's `os.walk` over a real folder takes them in the
//! file system's order, which only changes which path a duplicate shader is attributed to.

use crate::Fatal;
use ibuilder_shadersplit::image::{Entry, Loc, is_image, walk_root};
use ibuilder_shadersplit::source::Source;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// One file of a tree
#[derive(Clone)]
pub struct File {
    /// `os.path.relpath(path, root)`, `/`-separated
    pub rel: String,
    /// the file's name (`fn`)
    pub name: String,
    pub loc: Loc,
}

impl File {
    pub fn len(&self) -> u64 {
        self.loc.len()
    }
    pub fn is_empty(&self) -> bool {
        self.loc.len() == 0
    }
    /// `open(p, 'rb').read()`
    pub fn read(&self) -> std::io::Result<Vec<u8>> {
        let s = self.loc.open()?;
        s.read(0, s.len() as usize)
    }
    /// `open(p, 'rb').read(n)`
    pub fn head(&self, n: usize) -> std::io::Result<Vec<u8>> {
        self.loc.open()?.read(0, n)
    }
}

/// One folder of `os.walk`: its path below the root (`""` for the root) and its files, sorted by name
#[derive(Clone)]
pub struct Dir {
    pub rel: String,
    pub files: Vec<File>,
}

/// What an input path is
pub enum Root {
    /// a regular file that is not a RomFS image (the main NSO)
    File(PathBuf),
    /// a folder or a RomFS image
    Tree(Vec<Dir>),
    /// neither (`os.path.exists` is false)
    Missing,
}

/// `os.path.isfile(r)` / `os.walk(r)` of one input path
pub fn root(path: &Path) -> Result<Root, Fatal> {
    if path.is_dir() || (path.is_file() && is_image(path)) {
        let entries = walk_root(path).map_err(|e| Fatal(format!("[RED] {e}")))?;
        return Ok(Root::Tree(tree(entries)));
    }
    if path.is_file() {
        return Ok(Root::File(path.to_path_buf()));
    }
    Ok(Root::Missing)
}

/// `os.walk` of a tree, top-down
pub fn walk(path: &Path) -> Result<Vec<Dir>, Fatal> {
    match root(path)? {
        Root::Tree(t) => Ok(t),
        _ => Ok(Vec::new()),
    }
}

fn tree(entries: Vec<Entry>) -> Vec<Dir> {
    let mut files: BTreeMap<String, Vec<File>> = BTreeMap::new();
    let mut kids: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    files.entry(String::new()).or_default();
    for e in entries {
        let (dir, name) = match e.rel.rfind('/') {
            Some(i) => (e.rel[..i].to_string(), e.rel[i + 1..].to_string()),
            None => (String::new(), e.rel.clone()),
        };
        // every ancestor folder is a child of its parent
        let mut d = dir.clone();
        while !d.is_empty() {
            let (parent, me) = match d.rfind('/') {
                Some(i) => (d[..i].to_string(), d[i + 1..].to_string()),
                None => (String::new(), d.clone()),
            };
            kids.entry(parent.clone()).or_default().insert(me);
            files.entry(d.clone()).or_default();
            d = parent;
        }
        files.entry(dir).or_default().push(File {
            rel: e.rel,
            name,
            loc: e.loc,
        });
    }
    let mut out = Vec::new();
    let mut stack = vec![String::new()];
    while let Some(d) = stack.pop() {
        let mut fs = files.remove(&d).unwrap_or_default();
        fs.sort_by(|a, b| a.name.cmp(&b.name));
        if let Some(k) = kids.get(&d) {
            for name in k.iter().rev() {
                stack.push(if d.is_empty() {
                    name.clone()
                } else {
                    format!("{d}/{name}")
                });
            }
        }
        out.push(Dir { rel: d, files: fs });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_down_order() {
        let t = std::env::temp_dir().join(format!("glvk-walk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        for p in ["b.txt", "a/x.txt", "a/c/y.txt", "z/w.txt", "a.txt"] {
            let f = t.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, b"1").unwrap();
        }
        let order: Vec<String> = walk(&t)
            .unwrap()
            .iter()
            .flat_map(|d| d.files.iter().map(|f| f.rel.clone()))
            .collect();
        assert_eq!(order, ["a.txt", "b.txt", "a/x.txt", "a/c/y.txt", "z/w.txt"]);
        let _ = std::fs::remove_dir_all(&t);
    }
}
