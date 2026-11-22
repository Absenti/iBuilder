//! `shader_tables <library image> <out dir> [--stamp STR]`: c1_guard.py and cb_extent.py over a finished shader library (the image dmsh_build / shader_lib.write makes).
use ibuilder_shader::{romfs::Image, tables};
use std::io::Write;
use std::time::Instant;

static LOG: std::sync::Mutex<Option<std::fs::File>> = std::sync::Mutex::new(None);
macro_rules! say { ($($a:tt)*) => {{ let s = format!($($a)*); eprintln!("{s}"); if let Some(f) = LOG.lock().unwrap().as_mut() { let _ = writeln!(f, "{s}"); } }}; }

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: shader_tables <library image> <out dir> [--stamp STR] [--threads N]");
        std::process::exit(2);
    }
    if let Some(p) = args
        .iter()
        .position(|a| a == "--log")
        .and_then(|i| args.get(i + 1))
    {
        *LOG.lock().unwrap() = Some(std::fs::File::create(p)?);
    }
    let (img_path, out_dir) = (&args[0], &args[1]);
    let stamp = args
        .iter()
        .position(|a| a == "--stamp")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "1970-01-01T00:00:00+00:00".into());
    if let Some(n) = args
        .iter()
        .position(|a| a == "--threads")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<usize>().ok())
    {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build_global()?;
    }
    let f = std::fs::File::open(img_path)?;
    // SAFETY: a read-only map of a finished library; nothing writes to it while the tables are made (the library crate itself is `forbid(unsafe_code)`)
    let map = unsafe { memmap2::Mmap::map(&f)? };
    let img = Image::open(&map[..]).map_err(|e| format!("{img_path}: {e}"))?;
    // the header of the tables: the library's chain version (the folder's name: b-<chain>) and its inputs stamp
    let chain = std::path::Path::new(img_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("b-unknown")
        .to_string();
    let inputs = img
        .find("inputs.tsv")
        .map(|r| {
            String::from_utf8_lossy(img.bytes(r))
                .lines()
                .find_map(|l| l.strip_prefix("inputs\t").map(|s| s.to_string()))
                .unwrap_or_else(|| "none".into())
        })
        .unwrap_or_else(|| "none".into());
    let head = format!("# chain {chain} inputs {inputs} {stamp}\n");
    if let Some(dst) = args
        .iter()
        .position(|a| a == "--repack")
        .and_then(|i| args.get(i + 1))
    {
        // shader_lib.write over the image's own files: the image writer's byte-for-byte check and its time
        let t = Instant::now();
        let files: Vec<(String, Vec<u8>)> = img
            .files
            .iter()
            .map(|f| (f.name.clone(), img.bytes(f).to_vec()))
            .collect();
        ibuilder_shader::romfs::write_image_file(files, std::path::Path::new(dst))?;
        say!(
            "repacked {} files into {dst} in {:.2} s",
            img.files.len(),
            t.elapsed().as_secs_f64()
        );
    }
    std::fs::create_dir_all(out_dir)?;
    let t0 = Instant::now();
    let keys = tables::c1_guard(&img);
    let t1 = t0.elapsed();
    let mut w = std::fs::File::create(format!("{out_dir}/c1_full.tsv"))?;
    w.write_all(head.as_bytes())?;
    for k in &keys {
        writeln!(w, "{k}")?;
    }
    let t2 = Instant::now();
    let rows = tables::cb_extent(&img);
    let t3 = t2.elapsed();
    let slots: usize = rows.iter().map(|r| r.matches(':').count()).sum();
    let mut w = std::fs::File::create(format!("{out_dir}/cb_extent.tsv"))?;
    w.write_all(head.as_bytes())?;
    w.write_all(b"# shader key, then reg:bytes for each uniform slot whose reads stay inside its first bytes, and s<reg> for each storage slot it never writes (cb_extent.py)\n")?;
    for r in &rows {
        writeln!(w, "{r}")?;
    }
    say!(
        "c1_guard: {} shaders not proven bounded, {:.2} s; cb_extent: {} slots (bounded uniform reads) in {} shaders, {:.2} s; files {}",
        keys.len(),
        t1.as_secs_f64(),
        slots,
        rows.len(),
        t3.as_secs_f64(),
        img.files.len()
    );
    Ok(())
}
