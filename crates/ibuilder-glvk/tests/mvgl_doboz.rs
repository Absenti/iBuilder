//! `mvgl.doboz` against the Python's answers on 400 synthetic streams (`tools/make_gl_fixtures.py doboz`): the decoded bytes, or the same `ValueError`, or an
//! `IndexError` (a failure that is not a `ValueError` here).
use ibuilder_glvk::mvgl::doboz;

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

#[test]
fn doboz_as_python() {
    let text = include_str!("fixtures/gl/doboz_vectors.txt");
    let (mut ok, mut errs) = (0, 0);
    for line in text.lines() {
        let (input, rest) = line.split_once(' ').unwrap();
        let got = doboz(&unhex(input));
        match rest.split_once(' ').unwrap_or((rest, "")) {
            ("OK", out) => {
                assert_eq!(got.as_deref(), Ok(unhex(out).as_slice()), "{input}");
                ok += 1;
            }
            ("VE", msg) => {
                let e = got.unwrap_err();
                assert!(e.value && e.msg == msg, "{input}: {e:?} vs {msg}");
                errs += 1;
            }
            ("IE", _) => {
                assert!(!got.unwrap_err().value, "{input}");
                errs += 1;
            }
            other => panic!("bad line {other:?}"),
        }
    }
    assert!(ok > 50 && errs > 50, "{ok} decoded, {errs} refused");
}
