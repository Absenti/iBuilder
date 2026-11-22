//! `loadnso <module>...`: loads each NSO/NRO with the crate's loader and prints its segments, flags and the time (ZBIC decode included).
fn main() {
    for p in std::env::args().skip(1) {
        let t = std::time::Instant::now();
        match ibuilder_nso::read(std::path::Path::new(&p)) {
            Ok(m) => {
                let h = std::fs::read(&p).unwrap();
                let flags = u32::from_le_bytes(h[0xc..0x10].try_into().unwrap());
                println!(
                    "{p}: ok, flags {flags:#x}, image {:#x} bytes, segments {:x?}, bss {:#x}, {:.3} s",
                    m.image.len(),
                    m.segments,
                    m.bss,
                    t.elapsed().as_secs_f64()
                );
            }
            Err(e) => println!("{p}: ERROR {e}"),
        }
    }
}
