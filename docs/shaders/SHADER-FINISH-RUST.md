# `ibuilder_shader::finish`: the per-shader `finish()` of `dmsh_build.py` in Rust (package F2)

Plan: `docs/shaders/SHADER-BACKEND-PLAN.md`. Context and measurements: `docs/shaders/SHADER-STAGE-ANALYSIS.md` (section 1.1), what the crate did before this round: `docs/shaders/SHADER-RUST.md`.
Code: `crates/ibuilder-shader/src/finish/`. Verification harness: `crates/ibuilder-shader/tools/finish/` (Python: instruments a COPY of the engine, generates differential cases) and
`src/bin/shader_finish.rs` (Rust, Mac only: drives the real `dmsh_xlate` / `spirv-cross` binaries). Tests: `tests/finish_corpus.rs`, `tests/finish_cases.rs` (+ unit tests in `finish/pyjson.rs`).

Python source version ported: the engine copy `$OLD/shader/engine/dm/gfx/tools/` (= `iOSBuilder/engine/dm/gfx/tools`, `diff -rq` empty, files dated 2026-11-16: `dmsh_build.py` 99,679 bytes / 1,157 lines,
`dmsh_reflect.py` 40,907, `dmsh_fetch.py` 22,629, `dmsh_side.py` 11,573, `dmsh_geometry.py` 16,714, `dmsh_pull.py` 13,443 + `dmsh_pull.metal` 7,065, `dmsh_entry.py` 20,386, `dmsh_alpha.py` 3,737,
`dmsh_logic.py` 5,591, `dmsh_vin.py` 3,927, `dmsh_atomic.py` 6,515, `dmsh_mtlcache.py` 10,654) and the phone's driver `gameapp/shader_device.py` of the iOSBuilder worktree `$IOSBUILDER_WORKTREE`
(37,785 bytes, mtime 2026-11-19 00:48, identical to the copy in `$OLD/shader/bin`; its `_source_mode` and `build` epilogue define the library names the Rust code returns).

## 0. Result in one paragraph

`finish()` for **ps, vs, cs and gs** is ported and **byte-identical to the Python on all 21,990 shaders of Grizzy** (10,541 ps, 10,537 vs, 767 cs, 145 gs): every one of the **56,882 shader files** of the Python's library
image (`<key>_b.json`, `<key>_b.metal`, `_bx`, `_bf`, `_bv`, `_bg0..4`, `_bgv`; with pulling on also `_bp`), names and bytes, reflection JSON spelling included. 0 differ, 0 unported, 0 failed.

**Round 3 (package SHADERS, `docs/shaders/SHADER-TESS-MESH-RUST.md`)** ported what this document listed as unported: tessellation (`hs` / `ds`: `dmsh_tess`), task and mesh shaders (`ts` / `ms`: `dmsh_meshstage`, with the pixel shaders' SPH
input maps of `partner_reads`), the passthrough geometry shaders' lean kernels (`_bl0/1/3`: `dmsh_geometry.lean`), geometry shaders with a transform feedback layout, and (in `ibuilder-shaderbuild`) the `job_for` mesh branch and the
pair-level `mesh_finish` (`<vs>_<gs>_m3`: `dmsh_mesh`). Grizzy has none of these kinds, so they are verified differentially against the engine's own Python on synthetic and tool-made input and end to end against the Python's
build of a synthetic game (`docs/shaders/SHADER-TESS-MESH-RUST.md` section 3), never on a real game's shader. Nothing is left `unported:` for these kinds except what section 5 names.

## 1. API for the build driver (package B codes against this)

