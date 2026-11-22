fn main() {
    let a: Vec<String> = std::env::args().collect();
    let f = std::fs::read(&a[1]).unwrap();
    let raw = std::fs::read(&a[2]).unwrap();
    let mut out = vec![0u8; raw.len()];
    match ibuilder_nso::zbic::zstd_decompress_into(&f, &mut out) {
        Ok(n) => println!("ok {n} equal {}", out == raw),
        Err(e) => {
            let d = out.iter().zip(&raw).position(|(a, b)| a != b);
            println!("ERR {e}; first differing byte {:?}", d);
        }
    }
}
