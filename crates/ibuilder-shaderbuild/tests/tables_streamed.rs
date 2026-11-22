//! The tables assembled from per-shader rows (`tablerows`) against the whole-image functions proved identical to `c1_guard.py` / `cb_extent.py`
//! (`ibuilder_shader::tables::{c1_guard, cb_extent}`), over a Python library image: 2,459 shaders of Grizzy (ps, vs, cs, gs) in `s1800-cap`.
mod common;
use common::*;
use ibuilder_shader::romfs::Image;
use ibuilder_shader::tables::{c1_guard, cb_extent};
use ibuilder_shaderbuild::tablerows::{c1_lines, cb_lines, rows_of};

#[test]
fn per_shader_rows_assemble_to_the_whole_image_tables() {
    let p = old("shader/runs/s1800-cap/b-b24b25240cad.img");
    if !have(&p) {
        return;
    }
    let data = std::fs::read(&p).unwrap();
    let img = Image::open(&data).unwrap();
    // group the files by shader (the first 16 characters of a library name)
    let mut metas = Vec::new();
    let mut i = 0;
    while i < img.files.len() {
        let key = img.files[i].name.get(..16).unwrap_or("").to_string();
        let mut files = Vec::new();
        while i < img.files.len() && img.files[i].name.get(..16).unwrap_or("") == key {
            files.push((img.files[i].name.clone(), img.bytes(&img.files[i]).to_vec()));
            i += 1;
        }
        if key.len() == 16 && key.bytes().all(|b| b.is_ascii_hexdigit()) {
            metas.push(rows_of(&files));
        }
    }
    let mine_c1 = c1_lines(metas.iter().map(|m| m.c1.clone()));
    let mine_cb = cb_lines(metas.iter().map(|m| m.cb.clone()));
    let want_c1 = c1_guard(&img);
    let want_cb = cb_extent(&img);
    assert_eq!(mine_c1, want_c1);
    assert_eq!(mine_cb, want_cb);
    eprintln!(
        "{} shaders: {} c1 rows and {} cb rows, equal to ibuilder_shader::tables over the whole image",
        metas.len(),
        mine_c1.len(),
        mine_cb.len()
    );
    assert!(
        !mine_c1.is_empty() && !mine_cb.is_empty(),
        "the sample must exercise both tables"
    );
}
