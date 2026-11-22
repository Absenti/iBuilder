//! `step_diff --cases cases.tsv --out out.tsv [--threads N] [--only STEP] [--dump DIR ID...]`
//!
//! The Rust half of the per-step differential test: every line of `cases.tsv` is `id<TAB>step<TAB>path of the input MSL<TAB>args json`; the result file has one line per case,
//! `id status same len crc32 meta msg`, in the format `tools/difftest/stepdiff.py` writes for the Python function of the same name. `cmp`/`compare.py` the two files.
use ibuilder_shader::difftest::run;
use rayon::prelude::*;
use std::collections::HashMap;
use std::io::Write;

fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for i in 0..256u32 {
            let mut c = i;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            t[i as usize] = c;
        }
        t
    });
    let mut c = !0u32;
    for &b in data {
        c = t[((c ^ b as u32) & 0xff) as usize] ^ (c >> 8);
    }
    !c
}

/// the message as one line (Python: `json.dumps(msg, ensure_ascii=False)`)
fn one_line(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cases = arg(&args, "--cases").ok_or("--cases")?;
    let out = arg(&args, "--out").ok_or("--out")?;
    let only = arg(&args, "--only");
    if let Some(n) = arg(&args, "--threads").and_then(|s| s.parse::<usize>().ok()) {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build_global()?;
    }
    let text = std::fs::read_to_string(&cases)?;
    let mut rows: Vec<(String, String, String, serde_json::Value)> = Vec::new();
    for l in text.lines() {
        let mut it = l.splitn(4, '\t');
        let (Some(id), Some(step), Some(path)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        if let Some(o) = &only
            && o != step
        {
            continue;
        }
        let a: serde_json::Value = it
            .next()
            .and_then(|x| serde_json::from_str(x).ok())
            .unwrap_or(serde_json::Value::Null);
        rows.push((id.to_string(), step.to_string(), path.to_string(), a));
    }
    let mut files: HashMap<String, String> = HashMap::new();
    for (_, _, p, _) in &rows {
        if !files.contains_key(p) {
            files.insert(p.clone(), std::fs::read_to_string(p)?);
        }
    }
    // `--dump DIR ID...`: a `--dump` with no DIR after it is a usage error (slicing past the end panicked)
    let dump: Vec<String> = match args.iter().position(|a| a == "--dump") {
        Some(i) => args.get(i + 2..).ok_or("--dump DIR [ID...]")?.to_vec(),
        None => Vec::new(),
    };
    let dump_dir = arg(&args, "--dump");
    let t0 = std::time::Instant::now();
    let lines: Vec<String> = rows
        .par_iter()
        .map(|(id, step, path, a)| {
            let msl = &files[path];
            match run(step, msl, a) {
                None => format!("{id}\tUNKNOWN\t0\t0\t0\t{{}}\t\"\""),
                Some(o) if o.ok => {
                    if let Some(d) = &dump_dir
                        && dump.contains(id)
                    {
                        let _ = std::fs::write(format!("{d}/{id}.{step}.out"), &o.text);
                    }
                    format!(
                        "{id}\tOK\t{}\t{}\t{}\t{}\t\"\"",
                        (o.text == *msl) as u8,
                        o.text.len(),
                        crc32(o.text.as_bytes()),
                        ibuilder_shader::difftest::sorted_json(o.meta.clone())
                    )
                }
                Some(o) => format!("{id}\tERR\t0\t0\t0\tnull\t{}", one_line(&o.msg)),
            }
        })
        .collect();
    let mut f = std::io::BufWriter::new(std::fs::File::create(&out)?);
    for l in &lines {
        writeln!(f, "{l}")?;
    }
    eprintln!(
        "{} cases in {:.2} s",
        lines.len(),
        t0.elapsed().as_secs_f64()
    );
    Ok(())
}
