//! Development probe: Capstone equivalence between the C library built here (capstone-sys / vendored) and the Python
//! `capstone` the oracle used. `uniq <nso> <out.list>` lists the first address of each distinct text word;
//! `dump <list> <out.tsv>` decodes them (same format as crates/ibuilder-rewrite/tools/cs_dump.py).
use ibuilder_a64::cs::*;
use std::io::Write;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a[1].as_str() {
        "uniq" => {
            let m = ibuilder_nso::read(std::path::Path::new(&a[2])).unwrap();
            let te = m.text_end() as usize;
            let mut seen = std::collections::HashMap::<u32, u32>::new();
            for o in (0..te).step_by(4) {
                let w = u32::from_le_bytes(m.image[o..o + 4].try_into().unwrap());
                seen.entry(w).or_insert(o as u32);
            }
            let mut v: Vec<(u32, u32)> = seen.into_iter().map(|(w, o)| (o, w)).collect();
            v.sort();
            let mut f = std::io::BufWriter::new(std::fs::File::create(&a[3]).unwrap());
            for (o, w) in &v {
                f.write_all(&o.to_le_bytes()).unwrap();
                f.write_all(&w.to_le_bytes()).unwrap();
            }
            eprintln!("{} unique words of {}", v.len(), te / 4);
        }
        "fuzz" => {
            // fuzz <n> <seed> <nso> <out.list>: half uniform random words, half single/double bit flips of module words
            let n: usize = a[2].parse().unwrap();
            let mut x: u64 = a[3].parse::<u64>().unwrap() | 1;
            let mut rng = move || {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x
            };
            let m = ibuilder_nso::read(std::path::Path::new(&a[4])).unwrap();
            let te = m.text_end() as usize / 4;
            let mut f = std::io::BufWriter::new(std::fs::File::create(&a[5]).unwrap());
            for k in 0..n {
                let addr = ((rng() % (1 << 24)) as u32) * 4;
                let w = if k % 2 == 0 {
                    rng() as u32
                } else {
                    let o = (rng() as usize % te) * 4;
                    let mut w = u32::from_le_bytes(m.image[o..o + 4].try_into().unwrap());
                    w ^= 1 << (rng() % 32);
                    if rng() % 2 == 0 {
                        w ^= 1 << (rng() % 32);
                    }
                    w
                };
                f.write_all(&addr.to_le_bytes()).unwrap();
                f.write_all(&w.to_le_bytes()).unwrap();
            }
        }
        "check18" => check18(&a[2..]),
        "dump" => {
            let d = std::fs::read(&a[2]).unwrap();
            let mut dec = Decoder::new(true);
            let mut f = std::io::BufWriter::new(std::fs::File::create(&a[3]).unwrap());
            for c in d.as_chunks::<8>().0 {
                let o = u32::from_le_bytes(c[..4].try_into().unwrap());
                let w = u32::from_le_bytes(c[4..].try_into().unwrap());
                writeln!(f, "{}", dec.probe_line(w, o)).unwrap();
            }
        }
        _ => panic!("usage"),
    }
}

/// `check18 <list>...`: every word whose operand text names x18/w18 has a register field (bits 0, 5, 10, 16) equal to 18
#[allow(dead_code)]
pub fn check18(lists: &[String]) {
    let mut total = 0u64;
    let mut named = 0u64;
    for l in lists {
        let d = std::fs::read(l).unwrap();
        for c in d.as_chunks::<8>().0 {
            let o = u32::from_le_bytes(c[..4].try_into().unwrap());
            let w = u32::from_le_bytes(c[4..].try_into().unwrap());
            total += 1;
            if ibuilder_a64::cs::names_x18(w, o as i64) == Some(true) {
                named += 1;
                let f = |s: u32| (w >> s) & 31 == 18;
                assert!(
                    f(0) || f(5) || f(10) || f(16),
                    "word {w:#010x} names x18 but no register field is 18"
                );
            }
        }
    }
    println!("{named} of {total} words name x18/w18, every one has a register field 18");
}
