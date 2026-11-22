//! `finish` for the round-3 shader kinds with stand-in tools (no translator, no spirv-cross): the plumbing the differential corpora do not reach, i.e. what `finish` does with a task / mesh
//! job (`mesh_stage`), a hull / domain job and their pairs, and which status it returns where the Python raises. The text rewrites themselves are in `tessmesh_cases.rs` and the end-to-end
//! comparison with the Python build is `crates/ibuilder-shaderbuild/tests/tessmesh_e2e.rs`.
use ibuilder_shader::finish::{
    Ctx, Options, Partners, SphReads, finish_partners, finish_with_locs,
};
use ibuilder_shader::tools::{ShaderTools, ToolRun};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// SPIRV-Cross that writes a fixed MSL to its `--output` file
struct Cross(String);
impl ShaderTools for Cross {
    fn xlate(&self, _: &[String]) -> ToolRun {
        ToolRun::fail(1, "no translator here")
    }
    fn spirv_cross(&self, argv: &[String]) -> ToolRun {
        let out = argv
            .iter()
            .position(|a| a == "--output")
            .and_then(|i| argv.get(i + 1))
            .expect("--output");
        std::fs::write(out, &self.0).unwrap();
        ToolRun::ok()
    }
}

const KERNEL: &str = "#include <metal_stdlib>\nusing namespace metal;\ntemplate<typename T, size_t Num>\nstruct spvUnsafeArray\n{\n    T elements[Num ? Num : 1];\n    thread T& operator [] (size_t pos) thread { return elements[pos]; }\n    threadgroup T& operator [] (size_t pos) threadgroup { return elements[pos]; }\n    device T& operator [] (size_t pos) device { return elements[pos]; }\n};\nstruct B { uint _m0[4]; };\nstruct spvDescriptorSetBuffer0 { constant B* cp_c3 [[id(0)]]; };\n\nkernel void main0(constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]], uint3 gl_LocalInvocationID [[thread_position_in_threadgroup]])\n{\n    threadgroup spvUnsafeArray<uint, 96> _23;\n    if (gl_LocalInvocationID.x != 0u) return;\n    _23[3] = (*spvDescriptorSet0.cp_c3)._m0[0];\n}\n";

const MS: &str = "00000000000000a1";
const PS: &str = "00000000000000b2";

fn ctx(pairs: bool) -> Ctx {
    let map = format!(
        "# test\nms_main.bin\t0\tms\t{MS}\tsynthetic#ms\nps_main.bin\t0\tps\t{PS}\tsynthetic#ps\n"
    );
    let p = format!("{MS}\t{PS}\n");
    Ctx::from_tables(
        "b24b25240cad",
        Options::ios_source_mode(false),
        &map,
        "",
        &if pairs { vec![p.as_str()] } else { vec![] },
        "",
        "",
    )
}

fn work(key: &str, spv: &[u8]) -> std::path::PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!(
        "tessmesh_finish_{}_{}_{key}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join(format!("{key}.spv")), spv).unwrap();
    std::fs::write(
        d.join(format!("{key}.res.json")),
        r#"{"cbufs": [], "sbufs": [], "textures": [], "images": [], "notes": []}"#,
    )
    .unwrap();
    d
}

fn header() -> Vec<u8> {
    let mut v = vec![0x03, 0x02, 0x23, 0x07];
    v.extend_from_slice(&[0u8; 16]);
    v
}

fn mesh_job() -> Value {
    json!({"key": MS, "stage": "cs", "mesh_stage": {"kind": "ms", "threads": 32, "payload": 128, "task": true, "verts": 3, "max_v": 32, "max_p": 32,
        "vertex": [[null, 0], [null, 1], [null, 2], [null, 3], [0, 0], [0, 1]], "primitive": [], "primitive_system": 0, "primitive_builtins": []}, "local": [32, 1, 1], "shared": 1024})
}

struct Pairs {
    reads: BTreeMap<i64, i64>,
    locs: BTreeSet<u32>,
}
impl Partners for Pairs {
    fn locs(&self, _: &str) -> Option<BTreeSet<u32>> {
        Some(self.locs.clone())
    }
    fn sph_reads(&self, _: &str) -> SphReads {
        SphReads::Reads(self.reads.clone())
    }
}

