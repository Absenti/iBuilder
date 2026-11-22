//! `shader_post (--cap <capture dir> | --manifest <shader_post manifest dir>) --expect <library image> [--profile <dir with shader_pairs.tsv>] [--threads N] [--repeat K]
//! [--partners expected|rust] [--dump DIR]`
//!
//! The Rust side of `tools/shader_post.py`: runs the post-rewrite chain over the Python's inputs, compares every result with the `<key>_b.metal` the Python build put in the library
//! image, and times the Rust chain.
use ibuilder_shader::{PostError, oracle::*, romfs::Image};
use std::collections::BTreeMap;
use std::time::Instant;

static LOG: std::sync::Mutex<Option<std::fs::File>> = std::sync::Mutex::new(None);
/// stdout + the `--log` file (`simctl spawn` does not forward a child's stdout)
macro_rules! say { ($($a:tt)*) => {{ let s = format!($($a)*); println!("{s}"); if let Some(f) = LOG.lock().unwrap().as_mut() { use std::io::Write; let _ = writeln!(f, "{s}"); } }}; }

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(p) = arg(&args, "--log") {
        *LOG.lock().unwrap() = Some(std::fs::File::create(p)?);
    }
    let expect = arg(&args, "--expect");
    let repeat: usize = arg(&args, "--repeat")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let partners = arg(&args, "--partners").unwrap_or_else(|| "expected".into());
    let dump = arg(&args, "--dump");
    if let Some(n) = arg(&args, "--threads").and_then(|s| s.parse::<usize>().ok()) {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build_global()?;
    }
    let t_load = Instant::now();
    let (cases, profile) = if let Some(c) = arg(&args, "--cap") {
        (
            load_capture(&c),
            arg(&args, "--profile").unwrap_or_default(),
        )
    } else if let Some(m) = arg(&args, "--manifest") {
        let p = arg(&args, "--profile").unwrap_or(format!("{m}/profile"));
        (load_manifest(&m), p)
    } else {
        return Err("--cap or --manifest".into());
    };
    let bytes: usize = cases.iter().map(|c| c.raw.len()).sum();
    say!(
        "{} shaders, {:.1} MB of raw MSL, loaded in {:.2} s",
        cases.len(),
        bytes as f64 / 1e6,
        t_load.elapsed().as_secs_f64()
    );
    let map = match &expect {
        Some(p) => {
            let f = std::fs::File::open(p)?; // SAFETY: a read-only map of a finished image; nothing writes to it meanwhile
            Some(unsafe { memmap2::Mmap::map(&f)? })
        }
        None => None,
    };
    let img = match &map {
        Some(m) => Some(Image::open(&m[..])?),
        None => None,
    };
    let expected = |key: &str| -> Option<&[u8]> {
        img.as_ref()
            .and_then(|i| i.find(&format!("{key}_b.metal")).map(|f| i.bytes(f)))
    };
    let pairs = load_pairs(&profile);
    let partner_msl = |ps: &str, out: &Outputs| -> Option<String> {
        if partners == "rust" {
            out.get(ps).and_then(|r| r.as_ref().ok().cloned())
        } else {
            expected(ps).map(|b| String::from_utf8_lossy(b).into_owned())
        }
    };
    let (out, t_a, t_b) = run_cases(&cases, &pairs, &partner_msl);
    say!(
        "one pass: ps/cs {:.3} s + vs {:.3} s = {:.3} s",
        t_a,
        t_b,
        t_a + t_b
    );

    let (mut same, mut differ, mut unported, mut failed) = (
        0,
        Vec::new(),
        BTreeMap::<&'static str, u32>::new(),
        Vec::new(),
    );
    let mut by_stage: BTreeMap<String, (u32, u32, u32)> = BTreeMap::new();
    for c in &cases {
        let e = by_stage.entry(format!("{:?}", c.stage)).or_default();
        match &out[&c.key] {
            Ok(s) => match expected(&c.key) {
                Some(x) if x == s.as_bytes() => {
                    same += 1;
                    e.0 += 1;
                }
                Some(_) => {
                    differ.push(c.key.clone());
                    e.1 += 1;
                }
                None => say!("{}: no expected output in the image", c.key),
            },
            Err(PostError::Unported(w)) => {
                *unported.entry(w).or_insert(0) += 1;
                e.2 += 1;
            }
            Err(PostError::Failed { step, msg }) => {
                failed.push(format!("{}: {step}: {msg}", c.key))
            }
        }
    }
    say!(
        "identical to the Python: {same} of {}; differ: {}; unported (gated): {:?}; failed: {}",
        cases.len(),
        differ.len(),
        unported,
        failed.len()
    );
    for (st, (a, b, c)) in &by_stage {
        say!("  {st}: identical {a}, differ {b}, unported {c}");
    }
    for k in differ.iter().take(10) {
        say!("DIFF {k}");
    }
    for f in failed.iter().take(10) {
        say!("FAILED {f}");
    }
    if let Some(d) = dump {
        std::fs::create_dir_all(&d)?;
        for k in &differ {
            if let Ok(s) = &out[k] {
                std::fs::write(format!("{d}/{k}_b.metal"), s)?;
            }
        }
    }
    if args.iter().any(|a| a == "--profile-steps") {
        // each step alone over the raw text of every ps / vs / cs (single thread): where the time of the chain goes, and how many raw texts the step changes
        use ibuilder_shader::{
            ampere, atomic, bit0, blend, border, cs, fsuint, guards, hmma, pervertex, pool, side,
            steps, utest, vs, wide, zsnap,
        };
        use std::borrow::Cow;
        let time = |name: &str, f: &dyn Fn(&str) -> bool| {
            let t = Instant::now();
            let n = cases.iter().filter(|c| f(&c.raw)).count();
            say!(
                "  {name:18} {:8.1} ms  {:6.1} us/shader   changes {n}",
                t.elapsed().as_secs_f64() * 1e3,
                t.elapsed().as_secs_f64() * 1e6 / cases.len() as f64
            );
        };
        let ch = |c: &Cow<str>| matches!(c, Cow::Owned(_));
        say!("per step over the {} raw inputs, one thread:", cases.len());
        time("check_msl", &|s| {
            steps::check_msl(s).map(|c| ch(&c)).unwrap_or(false)
        });
        time("wide_atomics", &|s| {
            atomic::wide_atomics(s).map(|c| ch(&c)).unwrap_or(false)
        });
        time("per_vertex", &|s| {
            pervertex::per_vertex(s).map(|c| ch(&c)).unwrap_or(false)
        });
        time("precise", &|s| ch(&steps::precise(s)));
        time("membar_fence", &|s| ch(&steps::membar_fence(s)));
        time("bit0_mask", &|s| ch(&bit0::bit0_mask(s)));
        time("select_guards", &|s| {
            guards::select_guards(s).map(|c| ch(&c)).unwrap_or(false)
        });
        time("mul_extended", &|s| ch(&ampere::mul_extended(s)));
        time("float_add_msl", &|s| ch(&ampere::float_add_msl(s)));
        time("gather_offsets", &|s| {
            ampere::gather_offsets(s).map(|c| ch(&c)).unwrap_or(false)
        });
        time("hmma_msl", &|s| {
            hmma::hmma_msl(s).map(|c| ch(&c)).unwrap_or(false)
        });
        time("shared_store_msl", &|s| {
            ampere::shared_store_msl(s).map(|c| ch(&c)).unwrap_or(false)
        });
        time("pool_arrays", &|s| {
            pool::pool_arrays(s).map(|c| ch(&c)).unwrap_or(false)
        });
        time("wide_loads", &|s| ch(&wide::wide_loads(s)));
        time("tg_fence", &|s| {
            steps::tg_fence(s).map(|c| ch(&c)).unwrap_or(false)
        });
        time("border_emu", &|s| {
            border::border_emu(s).map(|c| ch(&c)).unwrap_or(false)
        });
        time("frag_z_snap", &|s| zsnap::frag_z_snap(s).1 > 0);
        time("uniform_tests", &|s| {
            utest::uniform_tests(s).map(|c| ch(&c)).unwrap_or(false)
        });
        time("blend_zero", &|s| {
            blend::blend_zero(s).map(|(_, k)| k > 0).unwrap_or(false)
        });
        time("first_store (64x1x1)", &|s| {
            !cs::first_store(s, Some(&[64, 1, 1])).1.is_empty()
        });
        time("tg_spill", &|s| {
            cs::tg_spill(s).map(|(_, t)| t > 0).unwrap_or(false)
        });
        time("fs_uint", &|s| fsuint::fs_uint(s).is_some());
        time("shared_body", &|s| side::shared_body(s).is_some());
        time("vs_init", &|s| vs::vs_init(s) != s);
        time("vs_base_instance", &|s| vs::vs_base_instance(s) != s);
        time("vs_depth", &|s| vs::vs_depth(s).is_ok());
        time("capture", &|s| vs::capture(s).is_ok());
    }
    if repeat > 0 {
        let ok_n = out.values().filter(|v| v.is_ok()).count();
        let t = Instant::now();
        for _ in 0..repeat {
            let (o, _, _) = run_cases(&cases, &pairs, &partner_msl);
            std::hint::black_box(o);
        }
        let el = t.elapsed().as_secs_f64() / repeat as f64;
        say!(
            "bench: {repeat} passes, {:.3} s a pass, {} shaders produced a result per pass: {:.1} us per shader, {:.0} shaders/s",
            el,
            ok_n,
            el * 1e6 / ok_n as f64,
            ok_n as f64 / el
        );
    }
    Ok(())
}
