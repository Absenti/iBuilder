//! Oracle tests: the Rust output against the Python's finished files on the same inputs. Fixtures are read-only paths of the iOSBuilder tree and of the scratch folder
//! (`SHADER_SCRATCH`, default `$IBUILDER_SCRATCH_OLD/shader`); a missing fixture skips the test with a message, it does not fail.
use ibuilder_shader::{PostError, oracle::*, romfs::Image, tables};
use std::path::Path;

use ibuilder_testkit::IOSBUILDER;

fn scratch() -> String {
    std::env::var("SHADER_SCRATCH")
        .unwrap_or_else(|_| format!("{}/shader", ibuilder_testkit::SCRATCH_OLD))
}
fn have(p: &str) -> bool {
    if Path::new(p).exists() {
        true
    } else {
        ibuilder_testkit::skip(&format!("fixture {p} is absent"));
        false
    }
}

fn mmap(p: &str) -> memmap2::Mmap {
    let f = std::fs::File::open(p).unwrap();
    unsafe { memmap2::Mmap::map(&f).unwrap() }
} // SAFETY: read-only fixture

fn verify(cases: &[Case], img: &Image, profile: &str, max_unported: usize) {
    let pairs = load_pairs(profile);
    let expected = |k: &str| img.find(&format!("{k}_b.metal")).map(|f| img.bytes(f));
    let partner =
        |ps: &str, _o: &Outputs| expected(ps).map(|b| String::from_utf8_lossy(b).into_owned());
    let (out, _, _) = run_cases(cases, &pairs, &partner);
    let (mut same, mut unported) = (0usize, 0usize);
    for c in cases {
        match &out[&c.key] {
            Ok(s) => {
                assert_eq!(
                    expected(&c.key),
                    Some(s.as_bytes()),
                    "{}: the Rust output differs from the Python's",
                    c.key
                );
                same += 1;
            }
            Err(PostError::Unported(w)) => {
                assert_eq!(*w, "stage", "{}: a step is still gated", c.key);
                unported += 1
            }
            Err(e) => panic!("{}: {e}", c.key),
        }
    }
    eprintln!(
        "{same} identical, {unported} gated (unported) of {}",
        cases.len()
    );
    assert!(
        unported <= max_unported,
        "{unported} shaders hit an unported step"
    );
    assert!(same > 0);
}

/// the 53 jobs `tools/shader_post.py` was proven on (docs/SHADER-POST.md): 40 ps, 10 cs, 3 vs, against the engine's own library image
#[test]
fn manifest_53_jobs_identical_to_the_engines_own_output() {
    let m = format!("{IOSBUILDER}/out-py2/manifest");
    let i = format!("{IOSBUILDER}/out-py2/capture-grizzy/refout/b-19d183a2b462.img");
    if !have(&m) || !have(&i) {
        return;
    }
    let map = mmap(&i);
    let img = Image::open(&map[..]).unwrap();
    let cases = load_manifest(&m);
    assert_eq!(cases.len(), 53);
    verify(&cases, &img, &format!("{m}/profile"), 0);
}

/// a random sample of Grizzy's shaders (instrumented dmsh_build.py capture + the library it built)
#[test]
fn grizzy_sample_identical_to_the_python_build() {
    let (cap, img) = (
        format!("{}/cap-s1800", scratch()),
        format!("{}/runs/s1800-cap/b-b24b25240cad.img", scratch()),
    );
    if !have(&cap) || !have(&img) {
        return;
    }
    let map = mmap(&img);
    let image = Image::open(&map[..]).unwrap();
    let cases = load_capture(&cap);
    verify(
        &cases,
        &image,
        &format!("{}/profile/0100DFA027504000", scratch()),
        cases.len() / 4,
    );
}

/// every ps / vs / cs of the whole Grizzy build (21,990 shaders: the instrumented Python build's captured inputs, `tools/measure/make_prof_engine.py` with DMSH_CAPTURE, and the library the
/// Python built): the chain with no gate left, 0 differences; only the 145 geometry shaders are another stage. Needs the capture (about 830 MB) and the full library image.
#[test]
#[ignore = "21,990 shaders, 650 MB of MSL, about 20 s; needs the full capture of a Python build"]
fn full_grizzy_chain_identical_to_the_python_build() {
    let cap = std::env::var("SHADER_FULL_CAPTURE")
        .unwrap_or_else(|_| format!("{}/f1/cap-full", ibuilder_testkit::SCRATCH));
    let img = format!("{}/runs/full-gil-j6/b-b24b25240cad.img", scratch());
    if !have(&cap) || !have(&img) {
        return;
    }
    let map = mmap(&img);
    let image = Image::open(&map[..]).unwrap();
    let cases = load_capture(&cap);
    assert_eq!(cases.len(), 21_990);
    verify(
        &cases,
        &image,
        &format!("{}/profile/0100DFA027504000", scratch()),
        145,
    );
}

/// c1_guard.py / cb_extent.py over the whole Grizzy library, against the tables of an independent full Mac build of the game (iOSBuilder worktree out-shaders)
#[test]
fn tables_identical_to_the_python_tables_of_a_full_mac_build() {
    let img = format!("{}/runs/full-gil-j6/b-b24b25240cad.img", scratch());
    let want = format!(
        "{}/out-shaders/0100DFA027504000/profile/0100DFA027504000",
        ibuilder_testkit::iosbuilder_worktree()
    );
    if !have(&img) || !have(&want) {
        return;
    }
    let map = mmap(&img);
    let image = Image::open(&map[..]).unwrap();
    let keys = tables::c1_guard(&image);
    let c1 = std::fs::read_to_string(format!("{want}/c1_full.tsv")).unwrap();
    let want_keys: Vec<&str> = c1.lines().skip(1).collect();
    assert_eq!(
        keys.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        want_keys,
        "c1_full.tsv"
    );
    let rows = tables::cb_extent(&image);
    let cb = std::fs::read_to_string(format!("{want}/cb_extent.tsv")).unwrap();
    let want_rows: Vec<&str> = cb.lines().skip(2).collect();
    assert_eq!(
        rows.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        want_rows,
        "cb_extent.tsv"
    );
}

/// the image writer reproduces shader_lib.write byte for byte (the sample library the Python wrote)
#[test]
fn image_writer_reproduces_the_pythons_image() {
    let img = format!("{}/runs/s1800-cap/b-b24b25240cad.img", scratch());
    if !have(&img) {
        return;
    }
    let map = mmap(&img);
    let image = Image::open(&map[..]).unwrap();
    let files: Vec<(String, Vec<u8>)> = image
        .files
        .iter()
        .map(|f| (f.name.clone(), image.bytes(f).to_vec()))
        .collect();
    let tmp = std::env::temp_dir().join(format!("ibuilder-shader-test-{}.img", std::process::id()));
    ibuilder_shader::romfs::write_image_file(files, &tmp).unwrap();
    let a = std::fs::read(&tmp).unwrap();
    let _ = std::fs::remove_file(&tmp);
    assert_eq!(a.len(), map.len());
    assert!(
        a == map[..],
        "the Rust image differs from the Python's at byte {:?}",
        ibuilder_testkit::first_diff(&a, &map[..])
    );
}