```rust
use ibuilder_shader::finish::{Ctx, Options, Finished, finish, finish_with_locs};
use ibuilder_shader::tools::ShaderTools;

/// What `dmsh_build.py` reads from its environment. The Rust finisher reproduces what `shader_device.build` makes: **iOS only, source mode**
/// (`DMSH_TARGETS=ios`; `Cache.compile / compile_split` write the MSL text under the library names), so every returned file is a file of the final image.
pub struct Options {
    /// `DMSH_IOS_PULL`: also write the pulling libraries `<key>_bp.metal` (shader_device.build `pulling=`). The reflection's `pull` entry is written either way, as in the Python
    pub pulling: bool,
    /// `ios_metal.RECORD`: written into every reflection json as `chain_metal`
    pub chain_metal: String,
    /// `dmsh_mtlcache.Cache.ident['ios'].hex()` in source mode: written as `chain_compiler`
    pub chain_compiler: String,
}
impl Options { pub fn ios_source_mode(pulling: bool) -> Options }   // sha256(FORMAT \0 RECORD \0 "source-mode iphoneos"); equal to the strings in the library image (verified)

/// Loaded ONCE per build (immutable, `Sync`): the tables dmsh_build.py reads at module level.
pub struct Ctx { /* chain id, Options, shader_map rows (`loc`), `pc_of` (+ conflicts), PAIRS (shader_pairs + pso_pairs), BF_WANT, DUAL_WANT */ }
impl Ctx {
    /// `profile_dir`: the profile folder shader_device.build copies (a folder holding game.tsv, else its `<game_id>` sub-folder)
    pub fn load(profile_dir: &Path, game_id: &str, chain: &str, opts: Options) -> Result<Ctx, String>;
    pub fn load_with(profile_dir: &Path, game_id: &str, chain: &str, opts: Options, extra_dir: Option<&Path>) -> Result<Ctx, String>;   // extra_dir: pso_pairs.tsv fallback
    /// the same from tables the caller already holds (the split's output); `pairs`: the texts of shader_pairs.tsv and pso_pairs.tsv
    pub fn from_tables(chain: &str, opts: Options, shader_map: &str, conflicts: &str, pairs: &[&str], bf_want: &str, dual_want: &str) -> Ctx;
    /// the build's two phases (`dmsh_build.batches`): shaders without pairs first, then the vertex shaders that have pairs (vs_pad reads their pixel shaders' finished MSL)
    pub fn is_paired(&self, key: &str) -> bool;
    pub fn pairs_of(&self, key: &str) -> &BTreeSet<String>;
    pub fn has_row(&self, key: &str) -> bool;                 // key is in shader_map.tsv
    pub fn source_of(&self, key: &str) -> Option<String>;     // "<package>#<index>", the reflection's `source`
}

pub struct Finished {
    /// "ok"; the Python's failure kind verbatim ("MSL failed", "translation failed", "geometry failed", "reflection failed", "uniform bounds rewrite failed", ...);
    /// "unported: <why>"; or "fatal: <msg>" where the Python `sys.exit`s or raises out of `finish` (the Python build stops there)
    pub status: String,
    /// the Python's detail string (`stderr[:160]`, `str(e)[:160]`); for "unported" the reason
    pub detail: String,
    /// the `refl` dict `finish` returns (before `with_compiler`), for the summary (`drv_reads`, `drv_unfillable`, `bindings`, `notes`, `stage`); None unless "ok". (serde sorts keys: the written json is in `outputs`)
    pub refl: Option<serde_json::Value>,
    /// library file name -> bytes: exactly the shader's files of the final image: `<key>_b.json`, `<key>_b.metal`, and by stage `<key>_bx.metal`, `<key>_bf.metal` (ps), `<key>_bv.metal`,
    /// `<key>_bp.metal` (vs), `<key>_bg0..4.{json,metal}`, `<key>_bgv.*`, `<key>_b.{json,metal}` (gs; a layer-only gs, `gs_slice`, has `<key>_b.json` only)
    pub outputs: Vec<(String, Vec<u8>)>,
    /// the `user(locnN)` numbers of `<key>_b.metal` (ps only): what a paired vertex shader's `vs_pad` needs. Keep this per pixel shader instead of 20-45 KB of MSL
    pub ps_locs: BTreeSet<u32>,
    /// the `[RED] ...` lines the Python prints for what a shader lost (side/readable-tile/shadow/pull library not built)
    pub log: Vec<String>,
}

/// `finish(key, w, j, c1)`. `work` holds `<key>.spv`, `<key>.res.json`, `<key>.bin` as the translator and `job_for` left them; `job` is `job_for`'s dict (mutated like the Python does:
/// `attr` gains the integer inputs); `c1` its c1 bytes. `paired_ps(ps key)` = the finished `<ps>_b.metal` text of a pixel shader paired with this vertex shader, None when it is not
/// available (the Python skips such pairs). Writes into `work` only files whose names start with `<key>` (`<key>.metal`, `job1_<key>.json`, `<key>_geometry.json`, `<key>_g<n>.*`):
/// remove them afterwards as `fin` does.
pub fn finish(ctx: &Ctx, key: &str, work: &Path, job: &mut serde_json::Value, c1: &[u8],
              tools: &dyn ShaderTools, paired_ps: &dyn Fn(&str) -> Option<String>) -> Finished;
/// the same with the paired pixel shaders' `ps_locs` (`Finished::ps_locs`) instead of their MSL
pub fn finish_with_locs(ctx: &Ctx, key: &str, work: &Path, job: &mut serde_json::Value, c1: &[u8],
                        tools: &dyn ShaderTools, paired_locs: &dyn Fn(&str) -> Option<BTreeSet<u32>>) -> Finished;
/// round 3: the same with the paired shaders as `Partners`. A mesh shader's `partner_reads` needs, besides the pixel shaders' finished locations, their SPH input maps (`Partners::sph_reads`, read from
/// the pixel shader's record); `finish_with_locs` cannot give those, so a mesh shader WITH pairs is `unported` through it (without pairs it finishes)
pub trait Partners { fn locs(&self, ps: &str) -> Option<BTreeSet<u32>>; fn sph_reads(&self, ps: &str) -> SphReads { SphReads::Unsupported } }
pub enum SphReads { Reads(BTreeMap<i64, i64>), Failed(String), Unsupported }
pub fn finish_partners(ctx: &Ctx, key: &str, work: &Path, job: &mut serde_json::Value, c1: &[u8], tools: &dyn ShaderTools, partners: &dyn Partners) -> Finished;
```

