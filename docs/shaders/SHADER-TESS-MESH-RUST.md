# Tessellation, task / mesh shaders and passthrough geometry in Rust (round 3, package SHADERS)

Context: `docs/CONVENTIONS.md`, `docs/shaders/SHADER-BACKEND-PLAN.md`, `docs/shaders/SHADER-FINISH-RUST.md` (what `ibuilder_shader::finish` ports), `docs/shaders/SHADERBUILD-RUST.md` (the build driver), `docs/shaders/SHADER-RUST.md` (the chain).
This file: what round 3 ported, how to run it, how it was verified (exact commands and counts), what is only synthetic, what is still not done.

Python source ported: the engine copy `$OLD/shader/engine/dm/gfx/tools/` (`diff -rq` empty against `iOSBuilder/engine/dm/gfx/tools`, files dated 2026-11-16): `dmsh_tess.py` 26,638 bytes (sha256 bd91b9a3...), `dmsh_meshstage.py` 26,725
(0994bbb1...), `dmsh_mesh.py` 12,362 (8f7932fc...), `dmsh_geometry.py` 16,714 (5aaf9fee...), `dmsh_build.py` 99,679 / 1,157 lines (e71ffba3...: `job_for` l.365-460, `partner_reads` l.551-566, `finish` l.737-875, `geometry_finish` l.656-736,
`mesh_finish` l.1083-1117), `dmsh_pull.py` (`variant`, already ported in round 2), and the phone's driver `gameapp/shader_device.py` (37,785 B, as before). The engine's own tests (`dm/gfx/tests/test_dmsh_*.py`) were read for
the shapes of real input and, for four of them, **run through the real translator** (section 3.2).

## 1. Result

