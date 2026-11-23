//! The reflection over a real library, against the Python's answers on the same inputs: `$REFLECT_CAPTURE` names a json-lines file of the engine's `reflect`, `raster_facts`,
//! `drv_reads`, `unfillable` and `attributes` answers (one line per shader: `key`, `stage`, `rstage`, the answers as `json.dumps` text, `err` the reflection's `ValueError`),
//! `$REFLECT_CAP` the capture folder with each shader's `<key>/<key>.res.json`, `$REFLECT_IMAGE` the library image whose `<key>_b.metal` the answers were made from.
//! Game data stays local: without the three variables the test says so and passes.
use ibuilder_shader::finish::{pyjson::J, reflect};
use ibuilder_shader::romfs::Image;
use serde_json::Value;

#[test]
fn reflection_over_a_captured_library() {
    let (Ok(lines), Ok(cap), Ok(img)) = (
        std::env::var("REFLECT_CAPTURE"),
        std::env::var("REFLECT_CAP"),
        std::env::var("REFLECT_IMAGE"),
    ) else {
        ibuilder_testkit::skip("REFLECT_CAPTURE / REFLECT_CAP / REFLECT_IMAGE not set");
        return;
    };
    let file = std::fs::File::open(&img).unwrap();
    // SAFETY: the image is a finished build output nothing writes while the test reads it
    let map = unsafe { memmap2::Mmap::map(&file) }.unwrap();
    let image = Image::open(&map).unwrap();
    let text = std::fs::read_to_string(&lines).unwrap();
    let names = [
        "bindings",
        "raster",
        "drv_reads",
        "unfillable",
        "attributes",
    ];
    let mut bad = [0usize; 5];
    let mut n = 0;
    let mut shown = 0;
    for line in text.lines() {
        let x: Value = serde_json::from_str(line).unwrap();
        let key = x["key"].as_str().unwrap();
        let (stage, rstage) = (x["stage"].as_str().unwrap(), x["rstage"].as_str().unwrap());
        let msl = std::str::from_utf8(image.bytes(image.find(&format!("{key}_b.metal")).unwrap()))
            .unwrap()
            .to_string();
        let res: Value =
            serde_json::from_slice(&std::fs::read(format!("{cap}/{key}/{key}.res.json")).unwrap())
                .unwrap();
        n += 1;
        let binds = match reflect::reflect(key, rstage, &msl, &res) {
            Ok(b) => J::Arr(b).dumps_compact(),
            Err(e) => format!("error {e}"),
        };
        let expect_binds = match x["bindings"].as_str() {
            Some(b) => b.to_string(),
            None => format!("error {}", x["err"].as_str().unwrap_or("")),
        };
        let dr = reflect::drv_reads(&msl);
        let got = [
            binds,
            reflect::raster_facts(stage, &msl).dumps_compact(),
            dr.dumps_compact(),
            J::Arr(
                reflect::unfillable(&dr, rstage)
                    .into_iter()
                    .map(J::Str)
                    .collect(),
            )
            .dumps_compact(),
            J::Arr(
                reflect::attributes(&msl)
                    .into_iter()
                    .map(|(t, m, l)| J::Arr(vec![J::Str(t), J::Str(m), J::Int(l)]))
                    .collect(),
            )
            .dumps_compact(),
        ];
        let expect = [
            expect_binds,
            x["raster"].as_str().unwrap().to_string(),
            x["drv"].as_str().unwrap().to_string(),
            x["unfillable"].as_str().unwrap().to_string(),
            x["attributes"].as_str().unwrap().to_string(),
        ];
        for i in 0..5 {
            if got[i] != expect[i] {
                bad[i] += 1;
                if shown < 6 {
                    shown += 1;
                    eprintln!(
                        "{key} {}: python {} / rust {}",
                        names[i],
                        expect[i].chars().take(300).collect::<String>(),
                        got[i].chars().take(300).collect::<String>()
                    );
                }
            }
        }
    }
    for i in 0..5 {
        eprintln!("{}: {n} shaders, {} differ", names[i], bad[i]);
    }
    assert_eq!(bad, [0; 5]);
}