Contract notes for B:

* **Thread safety.** `Ctx` is immutable; `finish` is re-entrant for different keys at once (every file it touches is key-prefixed). It calls `tools.xlate([jobs.json, work])` (the integer-input second pass; a geometry shader's five
  topology variants in one call) and `tools.spirv_cross(argv)` with the argv the Python passes, without the program name: `--msl --msl-version 20300 --msl-argument-buffers --msl-texture-buffer-native
  --msl-pad-fragment-output <key>.spv --output <key>.metal` (MSL 3.1 when the module has image atomics), `<g>.spv` with the MSL on **stdout** for the geometry variants, and `<key>.spv --version 450` (GLSL on stdout)
  for `gs_slice`. A `ShaderTools` that serves the phone must answer those three forms.
* **Order.** Everything without pairs first (all stages), then the paired vertex shaders (`Ctx::is_paired`); a paired vertex shader calls `paired_ps` for each of its pixel shaders (`Ctx::pairs_of`).
* **Files in `work`.** The Python's `finish` reads `j['code']` (= `<key>.bin`) for a pixel shader's `omap` and for geometry shaders; the Rust code reads `<work>/<key>.bin` first and `job["code"]` second.
* **Cache.** `finish` knows nothing of the cache. The Python caches `refl` minus `fetch/vin/pull/alpha/logic` and rebuilds the side libraries from the cached MSL because it caches *compiled* libraries by MSL hash; with the
  Rust source-mode output there is nothing to rebuild: B can cache `Finished.outputs` whole (all files of the shader) under its own key. (`finish::restore` was planned and dropped for that reason.)
* **Statuses.** `ok` is returned only when every step ran; anything the port does not handle is `unported: <why>` with **no outputs**: a mesh shader with pairs through the locations-only entry point (above), a stage
  other than ps / vs / cs / hs / ds / gs / a mesh job, and whatever the post-rewrite chain still returns `PostError::Unported(step)` for (`unported: chain step <step>`). `fatal` is a Python exception that is not caught in
  `finish` (round 3: a `struct.error` of `dmsh_tess.spirv_io` / `spv_single_lane` on a SPIR-V file whose size is not a multiple of four, an `IndexError` of `dmsh_geometry.capture` ...). Round 3 statuses of the Python's own:
  `mesh stage failed` (`mesh['refused']`, or a `ValueError` of `dmsh_meshstage.to_stage`: detail = the message[:160]); a hull / domain shape `dmsh_tess` refuses is NOT a failure: the shader is `ok`, keeps the translated source
  and its reflection says `"tess": {"unsupported": "<message>"}`.
* **What `finish` does not do** (B or later): `job_for`, `vertex_ab`, the cache, `commit`, `mesh_finish` (`<vs>_<gs>_m3`, a pair-level step after all shaders: `ibuilder-shaderbuild/src/mesh.rs` over `finish::mesh::compose`),
  `publish_library`, the profile tables (`shadertables`).
