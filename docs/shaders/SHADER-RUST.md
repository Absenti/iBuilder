# `ibuilder-shader`: the Rust side of stage 3

Analysis and the numbers behind the choices: `docs/shaders/SHADER-STAGE-ANALYSIS.md`. The per-shader `finish()` around the chain (reflection, side libraries, ...): `docs/shaders/SHADER-FINISH-RUST.md`.
This file: what is ported, how to run it, what is verified (exact commands), what is not. **State of 2026-11-19 (round 2, package F1): the chain has no gated step left for ps / vs / cs.**

## 1. What it ports (Python source -> Rust module)

| Python (engine `dm/gfx/tools`, `dm/tools`, iOSBuilder `tools/`) | Rust (`crates/ibuilder-shader/src/`) | status |
|---|---|---|
| `tools/shader_post.py` `post_rewrites` = `dmsh_build.py finish()` l.762-868 (`check_msl` ... `blend_zero`, then the per-stage tail), for **ps / vs / cs** | `post.rs` (driver), `steps.rs`, `guards.rs`, `utest.rs`, `border.rs`, `blend.rs`, `fsuint.rs`, `side.rs`, `vs.rs`, `zsnap.rs`, `wide.rs`, `text.rs` and the files below | **ported, ungated** |
| `dmsh_atomic.{check_msl, wide_atomics}` | `steps.rs`, `atomic.rs` | ported (round 2: `wide_atomics`) |
| `dmsh_rewrite.{per_vertex, bit0_mask, mul_extended, float_add_msl, gather_offsets, shared_store_msl, hmma_msl (+ hmma_bslots)}` | `pervertex.rs`, `bit0.rs`, `ampere.rs` (mul / float add / gather / shared store), `hmma.rs` | ported (round 2) |
| `dmsh_rewrite.{precise, membar_fence, tg_fence, max_threads, uniform_tests, border_emu, blend_zero}`, `dmsh_fetch.select_guards` | `steps.rs`, `utest.rs`, `border.rs`, `blend.rs`, `guards.rs` | ported (round 1) |
| `dmsh_rewrite.{frag_z_snap, first_store, tg_spill}` | `zsnap.rs`, `cs.rs` | ported (round 2; round 1 had the analyses only) |
| `dmsh_reflect.pool_arrays`, `dmsh_wide.wide_loads` | `pool.rs`, `wide.rs` | ported (round 2) |
| `dmsh_entry.{vs_init, vs_layer, vs_depth, vs_discard, vs_writes, vs_layer_entry, drop_point_size, fs_uint, uint_entry, fs_dual}`, `dmsh_geometry.{capture, output_words}`, `dmsh_build.vs_pad` | `vs.rs`, `fsuint.rs` | ported |
| `dmsh_entry.vs_base_instance`, `dmsh_build.{vs_tf, tf_layout}` | `vs.rs`, `tf.rs` | ported (round 2) |
| `dmsh_reflect.reflect` (only the `rw` flag and the ValueErrors that depend on the argument buffer's members, for a vertex shader) | `vs.rs` `reflect_rw` | the part the chain needs; the full reflection is F2's (`finish/reflect.rs`) |
| `dmsh_side.{entry, shared_body, body, calling}` | `side.rs` | ported |
| `c1_guard.py` (all of it incl. `Interval`, `definitions_of`) and `cb_extent.py` | `interval.rs`, `tables.rs` | ported, parallel (rayon) |
| `shader_lib.write`, `romfs.Image` (the `DMROMFS1` library image) | `romfs.rs` | ported (flat library layout) |
| fixed MSL text the rewrites insert | `templates.rs`, **generated** from the engine's Python by `tools/gen_templates.py` (nothing retyped) | |
| registry of the steps under their Python names, for the differential tests | `difftest.rs`, `src/bin/step_diff.rs` | test support |

**What is still gated** (`Err(PostError::Unported(..))`), each with its trigger and why; nothing else is:

| trigger | why |
|---|---|
| `job.stage` is not ps / vs / cs / hs / ds (gs: a different code path of `finish()`, `geometry_finish`, F2's `finish/geometry.rs`) | round 3: `hs` / `ds` run the generic steps (`Stage::Hs/Ds`, no stage tail; `dmsh_tess` is `finish/tess.rs`), a task / mesh job is `cs` with `Job::mesh` (no compute tail; `dmsh_meshstage` is `finish/meshstage.rs`): `docs/shaders/SHADER-TESS-MESH-RUST.md`. Grizzy has 145 gs and none of the others |
| a vertex shader without the translator's answer json (`Job.res == None`) | the chain reads it for `vs_writes` (`main0_discard`); the build always has it |
| `Job.has_tf` without `Job.tf` | the capture entry point needs the layout; the build has it |

Steps that **fail like the Python** (`PostError::Failed`, whose `status()` is the string `finish()` returns): `check_msl` ("image atomic MSL failed"), `per_vertex`, `select_guards`, `gather_offsets`, `hmma_msl`, `shared_store_msl`,
`pool_arrays`, `uniform_tests`, `tg_spill` (each with its own status), and the exceptions `finish()` does not catch (a `sys.exit` or an uncaught ValueError / IndexError of `vs_depth`, `capture`, `vs_tf`,
`drop_point_size`, `reflect_rw`, ...: status "exception"; the Python's build stops there).

**Interface for the build driver** (what `finish/` uses): `post_rewrites(src, &Job) -> Result<PostOut, PostError>`; `Job` is `Default` (`Job { stage, local, has_tf, tf, dual_want, partner_locs, res, ..Default::default() }`);
`PostOut` carries `out` (the text `finish()` writes to `<key>.metal` / `_b.metal`), `analysis` (the source the reflection reads, when it is not `out`: a vertex shader's main0-only source, a pixel shader's source before its shared body),
`plain_len` (vertex: `out[..plain_len]` is the source without the layer entry points), `blend_zero`, `fs_uint`, `fs_dual`, `shared_body`, `vs_pad`, `entry_discard`, `layer_entry` / `layer_from` (`{'loc', 'comp'}`), `first_store`
(`[(binding's MSL name, word, value)]`), `tg_spill`. `tf::tf_layout(key, control section)` is `dmsh_build.tf_layout` (the job's `tf` value and the `[RED]` line it prints), `tf::vs_tf` the capture entry point.

## 2. Running it

```
export CARGO_TARGET_DIR=$SCRATCH_OLD/target-shader-repo
cargo test  --release -p ibuilder-shader                       # unit + oracle + step tests (tests skip with a message when a fixture is absent)
cargo test  --release -p ibuilder-shader --test oracle -- --ignored full_grizzy_chain     # every ps/vs/cs of the full Grizzy capture (~10 s, 650 MB)
cargo build --release -p ibuilder-shader                       # bins: shader_post, shader_tables, step_diff (+ F2's shader_finish)
# chain: Rust over the Python's inputs, compared with the Python's library image
shader_post --manifest iOSBuilder/out-py2/manifest --expect iOSBuilder/out-py2/capture-grizzy/refout/b-19d183a2b462.img     # the 53 jobs shader_post.py was proven on
shader_post --cap <capture dir> --expect <library image> --profile $SCRATCH_OLD/shader/profile/0100DFA027504000 [--threads N] [--repeat K] [--profile-steps] [--partners expected|rust]
# per-step differential test (section 3.2): the Python and the Rust half over one case list, then compare
tools/difftest/run.sh <corpus dir> [step]      # <corpus>/cases.tsv -> <corpus>/res/{py,rs}.tsv -> compare.py   (fuzz.py / mkreal.py / mkmut.py make corpora, mkfixtures.py the committed fixtures, dump.sh <corpus> <id> shows one case's two outputs, summary.py the table of 3.2)
# tables over a finished library
shader_tables <library image> <out dir> [--stamp ISO8601] [--threads N] [--repack OUT.img]     # writes c1_full.tsv, cb_extent.tsv (header stamp as given)
```
The capture (`<key>/{raw.metal, job.json (stage, local, tf), res.json}`) is written by `tools/measure/make_prof_engine.py` (an instrumented *copy* of `dmsh_build.py`, `DMSH_CAPTURE=dir`); `tools/measure/` also holds the
split / full-build / Python-chain benchmark drivers. Library API: `ibuilder_shader::{post_rewrites, Job, PostOut, Stage, PostError}`, `tf::{vs_tf, tf_layout}`, `tables::{c1_guard, cb_extent}`, `romfs::{Image, write_image_file}`.

## 3. Verification (what I ran, what came out)

Oracles are the **Python's own outputs on the same inputs**; nothing is compared with itself. The Python run for the differential tests is the engine's own module functions, imported from a COPY of the engine tree
in the scratch folder (`engine/dm/gfx/tools`, `PYTHONDONTWRITEBYTECODE=1`, `-I`, the reference venv, Python 3.14.1). **Version of the Python ported**: the engine sources of `iOSBuilder/engine/dm/gfx/tools` as of 2026-11-16 01:27
(`diff`-identical to the copy I ran): `dmsh_rewrite.py` 66,906 B (sha256 e60db42d...), `dmsh_wide.py` 7,614 B (86287413...), `dmsh_reflect.py` 40,907 B (a86a7dd0...), `dmsh_entry.py` 20,386 B (bbb098f5...),
`dmsh_atomic.py` 6,515 B (9b2aa6b4...), `dmsh_side.py` 11,573 B (2fad3633...), `dmsh_fetch.py` 22,629 B (b05106d0...), `dmsh_geometry.py` 16,714 B (5aaf9fee...), `dmsh_build.py` 99,679 B (e71ffba3...; `vs_tf`, `tf_layout`, `vs_pad`,
`finish` l.737-868), `dmsh_abi.py`/`dmsh_abi.h` (the ABI values enter through `templates.rs`). None of the worktree's `paches/engine/*.patch` touches these files. The iOSBuilder worktree's `gameapp/shader_device.py` (37,785 B, 2026-11-19 00:48) is the driver the capture ran through.

### 3.1 The full Grizzy chain run (the gate of the package)

| check | command / fixture | result |
|---|---|---|
| **chain on every ps / vs / cs of the full Grizzy build** | the instrumented Python build (`tools/measure/run_build.py --engine <instrumented copy> --keys all --jobs 4 --tables 0`, `DMSH_CAPTURE`, `nice`, 801 s wall) captured `raw.metal / job.json / res.json` of **all 21,990 shaders** (645.3 MB of MSL, 830 MB on disk); `shader_post --cap <that> --expect <full-gil-j6/b-b24b25240cad.img> --profile .../0100DFA027504000 --threads 6` | **21,845 of 21,845 byte-identical** (ps 10,541, vs 10,537, cs 767), **0 differ, 0 failed, 0 gated**; the other 145 are geometry shaders, `Unported("stage")` by design |
| same, expected image = the library that capture run itself built | `--expect <f1>/runs/cap-full/b-b24b25240cad.img` | identical, same counts (the two Python builds agree) |
| same, the vertex shaders' pad sets taken from the *Rust* pixel outputs | `--partners rust` | identical, same counts |
| same, in the iOS simulator | `xcrun simctl spawn <iPhone 17> .../aarch64-apple-ios-sim/release/shader_post --cap ... --threads 4` | identical, same counts |
| as a test | `cargo test --release -p ibuilder-shader --test oracle -- --ignored full_grizzy_chain` | passes (10 s) |
| chain, 53 jobs of `docs/SHADER-POST.md` / 2,459-shader sample | `tests/oracle.rs` | 53/53; 2,439 of 2,439 ps/vs/cs (round 1: 2,374, with 65 gated: `bit0_mask` 25, `pool_arrays` 20, `wide_loads` 20) |
| `c1_full.tsv`, `cb_extent.tsv`, image writer, interval arithmetic | unchanged from round 1 (`tests/oracle.rs`, `tests/units.rs`) | identical |

What Grizzy exercises of the previously gated steps, run alone over the 21,990 raw texts (`shader_post --profile-steps`): `pool_arrays` rewrites **122** shaders, `wide_loads` **160** (the same 160 the build's own timers counted), and none of
`per_vertex`, `bit0_mask`, `mul_extended`, `float_add_msl`, `gather_offsets`, `hmma_msl`, `shared_store_msl`, `frag_z_snap`, `first_store`, `tg_spill`, `wide_atomics`, `vs_base_instance`, `vs_tf` changes a Grizzy shader. For those the proof is 3.2.

### 3.2 Per-step differential tests (`tools/difftest/`)

`stepdiff.py` applies ONE function of the engine (`dmsh_rewrite.*`, `dmsh_fetch.select_guards`, `dmsh_entry.*`, `dmsh_wide.wide_loads`, `dmsh_atomic.{check_msl, wide_atomics}`, `dmsh_reflect.pool_arrays`, `dmsh_side.shared_body`, `dmsh_geometry.capture`,
`dmsh_build.{vs_tf, tf_layout}` (extracted by `ast`: the module runs a whole build at import), and `chain` = the sequence of `finish()` l.762-868 on those functions) to every case of a list, in worker processes, with a 1 s timer (the engine's `drop_point_size`
loops for ever on a text no real shader has: that counts as an error); `step_diff` (Rust, `difftest::run`) does the same with the Rust function. Both write one line per case: status (OK, or ERR for a ValueError / SystemExit / any other exception),
"text unchanged" flag, length and crc32 of the text, and a json of everything else the function returns (counts, the second text of `vs_depth`, `first_store`'s list, `tg_spill`'s bytes, the chain's flags and the crc32 of its analysis text, ...);
`compare.py` requires all of it to match, and reports whether the messages of two errors match too (informational: the build prints them, the library does not depend on them).

Inputs: (1) **all 2,459 captured real shaders**, every step of their stage plus a few of other stages, and the whole chain on each with its own job and answer json; (2) **mutated real shaders** (`mkmut.py`: two seeds of 1,900 mutated texts, 1-3 line-structure-preserving
mutations each: char / line / token damage, truncation, swapped / duplicated lines, digits changed) with all steps of their stage and the chain; (3) **synthetic documents** (`fuzz.py`): per step a generator written from the docstring and the regexes of the Python rule
(every alternative of every regex, the idiom's whitespace and indentation variants, near-misses, the guard / argument / parameter shapes that make the ValueError paths fire), composed with distractor lines, and 35 % of them mutated; (4) the chain
over those documents as a shader of a random stage with a real answer json; (5) random control sections for `tf_layout`. The smallest changed cases of each step (up to 3), one unchanged and one failing case, 71 in all, are committed as `tests/fixtures/` (expected value produced by the Python function) and replayed by `tests/steps.rs`;
`tests/steps.rs` also replays the big corpora against the Python's stored result files when they are on the machine.

**Result: 272,014 cases, identical in every field, 0 different** (87,298 of them change the text; in 26,039 the Python raised and so does the Rust). Per step:

| step | cases | identical | different | text changed by the step | Python raised |
|---|---|---|---|---|---|
| bit0_mask | 11,759 | 11,759 | 0 | 3,852 | 0 |
| blend_zero | 3,835 | 3,835 | 0 | 3,044 | 0 |
| border_emu | 6,259 | 6,259 | 0 | 3,214 | 4 |
| capture | 2,116 | 2,116 | 0 | 1,445 | 671 |
| chain | 30,239 | 30,239 | 0 | 13,755 | 6,367 |
| check_msl | 6,259 | 6,259 | 0 | 0 | 0 |
| drop_point_size | 7,608 | 7,608 | 0 | 6,332 | 207 |
| first_store | 7,182 | 7,182 | 0 | 1,595 | 0 |
| float_add_msl | 11,759 | 11,759 | 0 | 2,661 | 0 |
| frag_z_snap | 9,353 | 9,353 | 0 | 354 | 0 |
| fs_dual | 3,864 | 3,864 | 0 | 3,864 | 0 |
| fs_uint | 3,825 | 3,825 | 0 | 3,825 | 0 |
| gather_offsets | 11,759 | 11,759 | 0 | 896 | 4,304 |
| hmma_msl | 11,759 | 11,759 | 0 | 2,392 | 1,197 |
| max_threads | 1,695 | 1,695 | 0 | 73 | 0 |
| membar_fence | 6,259 | 6,259 | 0 | 40 | 0 |
| mul_extended | 11,759 | 11,759 | 0 | 2,956 | 0 |
| per_vertex | 9,330 | 9,330 | 0 | 1,680 | 2,167 |
| pool_arrays | 11,759 | 11,759 | 0 | 1,917 | 1,115 |
| precise | 6,259 | 6,259 | 0 | 4,975 | 0 |
| select_guards | 6,259 | 6,259 | 0 | 2,457 | 29 |
| shared_body | 6,259 | 6,259 | 0 | 6,259 | 0 |
| shared_store_msl | 11,759 | 11,759 | 0 | 1,040 | 4,280 |
| tf_layout | 4,000 | 4,000 | 0 | 2,569 | 1,431 |
| tg_fence | 6,259 | 6,259 | 0 | 40 | 0 |
| tg_spill | 7,186 | 7,186 | 0 | 780 | 1,131 |
| uniform_tests | 6,259 | 6,259 | 0 | 243 | 0 |
| vs_base_instance | 7,643 | 7,643 | 0 | 3,090 | 0 |
| vs_depth | 2,125 | 2,125 | 0 | 1,487 | 471 |
| vs_discard | 2,112 | 2,112 | 0 | 1,644 | 468 |
| vs_init | 2,167 | 2,167 | 0 | 1,415 | 0 |
| vs_layer | 2,112 | 2,112 | 0 | 0 | 0 |
| vs_layer_entry | 2,087 | 2,087 | 0 | 0 | 0 |
| vs_tf | 7,632 | 7,632 | 0 | 6,807 | 825 |
| wide_atomics | 11,759 | 11,759 | 0 | 347 | 1,372 |
| wide_loads | 11,759 | 11,759 | 0 | 250 | 0 |
| **all** | 272,014 | 272,014 | 0 | 87,298 | 26,039 |

Counts include the Python's failures on the "wrong stage" texts of corpus (1). The harness was checked for power by breaking the Rust on purpose (restored afterwards): `bit0_mask` with `operand_ok` forced to true: 1,793 of 4,000 cases differ; with the address-taken set `other` half dropped: 56 of 4,000; `hmma_bslots` with its top-level test loosened: 116 of 4,000,
with its "the last call keeps its B" test changed: 44 of 4,000. The same sources built for `aarch64-apple-ios-sim` and run in the booted iPhone 17 simulator give result files `cmp`-identical to the host's on all five corpora (272,014 lines).

Other checks that changed with this round: the `wide_atomics` / `first_store` / `hmma` fixtures only exist synthetically (no real shader has them); every other new step is also exercised by real shaders (`pool_arrays` 122 and `wide_loads` 160 rewrites in Grizzy; the 2,459 real shaders for all steps' no-change paths).

## 4. Numbers (indicative: a shared M1 Pro, load average 28-110 while I measured; best of N)

| | Python | Rust | factor |
|---|---|---|---|
| MSL chain, 2,439 shaders, one thread | `post_rewrites` alone, `process_time`: **8.18 ms a shader** (round 1 measurement; the full build's own timers: 6.97 ms) | **0.189 ms a shader** (5,295 shaders/s; `--threads 1 --repeat 6`; round 1 with the gates: 0.229 ms under heavier load) | **43x** |
| chain, the full Grizzy, 21,845 ps/vs/cs, one thread | 153 s in the full build's timers (inside the GIL) | **165 us a shader** (3.61 s a pass, 6,045 shaders/s; `--threads 1 --repeat 2`) | ~42x |
| chain, the full Grizzy, 8 threads | no gain (GIL) | **37.8 us a shader** (0.83 s a pass, 26,462 shaders/s) | |
| memory | | `shader_post` over all 21,990 shaders holds 645 MB of inputs and the outputs it compares: max RSS 3.5 GB, peak footprint 2.3 GB; the chain itself needs ~5 copies of one shader's MSL a thread | |

Cost of the steps ported this round, one thread, over the raw texts (`--profile-steps`, us a shader): `bit0_mask` 6.8-12 (the largest: a scan of every function body that holds ` ? (-1) : 0;`), `wide_loads` 2.5-3, `check_msl` 5.4 (the old `emulated` scan, now also `_rg32ui`),
`first_store` 3.5, `tg_spill` 2.4, `vs_base_instance` 2.7, `per_vertex`, `mul_extended`, `float_add_msl`, `gather_offsets`, `hmma_msl`, `shared_store_msl`, `wide_atomics`, `pool_arrays` 0.8-2.3 each (a substring test before any scan);
`frag_z_snap` 47-79 (the slowest, as before). The chain's time a shader did not rise with the ported steps (0.189 ms against 0.229 ms before, under different load).

## 5. What is NOT verified / known limits

* **Only Grizzy's MSL is real.** `per_vertex`, `mul_extended`, `float_add_msl`, `gather_offsets`, `hmma_msl`, `shared_store_msl`, `wide_atomics`, `first_store`, `tg_spill`, `vs_tf`, `bit0_mask`, `frag_z_snap`, `vs_base_instance` never fire on it (Switch 2 / Unreal idioms);
  their proof is the differential run on documents built from the Python's own docstrings and regexes. A spirv-cross output shape I did not think of (another game, another spirv-cross) is the real risk: run `shader_post` / `step_diff` over a new game's capture first.
* **Line structure.** The scanners assume what spirv-cross always writes, one declaration or statement per line; a mutation that splits a token or a member declaration over two lines (the Python's `\s+` would span them) is out of scope: the fuzz
  mutations never insert a newline into a line. `border::parse_member` (argument buffer members) is line-based.
* Python `\w`, `\s`, `\d`, `strip()`, `isdigit()` are Unicode-aware, the scanners are ASCII (spirv-cross writes ASCII). Integers are `i64` / `u64` / `i128` where Python's are unbounded (a local size of 2^63 or a 40-digit slot number is not reproduced).
* `vs::reflect_rw` is the part of `dmsh_reflect.reflect` the chain needs (the `rw` flag, the member-count ValueError, the bounded-storage-buffer ValueErrors); the exceptions the full reflection raises for names it cannot decode (a TypeError from `decode_tex`) are F2's
  (`finish/reflect.rs`), and F2's `finish` cross-checks the discard decision with the real bindings.
* The Python's `drop_point_size` never terminates on a point size used outside a vertex function; the Rust gives up after 4,096 rounds with a `Failed`. Where the Python raises something `finish()` does not catch (IndexError, a bare `sys.exit`), the Rust returns `Failed` with status "exception".
* `tf::tf_layout` is ported and differentially tested but nothing calls it yet: it belongs to `job_for` (package B), which reads the control section.
* The shader driver of the phone (`gameapp/shader_device.py`, 37,785 B, 2026-11-19 00:48) is the one the capture ran through; the Python of the phone side is still changing in that worktree.
* The shared workspace did not always load while other contributors' crates were half written; I built and tested in a private workspace copy (`ws.sh` equivalent, excluding F2's `finish`) and then in the shared one, where `cargo test --release -p ibuilder-shader` passes (all tests of both packages).

## 6. iOS portability

`#![forbid(unsafe_code)]` in the library; no `fork`/`exec`/`Command`, no `std::os::unix` use, only `rayon`, `memchr`, `serde_json` (+ `memmap2` for the Mac CLIs; F2 added `regex`, `sha2`). **Builds** with
`PATH=/opt/homebrew/opt/rustup/bin:$PATH cargo build --release --target aarch64-apple-ios -p ibuilder-shader` and `--target aarch64-apple-ios-sim` (both finish, no warnings from this crate). The simulator binaries ran in the booted iPhone 17
(`xcrun simctl spawn <SIMULATOR-UDID> <bin> ...`, absolute paths): `step_diff` over the five corpora (272,014 cases) writes files `cmp`-identical to the host's, `shader_post` over the full Grizzy capture gives 21,845 identical, 0 differ.
Simulator timings are the Mac's CPU, not the phone's.

## 7. Dependencies (beyond the allowed list)

* `memchr` (SIMD substring search; the scanners' only primitive; already in the lock through `serde_json`, pure Rust). `serde_json` (answer json, reflection input, test fixtures) is on the allowed list.
* This round added **no dependency**: the Python regexes (look-behind, back-references, lazy quantifiers) are cursors (`text::Cur`) and scanners; `regex` / `fancy-regex` were not used by the chain (F2 added `regex` to the crate for its own files).

## 8. Review r3 items of this crate (read again against the code, 2026-11-21)

All in place since the round before, each with its guard (review r4 4.4 confirmed 11 of 12; the 12th, the split's Unity decoder, is `docs/shaders/SHADERSPLIT-RUST.md` section 11):
4.8 `finish/tess.rs` `body_end` (a domain entry that ends the text is an empty body, then `Err("tessellation: domain without return")`, unit test `a_domain_body_ends_at_its_brace_or_at_the_last_character`);
4.13 `tables.rs` `row_end` (`16 * (high + 1)` checked: an overflow is "not proven", in `unproven_in` and in `cb_extent`'s row) and `finish/reflect.rs` `row_offset` (saturating, clamped to the json's `i64`);
5.10 `bin/step_diff.rs` (`--dump` without a folder is a usage error); 5.11 `bin/shader_finish.rs` (the dead `filter(|_| false)` is gone, the json diff is reported); 5.12 `side.rs` / `guards.rs` (cuts by characters,
not bytes); 5.24 `finish/reflect.rs` `hex` (a handle past `i64` keeps its type and offsets, unit test); 5.25 `finish/mod.rs` (`tg_spill` through `i64::try_from`), the dead `name_len` of `steps.rs` gone, the `shader_finish`
bin declared in `Cargo.toml`. The run-time regexes of `finish/` (review r4 6.3): `docs/shaders/SHADER-FINISH-RUST.md` section 6.
