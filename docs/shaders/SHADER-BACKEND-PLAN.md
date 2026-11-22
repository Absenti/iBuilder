# Plan: the Rust stage-3 backend (shaders) and the delta since the first port (2026-11-19)

Read `docs/CONVENTIONS.md` first (hard rules: read-only repos, no secrets, no game data in the repo, truthful reports, iOS constraints).
Then `docs/shaders/SHADER-STAGE-ANALYSIS.md` (call tree + measurements of stage 3) and `docs/shaders/SHADER-RUST.md` (what `ibuilder-shader` already does and its gates).

## Scratch folders

* **Your writes** go under the scratch folder: `$SCRATCH`
  (call it `$S2`). Use your own `CARGO_TARGET_DIR=$S2/target-<your-name>` and your own sub-folder `$S2/<your-name>/`.
* **Reference material from the first part of the work** (READ-ONLY, treat as fixtures; never write there):
  `OLD=$SCRATCH_OLD`
  - `$OLD/ref/venv/bin/python` (Python 3.14 + capstone + lz4 + zbic), `$OLD/ref/engine/dm/{tools,games}` (writable copies of the engine's Python)
  - `$OLD/shader/engine/dm/...` (the engine's `gfx/tools` Python **as copied for the shader harness**, plus `ios/tools`); run Python from a COPY of it in `$S2`, with
    `PYTHONDONTWRITEBYTECODE=1`; `$OLD/shader/engine-cap` and `engine-prof` are instrumented copies (capture of `raw.metal/job.json/res.json`, timers)
  - `$OLD/shader/bin/{shader_device.py,run_build.py,run_split.py,lzma_py.py,...}` = the phone's driver + the Mac harness (see `crates/ibuilder-shader/tools/measure/`)
  - `$OLD/shader/split-out/{pkg,tables}` = the Python split of Grizzy (oracle for the split); `$OLD/shader/profile/0100DFA027504000` = Grizzy's provisional profile
  - `$OLD/shader/cap-s300`, `cap-s1800` = captured `<key>/{raw.metal,job.json,res.json}` (291 / 2,459 shaders); `keys-s300.txt`, `keys-s1800.txt`
  - `$OLD/shader/runs/full-gil-j6/b-b24b25240cad.img` = the complete Python library of Grizzy (21,990 shaders, 56,885 files, 1.7 GB, `DMROMFS1`);
    `runs/s300-j1/`, `runs/s1800-cap/`, `runs/smoke/` = Python builds of key subsets (with their `*.log`, `harness.json`)
  - native tools (Mac): the engine's `dmsh_xlate_bin/dmsh_xlate` and `spirv_cross_bin/spirv-cross` under `iOSBuilder/engine/dm/gfx/tools/` (run them, never write there)
  - Grizzy inputs: `iOSBuilder/out/ref-img-grizzy/RomFS.img`, `iOSBuilder/out/ref-out-grizzy/ExeFS/main`
* The Python of the phone side (newest) is in the worktree `$IOSBUILDER_WORKTREE/gameapp/` (`shader_device.py`, `gameapp_pipeline.py`, `mods.py`, ...).
  **Another session is still changing that worktree; re-read a file before relying on it and say in your report which version (size, mtime) you ported.**

## What changed since the first port (why this plan exists)

The worktree gained, after our pipeline agent read it: `gameapp/mods.py` (+ `test_mods.py`, IPS/pchtxt/cheat mods applied before stage 2), a rewrite cache
(`rewrite_cache_dir`), `shader_parallelism`, `release_mimalloc`, `learned_*` (shader learning), `apply_mods` hooks in `gameapp_pipeline.py` (2,815 -> 2,910 lines),
`tools/pyrewrite_driver.py` (SystemExit becomes an error), `paches/engine/0301-rewrite-threads.patch` (threads for `patch_nso`), new engine patches
(0009 PSO archive, 0013 controllers remap, framegen, ...). Shader driver `gameapp/shader_device.py` is unchanged since our copy (`diff` empty).

## Work packages (one agent each; edit only what is yours)

| id | name | owns | main Python | oracle |
|---|---|---|---|---|
| M | mods + pipeline delta | new crate `ibuilder-mods`; `crates/ibuilder-pipeline/**` | `gameapp/mods.py`, `test_mods.py`, the new parts of `gameapp_pipeline.py`, `pyrewrite_driver.py` | `test_mods.py` cases + differential tests against `mods.py` on synthetic inputs |
| F1 | chain: remove the gates | existing chain files of `crates/ibuilder-shader/src/` (`steps.rs`, `guards.rs`, `wide.rs`, `zsnap.rs`, `post.rs`, ...) | `dmsh_rewrite.py`, `dmsh_wide.py`, `dmsh_reflect.pool_arrays`, `dmsh_entry.py`, `dmsh_build.vs_tf`/`tf_layout` | full Grizzy capture vs Python, every ps/vs/cs |
| F2 | per-shader `finish()` | NEW dir `crates/ibuilder-shader/src/finish/` (+ its `pub mod finish;` line in `lib.rs`) | `dmsh_build.py` l.245-360 (`write_refl`, `pull_finish`, `vin_finish`, `ps_libraries`), 578-736 (`gs_slice`, `spv_single_lane`, `geometry_finish`), 737-875 (`finish`); `dmsh_reflect.py`, `dmsh_side.py`, `dmsh_alpha.py`, `dmsh_logic.py`, `dmsh_fetch.py` (fold, screen_varyings), `dmsh_vin.py`, `dmsh_pull.py`, `dmsh_geometry.py`, `dmsh_atomic.py` | library image of the Python build, file by file |
| B | build driver + backend | NEW crate `crates/ibuilder-shaderbuild`; adapter wiring in `crates/ibuilder-cli` and `crates/ibuilder-ffi` | `dmsh_build.py` l.1-244 (keys, cache, outputs, publish), 365-460 (`vertex_ab`, `early_fragment_tests`, `ps_omap`, `job_for`), 876-1157 (batches, `translate`, `fin`, summary); `shader_device.py` (`build`, `live_package`, `live_translate`, `JobMemo`); `dmsh_mtlcache.py` (source mode) | captured `job.json`, library image, `shader_lib` format |
| S | split | NEW crate `crates/ibuilder-shadersplit` | `engine/dm/gfx/tools/shader_split.py` (3,821 lines) + `shader_device.split` | `$OLD/shader/split-out/{pkg,tables}` byte for byte |

Interfaces between packages (nobody waits for anybody: each side tests against a mock or the Python's stored files):

* **Tools** (`crates/ibuilder-shader/src/tools.rs`, already written): `trait ShaderTools { xlate(&[String]) -> ToolRun; spirv_cross(&[String]) -> ToolRun }`, argv without the
  program name, exactly as `shader_device._FakeSubprocess._tool` passes them. F2 uses it for its spirv-cross / re-translation calls; B implements it (process-backed for the Mac behind
  feature `host-tools`; callback-backed for the phone) and drives it.
* **Finish** (F2 -> B): in `ibuilder_shader::finish`
  ```rust
  pub struct Options { /* pulling, ios targets, ... : what dmsh_build reads from its environment */ }
  pub struct Ctx { .. }   // loaded ONCE per build from the profile folder + tables: GAME rows, shader_map (loc), facts, pairs, bf_want, dual_want, chain id, Options
  impl Ctx { pub fn load(profile_dir: &Path, game_id: &str, chain: &str, opts: Options) -> Result<Ctx, String> }
  pub struct Finished { pub status: String /* "ok" or the Python's failure kind: "MSL failed", "translation failed", ... */, pub detail: String,
                        pub refl: Option<serde_json::Value>, pub outputs: Vec<(String, Vec<u8>)> /* library file name -> bytes, e.g. <key>_b.metal, <key>_b.json, <key>_bx.metal */ }
  /// `finish(key, w, j, c1)` of dmsh_build.py: `work` holds <key>.spv, <key>.res.json (and <key>.bin); `job` is job_for's dict (mutated like the Python does); `c1` its c1 bytes.
  pub fn finish(ctx: &Ctx, key: &str, work: &Path, job: &mut serde_json::Value, c1: &[u8], tools: &dyn ShaderTools, paired_ps: &dyn Fn(&str) -> Option<String>) -> Finished
  ```
  F2 may refine the exact signature (document it in `docs/shaders/SHADER-FINISH-RUST.md` within the first hour, in a section "API for the build driver"); B codes against this shape with a `Finisher`
  trait of its own and a mock, and the two are joined at integration. Shaders F2 cannot finish yet return `status = "unported: <why>"` (never a wrong output).
* **Chain** (F1): `ibuilder_shader::post_rewrites(job, msl) -> Result<PostOut, PostError>` keeps its signature; F1 only removes `PostError::Unported` cases (and keeps the gate for what it still
  cannot port, honestly). F2 calls it.
* **Pipeline** (B -> M): B implements `ibuilder_pipeline::adapters::ShaderBackend` (and the `ShaderTools` plumbing); M owns the pipeline crate and does not change that trait without telling B in
  its report (a change request goes in the report, not a silent edit).

## Rules that are special for this round

1. **Byte-identity to the Python is still the oracle**; where the Python's own output depends on something Rust cannot reproduce (the cache key hashes the Python's source text, `dmsh_build.CODE_FP`),
   define the Rust key explicitly (document it) and prove the *outputs* identical, not the key.
2. **No silent partial ports.** A shader/stage/option the Rust code does not handle returns an explicit "unported: ..." result so the pipeline can fall back or say so. Never emit approximate output.
3. Core crates: no `fork`/`exec`/`Command`; the process-backed tools and any test that needs the Mac binaries live behind a feature / in tests / in `src/bin`. `#![forbid(unsafe_code)]` unless justified.
4. New dependencies are allowed if justified in your doc and available (crates.io is reachable; the offline cache is not the limit): candidates `zstd` or `ruzstd`, `rusqlite` (bundled), `regex`.
   Prefer pure Rust that builds for `aarch64-apple-ios`; verify with `PATH=/opt/homebrew/opt/rustup/bin:$PATH cargo build --release --target aarch64-apple-ios-sim -p <crate>`.
5. Shared workspace files (`Cargo.toml` workspace deps, `ibuilder-shader/src/lib.rs`): append only, with the Edit tool (never rewrite the whole file), so parallel agents do not erase each other.
   If the workspace fails to load because of another agent's half-scaffolded crate, build in a private copy of the workspace (see `crates/ibuilder-shader/tools/measure/ws.sh`) and retry later.
6. Heavy runs: the Mac is shared by 5 agents. Never run two heavy Python jobs at once yourself; prefer small key subsets (keys-s300 / s1800); one full-Grizzy run only when your report needs it.
7. Do not touch `$OLD`; do not touch other packages' files; do not use `git`.
8. Final report (≤ 300 words) exactly as in `CONVENTIONS.md`, plus: the version of the Python source you ported, what is gated/unported, and a doc `docs/<your topic>.md`.