* **What `finish` needs from the chain** (`crate::post_rewrites`, F1): `PostOut.{out, analysis, layer_from, first_store, tg_spill, vs_pad, entry_discard, blend_zero, fs_uint, fs_dual}` and `Job.tf`. `analysis` is `src` of the Python at the
  point where it reflects (vertex shader: main0 only, after the GL depth rewrite; pixel shader with a shared body: main0 before `dm_body_main0`).

## 2. Python -> Rust

| Python (engine `dm/gfx/tools`) | Rust (`crates/ibuilder-shader/src/finish/`) | verified by |
|---|---|---|
| `dmsh_build.finish` l.737-875 (the flow, the reflection dict, `write_refl` / `atomic_json` / `MTL.with_compiler`) | `mod.rs` `finish_inner`, `finalize`, `pyjson.rs` (insertion-ordered dict, `json.dump(indent=1, ensure_ascii=False)` spelling) | the full library |
| module-level tables: `loc`, `pc_of` (+ `.conflicts.tsv`), `PAIRS` (two tables), `BF_WANT`, `DUAL_WANT`, `CHAIN`, `ios_metal.RECORD`, `Cache.ident` | `mod.rs` `Ctx`, `Options` | the full library (`chain_compiler` bytes) |
| `spv_single_lane`, `_ep_drop` (SPIR-V words) | `spv.rs` | 200 synthetic modules (148 rewrite); 0 of the 10,537 Grizzy vs is rewritten by it (checked with the Python function on the captured modules) |
| `dmsh_atomic.cross_args`, `uses_image_atomics`; the spirv-cross argv | `spv.rs`, `mod.rs` `run_cross` | corpus |
| second translation pass (`dmsh_entry.int_inputs`, `job1_<key>.json`, `XLATE`) | `entry.rs`, `mod.rs` | 155 Grizzy vs take it; 260 synthetic |
| the post-rewrite chain | `crate::post_rewrites` (F1) | corpus |
| `dmsh_reflect.reflect` + every `decode_*`, `c2_array`, `buffer_image_bias`, `buffer_bias_entries`, `has_word` | `reflect.rs` | corpus + 420 synthetic argument buffers (29 of the 32 binding kinds; the real library adds `samp_pool`; `samp_c2a` and `?` are never reached) |
| `drv_reads`, `index_range` (it parses with Python's `ast`: a small expression parser here), `unfillable` | `reflect.rs` | corpus + 200 synthetic (168 dynamic, 62 ranged) |
| `attributes`, `raster_facts`, `far_plane` | `reflect.rs` | corpus + 310 synthetic |
| `dmsh_fetch.screen_varyings`, `_entry_screen`, `_lin`, `fold`, `_proj`, `_frag_pos`, `_once` (the `(?(1)...)` conditional group and back-references by hand) | `fetch.rs` | corpus (74 `_bf`) + 260 synthetic (92 fold ok: 38 proj, 31 frag_pos, 36 varying) + the all-ps `bf_want` run below |
| `dmsh_side.entries / entry / body / shared_body / calling / unit`, `dmsh_alpha`, `dmsh_logic` (+ their fixed MSL helpers) | `unit.rs`, `texts.rs` (generated by `tools/finish/gen_texts.py`) | corpus (1,536 `_bx`) + 260 synthetic |
| `ps_libraries` in source mode (`compile_split` writes the one unit under `_b`, `_bx`, `_bf`; `build` swaps `_b` for the plain MSL) | `mod.rs` `ps_libraries` | corpus |
| `dmsh_vin.variant`, `vin_finish` | `vin.rs` | corpus (13) + 260 synthetic |
| `dmsh_pull.variant`, `pull_finish`, `dmsh_pull.metal`, wide-line entry points | `vin.rs`, `texts.rs` | `pulling=True` run (658 `_bp`) + 260 synthetic |
| `gs_slice`, `geometry_finish`, `dmsh_geometry.output_layout / reader / topology_pointer / capture_input / flatten_sets / passthrough` | `geometry.rs`, `spv.rs` | corpus (all 145 gs, none is a layer-only slice: `gs_slice` returns None for all) + 80-120 synthetic per helper (`gs_slice` on 120 synthetic GLSL texts, 11 slices) |
| `dmsh_abi.py` / `dmsh_abi.h` | `abi.rs` (generated by `tools/finish/gen_abi.py` through the engine's own reader) | |
| `partner_inputs` (`vs_pad`'s input) | `Finished::ps_locs`, `finish_with_locs` | corpus, both `expected` and `rust` partner modes |
| `dmsh_tess` (`spirv_io`, `mode_bits`, `control_mode`, `hull`, `domain`, `TESS_MSL`) and `finish` l.793-799 / 829-832 (hs / ds) | `tess.rs`, `tess_text.rs` (generated by `tools/tessmesh/gen_texts.py`), `mod.rs` | round 3: `docs/shaders/SHADER-TESS-MESH-RUST.md` |
| `dmsh_meshstage` (`info`, `memory`, `own_shared`, `pixel_reads`, `to_stage`) and `finish` l.800-804 (`partner_reads`) | `meshstage.rs`, `mod.rs` `partner_reads` / `Partners` | round 3 |
| `dmsh_mesh` (`compose`, `out_bytes`, `prims_for`) | `mesh.rs` (the pair-level `mesh_finish` is `ibuilder-shaderbuild/src/mesh.rs`) | round 3 |
| `dmsh_geometry.lean` and the lean / `vs_tf(geometry=True)` branches of `geometry_finish` | `lean.rs`, `geometry.rs` | round 3 |
| Python helpers whose text reaches a result (`str.splitlines`, `strip`, `repr`, `int()` with its `ValueError`) | `pycompat.rs` | round 3 |

## 3. Verification

Oracle = the Python's own build, run for real. `tools/finish/make_cap_engine.py` instruments a **copy** of the engine (`dmsh_build.finish` keeps its inputs, `<key>.spv`, `<key>.res.json`, `<key>.bin`, the job dict as it was before
`finish` mutates it, c1, and its own result) and `run_build.py` (`shader_device.build`, the phone's driver, GIL interpreter, real Mac `dmsh_xlate` / `spirv-cross`) builds the library once. `shader_finish` then runs the Rust
`finish` on every captured shader with the same two native binaries (a test-only `ProcessTools`) and compares **every file it returns** with the file of the same name in the Python's library image, and reports files the Python has that Rust
did not return (none) and files Rust returned that the Python has not (none). The instrumented full build is itself a faithful oracle: its image equals the earlier full Python image (`full-gil-j6`) file by file except the two profile tables
it does not regenerate (`$PY cmpimg2.py`: 56,885 files, differing: `c1_full.tsv`, `cb_extent.tsv`).

```
S2=$SCRATCH; F2=$S2/f2; OLD=$SCRATCH_OLD
SRC=$F2/engine-cap DST=$F2/engine-cap2 python -I tools/finish/make_cap_engine.py            # instrumented COPY (engine-cap = rsync of $OLD/shader/engine)
S=$F2/S DMSH_CAPF=$F2/cap-s1800 python -I $F2/bin/run_build.py --engine $F2/engine-cap2 --keys $OLD/shader/keys-s1800.txt --jobs 4 --out $F2/runs/s1800 --tables 0
cargo build --release -p ibuilder-shader --bin shader_finish
shader_finish --cap $F2/cap-s1800 --expect $F2/runs/s1800/b-b24b25240cad.img --profile $OLD/shader/profile --xlate <dmsh_xlate> --spirv-cross <spirv-cross> [--pulling] [--partners rust] [--mutate tf,discard] [--only gs]
cargo test --release -p ibuilder-shader --test finish_corpus --test finish_cases          # the same, as tests (skip with a message when a fixture is absent)
```

The chain (F1's `post_rewrites` and its steps) is the one in the tree at 2026-11-19 10:39-10:48 (`post.rs` 8,162 bytes, `pool.rs` 10,976, `vs.rs` 27,924, `cs.rs` 18,465, `tf.rs` 7,558); F1 was still working, so a later chain is re-checked by
re-running the tests below (`tests/steps.rs`, F1's, failed at that moment: not mine).

Results (Mac, shared, load average 20-500 while I measured):

| run | shaders | result |
|---|---|---|
| keys-s300 (+ pairs): 401 shaders (260 ps, 110 vs, 25 cs, 6 gs) | 401 | **identical 401**, differ 0, unported 0; 1,141 files |
| keys-s1800 (+ pairs): 2,459 shaders (1,539 ps, 700 vs, 200 cs, 20 gs) | 2,459 | **identical 2,459**; 6,781 files (every file of the image); same with `--partners rust` (vs_pad fed by the Rust pixel shaders) |
| s1800 built with `pulling=True` | 2,459 | **identical 2,459**; 7,439 files incl. 658 `_bp.metal` |
| s1800 with `dual_want.tsv` + `bf_want.tsv` listing every pixel shader (`profile-dual`) | 2,459 | **identical 2,459**; 8,241 files incl. 1,534 `_bf.metal` (the `om_only_ok` readable-tile path) and 1 `main0_d` entry (`fs_dual`) |
| s1800 built by the Python with `DMSH_MUTATE=tf,discard` (every vertex shader gets a transform feedback layout, `main0_tf`, and a storage write, `main0_discard`; 700 vs with both) | 2,459 | **identical 2,459**; 6,781 files |
| **full Grizzy: 21,990 shaders** (10,541 ps, 10,537 vs, 767 cs, 145 gs; 23,315 tool calls) | 21,990 | **identical 21,990**, differ 0, unported 0, failed 0; **56,882 files** = every shader file of the 56,885-file image (the other three are `inputs.tsv`, `c1_full.tsv`, `cb_extent.tsv`) |

Byte-identical files by kind (full Grizzy, the counts `shader_finish` prints): `<key>_b.json` 21,990, `<key>_b.metal` 21,990, `_bx.metal` 10,529, `_bf.metal` 478, `_bv.metal` 155, `_bg0..4.json` and `.metal` 145 each (10 kinds), `_bgv.json` / `.metal` 145 each.
(`_bp.metal` and `_bl*` are not in that library: pulling was off, Grizzy has no passthrough geometry; `_bp` is covered by the `pulling=True` sample.)

Differential cases (`tests/finish_cases.rs`, 6 tests, `tools/finish/gen_diff_cases.py`; the expectations are the engine's own functions run on hand-made synthetic MSL / SPIR-V, 3.4 MB in `tests/data/finish_cases.json`, long
expectations stored as `sha256:<digest>:<length>`): the parts the Grizzy library never reaches. Cases and what they exercise (generator's stderr): reflect 420 (217 ok, 203 raise; 29 binding kinds incl. `tex_ubo`, `samp_ubo`, `img_ubo`, `tbuf_ubo`, `tex_c2a`,
`tex_htab`, `sbuf_ubo`, `sbuf_push`, `ubo_ptr`, `gmem_log`, `sblen`), drv_reads + unfillable 200 (164 with a dynamic index, 70 ranged: the expression parser), vertex raster facts 250 (123 `pos_attr`, 61 `uv_from_attr`, 16 `uv_affine`, 35 `far`, 28 non-empty
`screen`), pixel raster facts 60, fragment units 260 (217 units; fold: 90 ok, 38 proj, 29 gl_FragCoord, 43 varying, 6 mixed-use slots), vertex libraries 260 (int_inputs 105, vin 151, pull 175 all with wide-line entry points), SPIR-V 200 (148 rewritten) +
60 `flatten_sets`, geometry helpers (`topology_pointer`, `capture_input`, `reader`, `output_layout`, `gs_slice`: 80 / 80 / 60 / 120 / 120). Result: every case equal. The tests have teeth: changing one constant of `buffer_bias_entries` fails
`reflection_and_driver_reads` (1 of the cases).

`cargo test --release -p ibuilder-shader --test finish_corpus --test finish_cases`: 11 tests pass (corpus 5 tests, 31 s; cases 6 tests, 0.5 s), plus the 2 `pyjson` unit tests (`cargo test --lib`); `-- --ignored` adds `full_grizzy_identical` (the 21,990 shaders; passed, 123 s on the loaded Mac).

Speed (indicative; the machine was shared with 4 other agents, load average 20-500 while I measured). The whole 21,990-shader library takes **77-91 s wall on 8 threads including 23,315 child processes** (`dmsh_xlate` and `spirv-cross`
are most of every call: summed over threads `finish` took 553-653 s = 25-30 ms a shader, of which 389-446 s were inside the native tools). **The Rust part is 3.4-3.7 ms a shader with one thread** (best of two runs on the 2,459-shader sample,
`time inside finish` minus `time in the tools`; it includes the shader's file reads and writes in `work`), 5.4-9.4 ms a shader on 8 threads under load. The instrumented Python spends about 12 ms CPU a shader on the same steps
(SHADER-STAGE-ANALYSIS section 3.2: chain 7.0 + reflection / side libraries / vin / pull 4.3 + `write_refl` 1.1, all under the GIL). Quiet-machine numbers and the phone's: not measured. Peak memory: not measured; a shader holds about five
copies of its MSL (20-45 KB each, the vertex shaders' pull library adds one).

## 4. iOS

`#![forbid(unsafe_code)]` stays; the library has no `fork` / `exec` / `Command` (the process-backed tools are in `src/bin/shader_finish.rs`). New dependencies: `regex` (1.13.2, pure Rust, already in the lock; the Python's regular
expressions are used as regular expressions here, with look-behind and back-references written by hand), `sha2` (`chain_compiler`).
`PATH=/opt/homebrew/opt/rustup/bin:$PATH cargo build --release --target aarch64-apple-ios-sim -p ibuilder-shader --lib` and `--target aarch64-apple-ios`: both build (no warning from this crate). The six differential tests, built for the simulator
(`cargo test --release --target aarch64-apple-ios-sim -p ibuilder-shader --test finish_cases --no-run`) and run in the booted iPhone 17 (`xcrun simctl spawn <SIMULATOR-UDID> <test binary>`): **6 passed**, as on the host.
The corpus tests were not run in the simulator (they need the Mac's native tools as child processes); a replay-backed `ShaderTools` for them was not written.

## 5. What is not done / not verified

* **Round 3 (was unported here, now ported; see `docs/shaders/SHADER-TESS-MESH-RUST.md` for how each was verified and for what is synthetic only)**: tessellation (`hs`, `ds`), task and mesh shaders (`ts` / `ms`), the passthrough geometry shaders'
  lean kernels (`_bl0/1/3`), geometry shaders with a transform feedback layout, the pair-level `mesh_finish` (`<vs>_<gs>_m3`, in `ibuilder-shaderbuild`). Grizzy has none of them: the oracle is the engine's Python on tool-made and
  synthetic input, and the Python's build of a synthetic game, not a game.
* **Round 3 kinds (hs / ds / ts / ms / lean / tf-gs)**: `docs/shaders/SHADER-TESS-MESH-RUST.md` section 4 lists what is synthetic only. **Verified only on synthetic input, not on a real shader** (Grizzy has none), round 2's: the `gs_slice` success path (a layer-only geometry shader), bindless handles (`tex_ubo`, `samp_ubo`, `img_ubo`, `tbuf_ubo`, `c2` arrays, handle tables), computed driver-segment indices, vertex raster facts (`pos_attr`, `uv_*`, `screen`),
  `gl_FragCoord` tile reads, `spv_single_lane`, `flatten_sets` on sets other than 0, the geometry text helpers on other shapes. The *assembly* of reflection keys that Grizzy never sets is checked only by the `tf,discard` run (`entry_tf`,
  `tf_strides`, `entry_discard`) and by reading: `first_store`, `tg_spill` (compute; the chain returns them, `finish` copies them), `entries_lost` (cannot happen in source mode), `tess`, `mesh_stage`.
* NVN2 (Switch 2) jobs (`arch sm86`, `nvn2_cs`, `stage_bank`) are `job_for` (B); `finish` treats them as ordinary stages: no oracle for them here.
* Python exceptions that `finish` does not catch (`TypeError` for a sampler name that does not decode, `KeyError` for a missing `res` field) are `reflection failed` / default values here; the Python build stops there.
* The `vs_writes` decision is the chain's (`reflect_rw` approximation, F1); `finish` re-checks it against the exact bindings and returns `unported` if they disagree (never happened).
* Timing is indicative only (shared machine); the phone's speed and memory are not measured. 

## 6. Run-time regular expressions without a compile per call (review r4 6.3, 2026-11-21)

The Python builds a few patterns from names it reads (`re.compile(f'...{re.escape(name)}...')`, with `re`'s own cache of 512). The straight port compiled a `regex::Regex` for each one at each call: four per input attribute
of every vertex shader (`entry.rs` `int_inputs`: `in\.{name}\b`, the `float _v = in.{name}.x;` declaration, `\b{_v}\b`, the copy `x = _v;`), up to three per texture sample (`fetch.rs`), one per `spvPool_*` member
(`reflect.rs`), one per entry point (`unit.rs`), one per entry / stride / struct name (`vin.rs`, `lean.rs`, `mesh.rs`, `tess.rs`). `finish/rx.rs` replaces them, each **exact by construction**:

* `rx::cached(pattern)`: the same pattern text compiled once per thread (a per-thread map of at most 256, no lock shared by the build's workers), for the patterns whose run-time part comes from a small set
  (struct and entry point names, a stride): `unit.rs` x 2, `tess.rs` (its global mutex cache replaced), `vin.rs` x 2, `lean.rs` x 2, `mesh.rs`.
* `rx::lit_then(text, lit, tail)`: the leftmost match of `escape(lit) + tail` is the first occurrence of the literal (overlapping occurrences included) after which `^(?:tail)` matches; the literal is a substring search, the
  tail a fixed pattern (cached). For the patterns whose literal holds a per-shader temporary (`float _123 = ...`): `fetch.rs` (`proj`, `frag_pos`, the sample declarations) and `reflect.rs` (`spvPool_<member>`).
* `entry.rs` `int_inputs`: the names are searched as substrings with the regex crate's own Unicode `\w` / `\b` around them (`rx::is_word`, `rx::boundary_at`, `rx::find_lit_iter`: the same non-overlapping
  leftmost matches as `find_iter`), and the two line patterns, which end in `$`, are "the line ends with the literal tail and what is before it fully matches a fixed pattern" (`DECL_HEAD`, `COPY_HEAD`).

Verified: `rx::tests` compare `lit_then` with the whole pattern on 4,000 generated texts x 9 patterns (overlaps, digits, non-ASCII, `\S`, `.+`), `find_lit_iter` with `find_iter` of `\b{lit}\b` / `in\.{lit}\b` on 3,000 texts,
`is_word` / `boundary_at` with the regex crate on 500 texts; `entry::tests::int_inputs_is_the_regex_form` keeps the old four-pattern form and compares it with the new one on the 260 vertex shaders of `tests/data/finish_cases.json`
(105 with integer inputs) and 10 mutations of each (2 in a debug build): 2,860 texts, equal. On those 2,860 texts the old form took **11,785 ms and the new one 91 ms** (release, load average ~200: about 4 ms against 0.03 ms a vertex
shader). The Python oracle tests (`tests/finish_cases.rs`, 6; `tests/tessmesh_*`) pass unchanged. **End to end, NOT proven**: the full stage 3 of Ocarina of Time (42,848 shaders, every one through `finish`) was built before and after these edits (473.4 s wall each, 9,109,648,504-byte library, 119,517 files both), but the whole-image sha256 differs as it must (the `c1_full.tsv` / `cb_extent.tsv` header carries the build time), and the per-file comparison that would show the shaders' files equal was lost with the machine's reboot (the extracted game is gone): `docs/shaders/SHADERBUILD-RUST.md` section 8.

**Before / after on every fixture input** (2026-11-21, after the machine's reboot took the real game): a private harness (outside the repository, `target/agent-work/`) builds the sources as they were before these edits
(every replacement applied backwards) and after them side by side, and runs both over every MSL text of `tests/data/finish_cases.json` (1,590) and 20 mutations of each (insertions, deletions, duplicated lines around
`float `, `in.`, `struct `, `spvPool_`, `main0`, ...: 33,390 texts) through `entry::int_inputs`, `fetch::screen_varyings`, `unit::unit` + `fetch::fold` + `unit::entries` + `unit::shared_body`, `vin::vin_variant` +
`pull_variant`, `reflect::reflect` + `raster_facts` + `attributes` + `drv_reads`, and the 948 cases of `tests/data/tessmesh_cases.json` with 20 mutations each (19,908) through `difftest::run` (`tess.rs`, `mesh.rs`, `lean.rs`):
**0 different** answers, panics included. Its power: a dropped space in `fetch`'s literal gives 1,760 differences, `int_inputs` without its `\b` test 151 (two mutants the fixtures do not reach, `\d*` for `\d+` in a sample
tail and `lit_then` skipping overlapping occurrences, are not seen by it; the second one is `rx::tests`'). The harness found an older bug on 390 of the mutated texts, the same in both versions: `unit::unit` sliced
`msl[at..e.start]` with `at` past `e.start` when two entry points share a body (a header without one, then the next entry point) and panicked, where the Python's slice is empty. Fixed with the Python's empty slice
(`unit.rs`, `tests::two_entry_points_sharing_a_body_do_not_panic`); those 390 texts are then the only difference between before (panic) and after (an answer).