#[test]
fn a_mesh_shader_with_pairs_is_unported_through_the_locations_only_entry_point() {
    let w = work(MS, &header());
    let mut job = mesh_job();
    let f = finish_with_locs(
        &ctx(true),
        MS,
        &w,
        &mut job,
        &[],
        &Cross(KERNEL.into()),
        &|_| Some(BTreeSet::new()),
    );
    assert!(f.is_unported(), "{} / {}", f.status, f.detail);
    assert!(f.outputs.is_empty());
    // without pairs the same call finishes it
    let mut job = mesh_job();
    let f = finish_with_locs(
        &ctx(false),
        MS,
        &w,
        &mut job,
        &[],
        &Cross(KERNEL.into()),
        &|_| None,
    );
    assert!(f.is_ok(), "{} / {}", f.status, f.detail);
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn a_mesh_shader_pads_what_its_pixel_shaders_read() {
    let w = work(MS, &header());
    let mut job = mesh_job();
    // the pixel shader's SPH names location 0 (all components); its finished MSL also declares location 3, which no input map names: that counts as all four
    let p = Pairs {
        reads: BTreeMap::from([(0, 0xf)]),
        locs: BTreeSet::from([0, 3]),
    };
    let f = finish_partners(&ctx(true), MS, &w, &mut job, &[], &Cross(KERNEL.into()), &p);
    assert!(f.is_ok(), "{} / {}", f.status, f.detail);
    let msl = String::from_utf8(
        f.outputs
            .iter()
            .find(|o| o.0 == format!("{MS}_b.metal"))
            .unwrap()
            .1
            .clone(),
    )
    .unwrap();
    assert!(
        msl.contains("float4 dm_pad3 [[user(locn3)]];"),
        "location 3 is padded"
    );
    assert!(
        !msl.contains("dm_pad0"),
        "location 0 is written by the shader"
    );
    assert!(msl.contains("[[mesh]] void main0("), "the mesh function");
    assert!(
        !msl.contains("main0_o("),
        "a mesh shader with a task has no object function of its own"
    );
    let json: Value = serde_json::from_slice(
        &f.outputs
            .iter()
            .find(|o| o.0 == format!("{MS}_b.json"))
            .unwrap()
            .1,
    )
    .unwrap();
    assert_eq!(json["stage"], "ms");
    assert_eq!(
        json["mesh_stage"],
        json!({"kind": "ms", "threads": 32, "payload": 128, "task": true})
    );
    assert_eq!(json["entry_z"], "main0_z");
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn a_refused_mesh_job_and_a_mesh_stage_error_are_failures_not_outputs() {
    let w = work(MS, &header());
    let mut job = mesh_job();
    job["mesh_stage"]["refused"] =
        json!("shared memory of its own, or stage memory past Metal's limit");
    let f = finish_with_locs(
        &ctx(false),
        MS,
        &w,
        &mut job,
        &[],
        &Cross(KERNEL.into()),
        &|_| None,
    );
    assert_eq!(
        (f.status.as_str(), f.detail.as_str()),
        (
            "mesh stage failed",
            "shared memory of its own, or stage memory past Metal's limit"
        )
    );
    let mut job = mesh_job();
    job["mesh_stage"]["verts"] = json!(4);
    let f = finish_with_locs(
        &ctx(false),
        MS,
        &w,
        &mut job,
        &[],
        &Cross(KERNEL.into()),
        &|_| None,
    );
    assert_eq!(
        (f.status.as_str(), f.detail.as_str()),
        (
            "mesh stage failed",
            "ms: primitive vertex count 4 is not 1, 2 or 3"
        )
    );
    let mut job = mesh_job();
    let f = finish_with_locs(
        &ctx(false),
        MS,
        &w,
        &mut job,
        &[],
        &Cross("kernel void other() {}\n".into()),
        &|_| None,
    );
    assert_eq!(
        (f.status.as_str(), f.detail.as_str()),
        (
            "mesh stage failed",
            "ms: no kernel main0 starting with its stage memory"
        )
    );
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn an_unsupported_domain_keeps_its_source_and_says_so_a_broken_spirv_stops_the_shader() {
    let key = "00000000000000c3";
    let map = format!("ds_main.bin\t0\tds\t{key}\tsynthetic#ds\n");
    let c = Ctx::from_tables(
        "b24b25240cad",
        Options::ios_source_mode(false),
        &map,
        "",
        &[],
        "",
        "",
    );
    // isolines: the Python raises ValueError inside dmsh_tess.domain, `finish` keeps the source and records the message
    let w = work(key, &header());
    let mut job = json!({"key": key, "stage": "ds", "tess_mode": {"mode": "isolines", "spacing": "equal", "winding": "cw"}});
    let msl = "#include <metal_stdlib>\nusing namespace metal;\nstruct main0_out { float4 gl_Position [[position]]; };\nvertex main0_out main0() { main0_out out = {}; return out; }\n";
    let f = finish_with_locs(&c, key, &w, &mut job, &[], &Cross(msl.into()), &|_| None);
    assert!(f.is_ok(), "{} / {}", f.status, f.detail);
    let refl = f.refl.unwrap();
    assert_eq!(
        refl["tess"],
        json!({"unsupported": "tessellation: domain mode isolines is not implemented (quads and triangles)"})
    );
    assert!(
        refl.get("entry_geometry_capture").is_none(),
        "no depth-convention entry points for an unsupported domain"
    );
    // a SPIR-V file of a size that is not a multiple of four bytes: struct.error in the Python's spirv_io (hull) or spv_single_lane (domain), which `finish` does not catch
    let hs = "00000000000000d4";
    let c2 = Ctx::from_tables(
        "b24b25240cad",
        Options::ios_source_mode(false),
        &format!("hs_main.bin\t0\ths\t{hs}\tsynthetic#hs\n"),
        "",
        &[],
        "",
        "",
    );
    let mut bad = header();
    bad.extend_from_slice(&[1, 2, 3]);
    let w2 = work(hs, &bad);
    let mut job = json!({"key": hs, "stage": "hs"});
    let f = finish_with_locs(&c2, hs, &w2, &mut job, &[], &Cross(msl.into()), &|_| None);
    assert_eq!(f.status, "fatal", "{}", f.detail);
    assert!(f.detail.contains("struct.error"), "{}", f.detail);
    let w3 = work(key, &bad);
    let mut job = json!({"key": key, "stage": "ds", "tess_mode": {"mode": "triangles", "spacing": "equal", "winding": "cw"}});
    let f = finish_with_locs(&c, key, &w3, &mut job, &[], &Cross(msl.into()), &|_| None);
    assert_eq!(f.status, "fatal", "{}", f.detail);
    let _ = std::fs::remove_dir_all(&w3);
    let _ = std::fs::remove_dir_all(&w);
    let _ = std::fs::remove_dir_all(&w2);
}
