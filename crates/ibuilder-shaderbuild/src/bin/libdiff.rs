//! `libdiff <a.img> <b.img> [--ignore-first-line NAME]...`: compares two shader library images file by file (names, sizes, bytes). For the tables (`c1_full.tsv`, `cb_extent.tsv`) whose
//! first line carries a time stamp, `--ignore-first-line <name>` compares everything after it and the line up to the stamp. Exit 0 identical, 1 different.
use ibuilder_shader::romfs::Image;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: libdiff <a.img> <b.img> [--ignore-first-line NAME]...");
        std::process::exit(2);
    }
    let ign: Vec<&str> = args
        .windows(2)
        .filter(|w| w[0] == "--ignore-first-line")
        .map(|w| w[1].as_str())
        .collect();
    let map = |p: &str| {
        let f = std::fs::File::open(p).unwrap_or_else(|e| panic!("{p}: {e}"));
        // SAFETY: read-only map of a finished image that nothing writes meanwhile
        unsafe { memmap2::Mmap::map(&f) }.unwrap()
    };
    let (ma, mb) = (map(&args[0]), map(&args[1]));
    let (a, b) = (Image::open(&ma[..]).unwrap(), Image::open(&mb[..]).unwrap());
    let (mut same, mut diff, mut only_a, mut only_b, mut lenient) =
        (0usize, 0usize, 0usize, 0usize, 0usize);
    let mut shown = 0;
    for fa in &a.files {
        match b.find(&fa.name) {
            None => {
                only_a += 1;
                if shown < 10 {
                    eprintln!("only in A: {}", fa.name);
                    shown += 1;
                }
            }
            Some(fb) => {
                let (x, y) = (a.bytes(fa), b.bytes(fb));
                if x == y {
                    same += 1;
                } else if ign.contains(&fa.name.as_str()) {
                    // equal after the first line, and the first lines equal but for the time stamp
                    let (xl, yl) = (
                        x.iter().position(|c| *c == b'\n'),
                        y.iter().position(|c| *c == b'\n'),
                    );
                    match (xl, yl) {
                        (Some(i), Some(j))
                            if x[i..] == y[j..]
                                && x[..i].rsplitn(2, |c| *c == b' ').nth(1)
                                    == y[..j].rsplitn(2, |c| *c == b' ').nth(1) =>
                        {
                            lenient += 1
                        }
                        _ => {
                            diff += 1;
                            eprintln!("differs: {}", fa.name);
                        }
                    }
                } else {
                    diff += 1;
                    if shown < 10 {
                        eprintln!("differs: {} ({} vs {} bytes)", fa.name, x.len(), y.len());
                        shown += 1;
                    }
                }
            }
        }
    }
    for fb in &b.files {
        if a.find(&fb.name).is_none() {
            only_b += 1;
            if shown < 10 {
                eprintln!("only in B: {}", fb.name);
                shown += 1;
            }
        }
    }
    println!(
        "files A {} B {}: identical {same}, identical but for the time stamp line {lenient}, different {diff}, only in A {only_a}, only in B {only_b}",
        a.files.len(),
        b.files.len()
    );
    std::process::exit(if diff + only_a + only_b == 0 { 0 } else { 1 });
}
