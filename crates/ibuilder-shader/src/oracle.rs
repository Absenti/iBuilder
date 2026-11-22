//! Loading the Python's inputs (a capture of the instrumented dmsh_build.py, or the manifest `tools/shader_post.py` reads) and running the Rust chain over them; used by the
//! `shader_post` binary and the oracle tests.
use crate::PostError;
use crate::post::{Job, Stage, post_rewrites};
use crate::vs::user_locations;
use rayon::prelude::*;
use std::collections::{BTreeSet, HashMap};

pub struct Case {
    pub key: String,
    pub raw: String,
    pub stage: Stage,
    pub local: Option<Vec<i64>>,
    pub has_tf: bool,
    pub tf: Option<serde_json::Value>,
    pub res: serde_json::Value,
}

/// `<dir>/<key>/{raw.metal, job.json, res.json}`
pub fn load_capture(dir: &str) -> Vec<Case> {
    let mut keys: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().join("raw.metal").is_file())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    keys.sort();
    keys.iter()
        .filter_map(|k| {
            let d = format!("{dir}/{k}");
            let job: serde_json::Value =
                serde_json::from_slice(&std::fs::read(format!("{d}/job.json")).ok()?).ok()?;
            Some(case(
                k,
                std::fs::read_to_string(format!("{d}/raw.metal")).ok()?,
                &job,
                serde_json::from_slice(&std::fs::read(format!("{d}/res.json")).ok()?).ok()?,
            ))
        })
        .collect()
}

/// the manifest of `tools/shader_post.py`: `jobs.json`, `keys.txt`, `<key>.metal`, `<key>.res.json`
pub fn load_manifest(dir: &str) -> Vec<Case> {
    let Ok(jobs) = std::fs::read(format!("{dir}/jobs.json")) else {
        return vec![];
    };
    let jobs: Vec<serde_json::Value> = serde_json::from_slice(&jobs).unwrap_or_default();
    jobs.iter()
        .filter_map(|j| {
            let k = j["key"].as_str()?.to_string();
            let raw = std::fs::read_to_string(format!("{dir}/{k}.metal")).ok()?;
            let res: serde_json::Value =
                serde_json::from_slice(&std::fs::read(format!("{dir}/{k}.res.json")).ok()?).ok()?;
            Some(case(&k, raw, j, res))
        })
        .collect()
}

fn case(k: &str, raw: String, job: &serde_json::Value, res: serde_json::Value) -> Case {
    let local = job
        .get("local")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_i64()).collect::<Vec<_>>());
    let has_tf = job
        .get("tf")
        .map(|v| match v {
            serde_json::Value::Null => false,
            serde_json::Value::Array(a) => !a.is_empty(),
            _ => true,
        })
        .unwrap_or(false);
    let tf = if has_tf { job.get("tf").cloned() } else { None };
    Case {
        key: k.to_string(),
        raw,
        stage: Stage::parse(job["stage"].as_str().unwrap_or("?")),
        local,
        has_tf,
        tf,
        res,
    }
}

pub fn load_pairs(profile: &str) -> HashMap<String, BTreeSet<String>> {
    let mut m: HashMap<String, BTreeSet<String>> = HashMap::new();
    for table in ["shader_pairs.tsv", "pso_pairs.tsv"] {
        let Ok(t) = std::fs::read_to_string(format!("{profile}/{table}")) else {
            continue;
        };
        for l in t.lines() {
            if l.trim().is_empty() || l.starts_with('#') {
                continue;
            }
            let mut it = l.split_whitespace();
            if let (Some(v), Some(p)) = (it.next(), it.next()) {
                m.entry(v.to_string()).or_default().insert(p.to_string());
            }
        }
    }
    m
}

pub type Outputs = HashMap<String, Result<String, PostError>>;

/// One pass of the chain over `cases`: phase 1 everything but vertex shaders, phase 2 the vertex shaders, which read their paired pixel shaders' finished MSL (`partner_msl(ps key)`).
/// -> (outputs, seconds of phase 1, seconds of phase 2)
pub fn run_cases(
    cases: &[Case],
    pairs: &HashMap<String, BTreeSet<String>>,
    partner_msl: &(dyn Fn(&str, &Outputs) -> Option<String> + Sync),
) -> (Outputs, f64, f64) {
    let run = |c: &Case, partner: Option<&BTreeSet<u32>>| -> Result<String, PostError> {
        let job = Job {
            stage: c.stage.clone(),
            local: c.local.clone(),
            has_tf: c.has_tf,
            tf: c.tf.as_ref(),
            dual_want: false,
            partner_locs: partner,
            res: Some(&c.res),
            mesh: false,
        };
        post_rewrites(&c.raw, &job).map(|o| o.out)
    };
    let t0 = std::time::Instant::now();
    let first: Vec<(String, Result<String, PostError>)> = cases
        .par_iter()
        .filter(|c| c.stage != Stage::Vs)
        .map(|c| (c.key.clone(), run(c, None)))
        .collect();
    let t1 = t0.elapsed().as_secs_f64();
    let mut out: Outputs = first.into_iter().collect();
    let t2 = std::time::Instant::now();
    let second: Vec<(String, Result<String, PostError>)> = cases
        .par_iter()
        .filter(|c| c.stage == Stage::Vs)
        .map(|c| {
            let partner = pairs.get(&c.key).filter(|p| !p.is_empty()).map(|ps| {
                let mut want = BTreeSet::new();
                for p in ps {
                    if let Some(d) = partner_msl(p, &out) {
                        want.extend(user_locations(&d));
                    }
                }
                want
            });
            (c.key.clone(), run(c, partner.as_ref()))
        })
        .collect();
    for (k, v) in second {
        out.insert(k, v);
    }
    (out, t1, t2.elapsed().as_secs_f64())
}