| kind | what | Rust | verified against the Python by |
|---|---|---|---|
| **hs / ds** (tessellation) | `dmsh_tess.hull` / `domain` / `spirv_io` / `mode_bits` / `control_mode`, the `finish` flow (`tess` in the reflection, `unsupported` shapes keep the source, the domain's two depth-convention entry points and capture entry) | `finish/tess.rs` | 955 + 150 unit cases on real glslang + SPIRV-Cross output and its mutations; 6 ds + 5 hs of the synthetic game (3 hs shapes from the real translator) end to end |
| **ts / ms** (Switch 2 task / mesh) | `dmsh_meshstage.info / memory / own_shared / pixel_reads / to_stage`, the `job_for` branch, `partner_reads`, the `finish` flow (`mesh stage failed`, `mesh_stage`, `entry_z`, `entry_object`, `mesh_spill`) | `finish/meshstage.rs`, `finish/mod.rs`, `ibuilder-shaderbuild/src/{jobs,mesh}.rs` | 69,600 unit cases; 64 synthetic `job_for` records; 129 task / mesh shaders (30 ts, 99 ms: 100 with libraries, 29 refused identically) translated by the real translator, end to end |
| **mesh libraries** (`<vs>_<gs>_m3`) | `dmsh_mesh.compose / out_bytes / prims_for` and the pair-level `mesh_finish` | `finish/mesh.rs`, `ibuilder-shaderbuild/src/mesh.rs` | 30,000 composition + 2,400 out_bytes + 738 prims_for unit cases; the synthetic game's pairs end to end |
| **passthrough gs** (`_bl0/1/3`) | `dmsh_geometry.lean`, the lean branch of `geometry_finish` | `finish/lean.rs`, `finish/geometry.rs` | 30,000 unit cases; the engine's own passthrough geometry program through the real translator |
| **gs with a transform feedback layout** | `vs_tf(geometry=True)` on the vertex reader (`entry_tf`, `tf_strides`) | `finish/geometry.rs` | end to end (two gs with 1 and 2 feedback streams) |

Everything the Python ported here returns, the Rust returns **byte for byte** on every input I could build; where the Python raises, the Rust returns the same `ValueError` message (`finish` records it in the reflection for tessellation, in
the build log otherwise) and the same status. **No game's shader of these kinds was available** (Grizzy has none of them, and no other dump may be used): the evidence is synthetic or tool-made (section 3), and says so below.
`dmsh_xlate` translates only Switch binaries, which I do not have; what I used instead is the engine's own hand-assembled test programs (section 3.2) and glslang-compiled SPIR-V.

The Grizzy regression is unchanged (section 3.4).

## 2. Python -> Rust and how to run it

| Python | Rust |
|---|---|
| `dmsh_tess.py` (`TESS_MSL`, `mode_bits`, `control_mode`, `spirv_io`, `_struct`, `_location`, `_slot_struct`, `_words`, `hull`, `domain`) | `crates/ibuilder-shader/src/finish/tess.rs`; `TESS_MSL` in `tess_text.rs`, **generated** from the Python by `tools/tessmesh/gen_texts.py` |
| `dmsh_meshstage.py` (`is_stage`, `sph_outputs`, `sph_system`, `info`, `memory`, `own_shared`, `pixel_reads`, `to_stage`, `_device`, `_params`, `_KERNEL`) | `finish/meshstage.rs` (`MeshInfo` = the info dict, JSON in the Python's key order) |
| `dmsh_mesh.py` (`out_bytes`, `prims_for`, `compose`) | `finish/mesh.rs` |
| `dmsh_geometry.lean`, `passthrough` | `finish/lean.rs`, `finish/geometry.rs` |
| `dmsh_build.finish` l.793-799 (tess), 800-804 (mesh), 829-832 (ds capture), 854-868 (the reflection's `mesh_stage`, `entry_z`, `entry_object`, `mesh_spill`, `tess`), `partner_reads` | `finish/mod.rs` (`finish_inner`, `partner_reads`, `Partners`, `finish_partners`) |
| `dmsh_build.geometry_finish` (lean, tf) | `finish/geometry.rs` `finish_gs` |
| `shader_post.post_rewrites` for hs / ds / a mesh job | `post.rs` (`Stage::Hs/Ds`, `Job::mesh`: the generic steps, no stage tail; a mesh job skips the compute tail) |
| `dmsh_build.job_for` ts / ms branch | `crates/ibuilder-shaderbuild/src/jobs.rs` |
| `dmsh_build.mesh_finish` and its loop | `crates/ibuilder-shaderbuild/src/mesh.rs` (`build_mesh_libs`), called by `build.rs` |
| Python's `splitlines`, `strip`, `repr`, `int()` (their results reach a message) | `finish/pycompat.rs` |

Tools (all under `crates/ibuilder-shader/tools/tessmesh/`; the Python is run from COPIES of the engine, `PYTHONDONTWRITEBYTECODE=1 python -I`):

```
S2=$SCRATCH; OLD=$SCRATCH_OLD
rsync -a --exclude __pycache__ $OLD/shader/engine/ $S2/sh3/engine/                       # the engine's Python, a copy
python -I gen_corpus.py $S2/sh3/engine $S2/sh3/corpN [--scale F] [--seed N] [--only tess,mesh,lean,compose]   # a corpus: cases.tsv + inputs
./run.sh $S2/sh3/corpN [step]            # stepdiff.py (the engine's functions) and step_diff (Rust) over the cases, then tools/difftest/compare.py
python -I make_game.py $S2/sh3/engine $S2/sh3/enginetests <dir> [--random N]   # a synthetic game (+ ./e2e.sh <name> [--random N]: Python build, Rust build, libdiff, job logs)
MUT_ROOT=<private workspace copy> CARGO_TARGET_DIR=<own dir> python3 mutate.py <corpus>   # the harness's teeth: one edit of the Rust at a time (section 3.5); mutate_py.py: of the engine's Python
python -I mkfixture.py <corpus> tests/data/tessmesh_cases.json --per-step N      # the committed subset
cargo test --release -p ibuilder-shader --test tessmesh_cases --test tessmesh_finish
cargo test --release -p ibuilder-shaderbuild --features host-tools,f2-finish --test tessmesh_e2e --test jobs_synthetic
```
The registry `ibuilder_shader::difftest::run` (step names `tess_hull`, `tess_domain`, `tess_mode`, `mesh_info`, `mesh_to_stage`, `mesh_pixel_reads`, `mesh_compose`, `geo_lean`, `mesh_out_bytes`, `mesh_prims_for`) and
`tools/difftest/stepdiff.py` are the two halves (the format of `docs/shaders/SHADER-RUST.md` 3.2: status, text unchanged, length, crc32, a json of everything else, the message).

## 3. Verification (everything below was run; command, then result)

The machine was shared (load average 270-400 for most of it): timings are not given for this work, and 31 of the 110,600 cases of the largest corpus hit the 1 s per-case timer of `stepdiff.py` (a `timeout` in the Python half, never in
the Rust); those 31 cases were re-run with `STEPDIFF_TIMEOUT=120` and agree with the Rust.

### 3.1 Per-function differential tests (`tools/tessmesh/run.sh`)

Oracle: the engine's own functions (`dmsh_tess.hull / domain / control_mode`, `dmsh_meshstage.info / memory / own_shared / pixel_reads / to_stage`, `dmsh_mesh.compose / out_bytes / prims_for`, `dmsh_geometry.lean`) imported
from a copy of the engine and run by `stepdiff.py` on the same inputs as the Rust (`step_diff`); every field must be equal: status (OK, or ERR for any Python exception), whether the text is unchanged, its length and crc32, the
json of everything else the function returns (facts, `spill`, `memory`, `reads` ...), and, for the Python's `ValueError`s, the message (`compare.py` reports them separately: every `ValueError` message is identical).

Inputs (`tools/tessmesh/gen_corpus.py`, seeded, no game data):
* **tessellation**: random GLSL (1-32 control points; 0-4 inputs, outputs and patch outputs of float4, int4, uint4, float3, float2, float and `vec4[2]`; random locations; triangles / quads / isolines; 3 spacings; 2 windings;
  the domain's float2 coordinate of a quads layout) compiled by `glslangValidator`, stripped by `spirv-opt --strip-debug` and turned into MSL by **the engine's own `spirv-cross` with `finish`'s flags**: the real output shape
  (`m_<id>` names, `main0_in`, `spvIndirectParams`, `patch_control_point<main0_in>`) with real SPIR-V decorations; plus mutants (a character, a token, a digit, a line damaged / duplicated / swapped / truncated; SPIR-V instructions dropped, a
  Location or BuiltIn literal changed, a storage class changed, a file truncated to a size that is not a multiple of four) and the job's `tess_mode` varied independently (any of the 3 modes, 3 spacings, 2 windings, or none);
* **task / mesh shaders**: kernels in the shape of `test_dmsh_meshstage.py` (`spvUnsafeArray<uint, N>` as the kernel's first statement, parameters of 1-4 of 12 kinds, 0-3 declarations before the array, barriers, byte-store helpers, a
  `threadgroup` word in comments and calls) with random info dicts (ts / ms, 1-256 threads, payloads, tasks, 0-5 vertices a primitive, 0-300 vertices and 0-600 primitives, point size / position / 0-4 generic attributes x 4 components,
  per-primitive attributes, 0-2 system values, builtins, array sizes from `memory()` and random ones across the spill limits 1,500 / 4,096 words) and `reads` dicts (none or random masks); control sections and SPH areas
  with random facts (stage bytes 0-7, short buffers, a per-primitive SPH past the area); pixel SPH input maps; `prims_for` at and around each exact limit;
* **lean kernels and mesh compositions**: the engine's `LEAN_KERNEL` shape (`test_dmsh_geometry.py`) with 1-6 capture reads, random strides, atomics, locals, mask sources, and 8 kinds of damage (computed offsets, a wrong stride,
  a store the pattern does not know, a missing mask store ...); vertex libraries in the shape of `MESH_VS` (1-3 stage inputs of float4 / uint4 / int4, 1-3 outputs, point size, entries with `vertex_id` / `instance_id` /
  `base_vertex`, resources at buffer slots 2, 13, 30, 31, 40, `[[buffer]]` without an index) composed with the lean kernel `lean` made of them (or damaged, with reads that include clip z, or none).

Result (`compare.py`; three corpora: seeds 20261119 scale 0.5, 777 scale 10 and 99 scale 1.5; the totals):

| step | cases | identical | different | the Python returns a result | the Python raises |
|---|---|---|---|---|---|
| `tess_hull` | 418 | 418 | 0 | 280 | 138 |
| `tess_domain` | 537 | 537 | 0 | 318 | 219 |
| `tess_mode` (`control_mode` + `mode_bits`) | 150 | 150 | 0 | 150 | 0 |
| `mesh_info` (`info` + `memory` + `own_shared`) | 18,000 | 18,000 | 0 | 7,893 | 10,107 |
| `mesh_to_stage` | 48,000 | 48,000 | 0 | 23,200 | 24,800 |
| `mesh_pixel_reads` | 3,600 | 3,600 | 0 | 2,394 | 1,206 |
| `mesh_compose` | 30,000 | 30,000 | 0 | 17,557 | 12,443 |
| `mesh_out_bytes` | 2,400 | 2,400 | 0 | 1,644 | 756 |
| `mesh_prims_for` | 738 | 738 | 0 | 517 | 221 |
| `geo_lean` | 30,000 | 30,000 | 0 | 26,309 | 3,691 |
| **all** | **133,843** | **133,843** | **0** | 80,262 | 53,581 |

(The "Python raises" column holds the error paths the task asked for: refused shapes of every module, `struct.error`, `IndexError`, `KeyError` and `TypeError` the Python does not catch: the Rust returns an error there too; for those the message is
not the Python's text, for every `ValueError` it is.) What the successes cover (`to_stage`, from the 2,000-case corpus: 969 ok): 590 mesh and 379 task functions; 97 spill to a device slice, 301 no-task meshes with an object function,
287 with a primitive-id builtin, 304 with padded outputs, 77 point and 86 line topologies, 589 with `reads`.
The tessellation corpus is the smallest (955 programs' functions + 150 control sections): generating it needs 6 external processes per program and the machine's load made a larger run (scale 2, 800 programs) impossible in the time; the
hull / domain functions are small (their branches are the error messages, all of which the corpus reaches except those of a SPIR-V the Python cannot see) and are also covered by the synthetic game (3.3).

A committed subset (948 cases, 1.7 MB: `tests/data/tessmesh_cases.json`, `tools/tessmesh/mkfixture.py`) runs as a plain test, with no Python or tool needed:
`cargo test --release -p ibuilder-shader --test tessmesh_cases`: **1 test (948 cases) passes**. Four more tests (`tests/tessmesh_finish.rs`: `finish` with stand-in tools) check the plumbing: a mesh shader with pairs is
`unported` through the locations-only entry point and finishes through `Partners`, padding what its pixel shaders read; the refused / failed statuses; an unsupported domain keeps its source and records `tess.unsupported`; a
SPIR-V of a size that is not a multiple of four stops the shader (`fatal`), as the Python's `struct.error` stops its build.

### 3.2 Real tool output, and what the translator does and does not give

* **SPIRV-Cross is the real binary** (the engine's, `vulkan-sdk-1.4.363.0`) on every SPIR-V of the tessellation corpus and of the synthetic game; glslang 16.6.0 and SPIRV-Tools 2026.4 (Homebrew) make the SPIR-V.
* **`dmsh_xlate` translates only Switch binaries and I have no game's.** What I have instead is the engine's own tests, which assemble tiny Switch programs by hand and run them through the real translator. I wrapped those same
  programs in split records and had the real `dmsh_xlate` translate them: the hull of `test_dmsh_hull_cbe.py` (+ its variant with an address that is not base + lane), the passthrough geometry shader of `test_dmsh_geometry.py` (its
  5 topology variants and its vertex readers; also with 1 and 2 transform feedback streams in the control section, and a non-passthrough one), and the stage-memory kernels of `test_dmsh_ampere_mem.py` (`StageMemory`: workgroup
  number, input, payload, attribute, read-back, count, index, zero-count encodings, with the job's `stage_memory` and `shared` that `job_for` makes from the record). These are real translator output for **a hull shader, a passthrough
  geometry shader and task / mesh kernels** (the shapes the port had never seen from anything but my own text). There is no translated **domain** shader: none of the engine's tests assembles one, so `ds` was
  verified with glslang SPIR-V only (translator-style `m_<id>` names; the real translator always declares the default triangles / equal-spacing domain, which the corpus follows).
* No Switch game, dump or shader of the engine's graphics stages was used or written: the code areas are the tests' own hand-assembled words plus zeros.

### 3.3 End to end against the Python's own build of a synthetic game

`tools/tessmesh/make_game.py` makes a game: split packages (`BTCDBTCD` records, `hs_main.bin` ... `ts_main.bin`), a profile folder (shader map, answer table, pair tables, `geo_mesh.tsv` mesh pairs) and, for the shaders the real
translator is not asked about, prepared answers (`fake_xlate.py` serves them and sends every other job to the real `dmsh_xlate`; both builds run the same script). `e2e.sh` then runs **the Python's build** (`shader_device.build`,
the phone's driver over the engine's Python: `job_for`, the translator, `finish`, `mesh_finish`, `publish_library`) and **the Rust build** (`shaderbuild build --finisher rust`) over it and compares:

```
tools/tessmesh/e2e.sh e2e-r120 --random 120 --seed 31
  shaders 181: ok 152, mesh stage failed 29            (Python and Rust log the same lines; mesh libraries: built 4, missing a library 2, not composed 1)
  libdiff:  files A 409 B 409: identical 409, different 0, only in A 0, only in B 0       (cmp: the two images are byte-identical)
  translator jobs: 201 job files, identical in both builds (job text and code sha256)
cargo test --release -p ibuilder-shaderbuild --features host-tools,f2-finish --test tessmesh_e2e   # the same comparison as a test: pass
```
The game: 181 shaders, **135 translated by the real translator** (2 hs, 4 gs, 30 ts, 99 ms) and 46 from glslang answers (3 vs, 34 ps, 3 hs, 6 ds). Every kind appears with its failure modes: 7 tessellation rewrites
and 4 `tess.unsupported` (an isolines mode, no mode, a 3-component input, a 3-component patch output), 70 mesh functions (27 with an object function, 11 with a device-slice array) and 29 refused identically (a primitive vertex count of 0 or 4, more
vertices or primitives than a Metal mesh, shared memory of the shader's own), 18 lean library files (`_bl0/1/3` json + metal of 3 of the 4 geometry shaders: one is not passthrough), 4 `_m3` libraries composed, 2 pairs missing a library and 1 not composed (identically), a geometry
reader with `main0_tf`. Mesh shaders are paired with 0-3 pixel shaders whose NVN1 / NVN2 SPH input maps are random (`partner_reads`). The earlier runs of the same game (`--random 0`: 31 shaders, 133 files; `--random 40`: 81 shaders, 227 files) were identical too. The Python build was run once per game and its image kept only until compared.

The `job_for` mesh branch has its own oracle: `crates/ibuilder-shaderbuild/tools/gen_synth_jobs.py` (the Python's `job_for` extracted from `dmsh_build.py` by `ast`, run on seeded synthetic records) was extended with 64 NVN2 task / mesh records
(random control facts and SPH areas, some with an LDS / STS / ATOMS opcode) and 2 malformed ones: `cargo test --release -p ibuilder-shaderbuild --features host-tools,f2-finish --test jobs_synthetic`: **142 identical jobs (48 of them task / mesh jobs with `mesh_stage`,
`stage_memory`, `local`, `shared`), 3 identical `sys.exit` messages, 16 records where the Python crashes (a short control section, an SPH past the code area, a stage byte that is neither) and the port reports an error.**

### 3.4 The Grizzy regression (must stay identical)

All of it unchanged by round 3 (the chain and `finish` for ps / vs / cs / gs were not edited apart from the new stage variants, the `Job::mesh` flag and the lean / tf branches of the geometry path, which Grizzy's 145 gs never reach).

```
cargo test --release -p ibuilder-shader -p ibuilder-shaderbuild --features host-tools,f2-finish      # CARGO_TARGET_DIR=$SCRATCH/target-sh3
```
**58 tests pass, 0 fail, 4 ignored** (the three big Grizzy runs of `docs/shaders/SHADER-FINISH-RUST.md` / `docs/shaders/SHADER-RUST.md` / `docs/shaders/SHADERBUILD-RUST.md`, run by hand below, and the 2,459-shader sample, run in those docs). ibuilder-shader: 6 unit (2 `pyjson`, 2 `pycompat`, 2 `tess`),
`finish_cases` 6, `finish_corpus` 5 (the 401-shader and the four 2,459-shader finish runs against the Python library: identical), `oracle` 4 (53 jobs, a 2,439-shader sample, the image writer, the tables), `steps` 2 (the 272,014 per-step cases of `docs/shaders/SHADER-RUST.md`
3.2: identical), `tessmesh_cases` 1 (948 cases), `tessmesh_finish` 4, `units` 4; ibuilder-shaderbuild: 11 unit, `driver` 6, `jobs_oracle` 2 (2,860 comparisons), `jobs_synthetic` 1, `live_oracle` 2, `real_build` 1, `tables_streamed` 1,
`tessmesh_e2e` 1, `translate_oracle` 1. The log: `$SCRATCH/sh3/regress-tests.log`.

The full library, with the Rust finisher and the real tools (the command of `docs/shaders/SHADERBUILD-RUST.md` section 2):
```
shaderbuild build --pkg $OLD/shader/split-out/pkg --profile $OLD/shader/profile --game 0100DFA027504000 --out <dir> --chain b24b25240cad --jobs 6 --tables 1 --pulling 0 --cas <dir>/cas --engine <engine dir> --finisher rust
libdiff <dir>/b-b24b25240cad.img $OLD/shader/runs/full-gil-j6/b-b24b25240cad.img --ignore-first-line c1_full.tsv --ignore-first-line cb_extent.tsv
shaders 21990: ok 21990 (failed none); library stamp inputs 8ea0535d1d1c; c1_full.tsv not proven bounded: 6679 shaders; cb_extent.tsv 49309 slots, 1578 read-only storage slots, 21522 shaders
mesh libraries (<vertex>_<geometry>_m3, dmsh_mesh.py): none
files A 56885 B 56885: identical 56883, identical but for the time stamp line 2, different 0, only in A 0, only in B 0     (the image is 1,706,277,556 bytes, the Python's size)
REPORT wall 126 s (load average ~300: 417 s inside the translator and 23,016 spirv-cross calls 365 s, summed over 6 threads); peak RSS 155 MiB
```
(The images and the cache were deleted after the comparison.) `cargo test --release -p ibuilder-cli --features shader-host-tools`: **17 pass, 0 fail, 2 ignored** (1 unit, 8 `cli`, 4 `mods_chain`, 3 `shaders_glue`, 1 `shaders_pipeline` which runs the 401-shader pipeline request with the real finish and tools; the two ignored are the heavy Grizzy end-to-end runs, not run here).

### 3.5 Does the harness have teeth?

Breaking the implementation on purpose, one edit at a time, and counting the cases of the harness that then differ. Two ways, both on the 6,635-case corpus: an edit of the **Rust** (`tools/tessmesh/mutate.py`; each needs a rebuild: only
the first four finished before the machine's load made the rest impractical, and it must run on a private copy of the workspace, never in place: an in-place attempt of mine briefly mutated the shared sources, see section 6), and an edit of
the **engine's Python** copy, which needs no rebuild (`tools/tessmesh/mutate_py.py`; the harness compares two implementations, so a one-sided edit shows the same thing).

| edit | cases | differ |
|---|---|---|
| Rust: `hull`: `in_bytes += 16` -> 15 | 418 | 280 |
| Rust: `_location`: `l < 32` -> `l <= 32` | 955 | 1 |
| Rust: `mode_bits`: the winding bit 3 -> 2 | 687 | 177 |
| Rust: the domain prologue's `dm_i < 32u` -> `31u` | 537 | 318 |
| Python: `in_bytes` 16 -> 15 / winding bit / `control_mode` spacing / patch flag of a patch input | 418 / 537 / 150 / 537 | 280 / 160 / 10 / 156 |
| Python: `memory`: payload rounding 15 -> 16 | 2,750 | 905 |
| Python: `to_stage`: the spill limit `<=` -> `<` (only the exact limit of 1,500 words) | 2,000 | 8 |
| Python: `_width`: component 3's mask 8 -> 4 | 2,000 | 215 |
| Python: `own_shared`: one opcode dropped | 750 | 11 |
| Python: `pixel_reads`: `2 * c` -> `c` | 150 | 93 |
| Python: the point topology's name | 2,000 | 77 |
| Python: `out_bytes`: half is 4 bytes | 1,350 | 10 |
| Python: `prims_for`: `4 * p` -> `5 * p` (`3 * p` is an equivalent mutant: no vertex size lies between the two limits) | 3,858 | 16 |
| Python: `compose`: the run-record slot limit `<` -> `<=` (only a resource at slot 31) | 3,750 | 17 |
| Python: `lean`: the mask word 5 -> 6 | 2,500 | 1,127 |
| Python: `lean`: the dead-code use count -2 | 1,250 | 949 |
| Python: `lean`: the "record base used elsewhere" test `>` -> `>=` | 1,250 | 1,106 |
| Python: `lean`: the renumbered stride | 1,250 | 1,106 |

Two of these were first **not** detected (`prims_for`: the equivalent mutant above, replaced by `5 * p`; `compose`: no resource at slot 31 as a reference in the corpus): the generator got the missing cases and both are detected now (`corp9`).
The location edit differs in a single case (an exact location of 32 is rare): the corpus is thin there.

### 3.6 iOS

`#![forbid(unsafe_code)]` stays; no `fork` / `exec` / `Command` in either library (the process-backed tools are in `host-tools` / `src/bin`); no new dependency (`regex`, `serde_json`, `sha2` as before; `rayon` in `mesh.rs`).
```
PATH=/opt/homebrew/opt/rustup/bin:$PATH IPHONEOS_DEPLOYMENT_TARGET=14.0 cargo build --release --target aarch64-apple-ios-sim -p ibuilder-shader -p ibuilder-shaderbuild --lib   # Finished in 35 s
PATH=/opt/homebrew/opt/rustup/bin:$PATH IPHONEOS_DEPLOYMENT_TARGET=14.0 cargo build --release --target aarch64-apple-ios     -p ibuilder-shader -p ibuilder-shaderbuild --lib   # Finished in 38 s
```
Both build (no warning from these crates). The two new test binaries built for the simulator (`cargo test --release --target aarch64-apple-ios-sim -p ibuilder-shader --test tessmesh_cases --test tessmesh_finish --no-run`) ran in the
booted iPhone 17 (`xcrun simctl spawn <SIMULATOR-UDID> <binary> --nocapture`): **`tessmesh_cases` 1 test (948 cases, every case identical to the Python's stored result) and `tessmesh_finish` 4 tests pass**, as on the host.
Not run in the simulator: the end-to-end test (it needs the Mac's `dmsh_xlate` and `spirv-cross` as child processes) and `tessmesh_e2e` is therefore host-only; `ibuilder-ffi --features real-stages` was not rebuilt for iOS in this round (nothing in it changed).


## 4. What is verified only synthetically, what is not verified

* **No real game shader of any of these kinds** went through this port (Grizzy has none; no other dump was available or may be used). What stands in: the engine's own hand-assembled programs through the real translator (a
  hull, a passthrough geometry shader, task / mesh stage-memory kernels), glslang SPIR-V through the real SPIRV-Cross (hull / domain), hand-shaped and mutated MSL (mesh kernels, lean kernels, vertex libraries). A shape of
  SPIRV-Cross output or of a real mesh kernel I did not think of is the risk, as in `docs/shaders/SHADER-RUST.md` section 5: run `tools/tessmesh/` (the corpora, `e2e.sh`) over a new game's captured translations first.
* **`ds` has no real translator output** (see 3.2); **the lean kernel was checked on the translator's passthrough kernel of the engine's test program only** (a viewport-mask kernel with no storage writes of its own): real games'
  lean kernels with atomics and subgroup operations are covered by the synthetic text (`_PURE` words, array elements, record bases), not by a real one.
* The tessellation text functions reproduce `dmsh_tess` byte for byte; **whether the host's expansion kernel and the Metal compiler accept the rewritten MSL was not checked** (no Metal compile in these tests: source mode ships the text, as
  for every other shader; a unit that does not compile is only found when the host loads it, as `shader_device.py` says).
* Python exceptions that `finish`, `job_for` and `mesh_finish` do not catch (`struct.error`, `IndexError`, `KeyError`, `TypeError`, `UnicodeDecodeError`) stop the Python's build; here they are `fatal` / `Red` / `crashed` results for that shader or
  pair, with a message that is not the Python's text. Integers are `i64` / `i128` where Python's are unbounded (a stage-memory array of 2^63 words, a 40-digit number in a location attribute are not reproduced); text is ASCII
  (`\w`, `\s`, `\d` of Python are Unicode-aware; `str.splitlines` and `strip` are reproduced exactly where the tessellation member lists use them).
* `finish_with_locs` (the older entry point) cannot finish a mesh shader that has pairs (`unported`: it has no way to read the pixel shaders' SPH): callers use `finish_partners` (the build driver does, through `Finisher::finish_with`).
* The pipeline adapter (`ibuilder-cli`) and the app (`ibuilder-ffi`) were not changed; `BuildReport::mesh_pairs_unported` is kept (always 0) for them.
* Mesh libraries are composed on every build and not cached (a text rewrite of two cached files; not timed); they are written under `<out>/_work/mesh/` and streamed into the image, so their size does not grow the build's memory.

## 5. Still not ported

Nothing that `finish` / `job_for` returned `unported:` for in round 2 is left for ps, vs, cs, gs, hs, ds, ts or ms. What remains `unported` is **defensive**: a stage name other than ps / vs / cs / hs / ds / gs / a mesh job (`unported: stage <name>`), a key that is not in the shader map,
a chain step that returns `PostError::Unported` (the two internal ones: no answer json for a vertex shader, a transform feedback layout missing from its job), the chain's `vs_writes` approximation disagreeing with the reflection
(never happened), and a mesh shader with pairs through the locations-only entry point. `ShaderBackend::post_rewrite` (the captured-manifest mode of the pipeline trait) is still the trait default, as `docs/shaders/SHADERBUILD-RUST.md` says.

## 6. A note on how this was built (shared files)

While I ran the Rust mutation test I edited sources of `crates/ibuilder-shader/src/finish/` **in place** (restoring after every build), which another agent's `cargo test --workspace` could have compiled mid-mutation, for about fifteen minutes
(the first four mutations of 3.5; no source was left changed: every mutation site was re-checked to hold its original text). `tools/tessmesh/mutate.py` now refuses to run outside a private copy of the workspace. A test failure of someone
else in that window that points into `finish/tess.rs`, `meshstage.rs`, `mesh.rs`, `lean.rs` or `pycompat.rs` is that, and goes away on a rerun.

## 7. Files

`crates/ibuilder-shader/src/finish/{tess,tess_text,meshstage,mesh,lean,pycompat}.rs`, `src/finish/{mod,geometry}.rs` (edited), `src/{post,difftest,vs,oracle}.rs` (edited: `Stage::Hs/Ds`, `Job::mesh`, the registry's round-3 steps, `output_words` public),
`src/bin/step_diff.rs` (sorted meta), `tests/{tessmesh_cases,tessmesh_finish}.rs`, `tests/data/tessmesh_cases.json`, `tools/tessmesh/{gen_texts,gen_corpus,run.sh,make_game,fake_xlate,run_py_build,e2e.sh,mkfixture,mutate,mutate_py}.{py,sh}`,
`tools/difftest/stepdiff.py` (the round-3 steps; `STEPDIFF_TIMEOUT`);
`crates/ibuilder-shaderbuild/src/{jobs,mesh,finish,finish_adapter,build,lib}.rs` (edited / new), `src/bin/shaderbuild.rs` (report line), `tests/{tessmesh_e2e,jobs_synthetic}.rs`, `tools/gen_synth_jobs.py`; this file, `docs/shaders/SHADER-FINISH-RUST.md`, `docs/shaders/SHADERBUILD-RUST.md`.
Oracle runs (not in the repo, deleted after use apart from small logs): `$SCRATCH/sh3/{e2e-r120, corp2, corp9, synth, *.summary.txt, mutate*.log}`.
