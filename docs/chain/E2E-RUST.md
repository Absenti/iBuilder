# The whole chain on Grizzy through the Mac CLI: profile -> rewrite -> shaders -> assemble -> sign (package I, 2026-11-19)

Before this round nobody had run stage 3 through the pipeline and the CLI on real data: `ibuilder` had no option for it, and `crates/ibuilder-cli/tests/shaders_pipeline.rs` covers only a 401-shader sub-game
through the backend's `BuildRequest`. Now `ibuilder --shaders --shader-chain b24b25240cad --engine <engine>` runs **all five stages on Grizzy and the Lemmings (`0100DFA027504000`)** in one process, through the pipeline's own stage 3
(`ibuilder_pipeline::shaders`: `ShaderResume` work folder, the split's job memo, the memory/thermal gate, the shared cache folder, the pack install), with `ibuilder-shadersplit` and `ibuilder-shaderbuild` as the backend and the engine's
native `dmsh_xlate` and `spirv-cross` as child processes (`shader-host-tools`, Mac only). Code and design: `docs/chain/PIPELINE-RUST.md` section 9. Everything below was run; the machine is an M1 Pro (8 cores, 16 GB),
**shared with other agents** (load averages are given per run: they are indicative, not a benchmark).

## 1. What runs, and the commands

```
cargo build --release -p ibuilder-cli --features shader-host-tools        # target/release/ibuilder   (default features + the engine's tools as child processes)
cargo build --release -p ibuilder-shaderbuild --features host-tools,f2-finish   # target/release/libdiff (the file-by-file image comparer)
ibuilder --exefs $IOS/out/ref-out-grizzy/ExeFS --out <out> --bundle $IOS/engine --host $IOS/out-load/host-prebuilt-loadable-ios.dylib \
         --romfs $IOS/out/ref-img-grizzy/RomFS.img --shaders --shader-chain b24b25240cad --engine $IOS/engine [--shader-jobs N] [--set KEY=VALUE] [--report f.tsv]
scripts/e2e-grizzy.sh run NAME | kill NAME SECONDS | verify NAME        # the same, under /usr/bin/time -l, plus the checks below (env vars at the top of the script)
```
(`$IOS` = `$IOSBUILDER`, read only: the ExeFS `main`/`sdk`/`subsdk0`, the 2.5 GB `RomFS.img`, the host dylib, the engine folder whose two tool binaries are run and whose `dm/games` and `dm/host` are read as the
profile bundle, the Python oracle `out-py/pyrewrite/out/<module>/`. The Python's library image used as the shader oracle is `$OLD/shader/runs/full-gil-j6/b-b24b25240cad.img`, the one every earlier package compared with. No key, certificate
or provisioning profile is read; the signature is ad hoc. The runs used the copy `$SCRATCH_OLD/ref/engine` as `--bundle` first and the engine itself later: the derived profile tables are `cmp`-identical either way
(`shader_map`, `shader_facts`, `shader_pairs`, `bf_want`, `pso_manifest`, `.conflicts`; `game.tsv` differs in one comment line that names the extract folder).)

The options: `--shaders` (run stage 3), `--shader-chain ID` (config `shader_chain`: the library is `b-<ID>.img`), `--engine DIR` (`dm/gfx/tools/{dmsh_xlate_bin/dmsh_xlate, spirv_cross_bin/spirv-cross}`), `--shader-jobs N` (`shader_jobs`),
`--set KEY=VALUE` (any config row, last). Without `--shaders` nothing changes (`shaders=none`, the old e2e test still passes). A bad combination is exit 2 with a message before anything is written (tests below). `ibuilder --help` lists them.
The feature `shader-host-tools` is **off by default**: the default build, `ibuilder-ffi --features real-stages` and `ibuilder-pipeline` still build for `aarch64-apple-ios-sim` and `aarch64-apple-ios` (section 5).

What stage 3 did on this data (from `pipeline.stages.log` of the cold run): the split read the RomFS and `main` in 0.9 s (log line `shaders: 21990 (compute 767, geometry 145, pixel 10541, vertex 10537)`), the build finished them on 6 threads
(every shader through the translator and SPIRV-Cross child processes), wrote the 1,706,277,556-byte library of 56,885 files, the tables `c1_full.tsv` (6,679 shaders not proven bounded) and `cb_extent.tsv` (49,309 slots, 1,578 read-only storage slots, 21,522 shaders), and the pipeline
installed the pack `Documents/shaders/0100DFA027504000/` (`b-b24b25240cad.img`, the tables beside it, `pack.txt`: `shaders 21990`, split `rsc7a267c668`).

## 2. Results

`/usr/bin/time -l` totals and the CLI's step table (wall seconds; peak = largest resident size sampled every 5 ms **during the step**, MiB). Load = 1-minute load average at the start of the run -> at its end (the run raises it itself:
6 translator threads + their children).

| run | what | wall | user + sys CPU (children included) | max RSS / peak footprint | load |
|---|---|---|---|---|---|
| cold 1 | empty output folder, no cache | **92.04 s** | 309.7 + 137.7 s | 964 MiB / (not kept) | 7.2 -> 18.2 |
| cold 2 | the same, a second empty folder | **96.99 s** | 308.8 + 138.3 s | 943 MiB / 823 MiB | 6.4 -> 15.4 |
| cold 3 | the Rust test `grizzy_end_to_end_with_shaders` (the library entry point), `--bundle` = the engine | 97.69 s | (test process) | 703 MiB sampled | 5.4 |
| killed 1 | the cold command, SIGTERM at 30 s | 30 s | | | |
| killed 2 | the same command again (no `--shader-jobs`), SIGTERM at 15 s | 15 s | | | |
| **resumed** | after the two kills, `--shader-jobs 6` | **49.68 s** | 179.4 + 98.0 s | 166 MiB / 163 MiB | 6.6 -> 14.1 |
| **warm 1** | the finished folder again: everything cached and installed | **0.35 s** (0.22 s with the repo script) | 0.14 + 0.13 s | 26 MiB / 24 MiB | 11.7 |
| **warm 2** | the library removed, the cache kept: rebuilt from 21,990 cache hits | **14.82 s** | 1.6 + 21.5 s | 677 MiB / 675 MiB | 10.9 -> 9.0 |

Step table (wall s / peak resident MiB):

| step | cold 1 | cold 2 | resumed | warm 2 | warm 1 |
|---|---|---|---|---|---|
| 1 deriving the profile | 0.33 / 262 | 0.35 / 262 | 0.00 / 3 | 0.00 / 3 | 0.01 / 3 |
| 2 rewrite main (75 MB; cached in the last three) | 4.81 / 513 | 4.73 / 494 | 0.04 / 7 | 0.04 / 7 | 0.05 / 7 |
| 2 rewrite subsdk0 / sdk | 0.60 / 517, 1.30 / 522 | 0.60 / 495, 1.31 / 501 | 0.00 / 7, 0.02 / 7 | 0.00 / 7, 0.02 / 7 | 0.01 / 7, 0.03 / 10 |
| **3 shaders split** | 0.89 / 964 | 0.87 / 943 | 0.41 / 9 (not run: the earlier run's split is reused) | 0.90 / 676 | (not run) |
| **3 shaders build** | 83.94 / 577 | 88.93 / 560 | 49.02 / 165 | 13.56 / 371 | "already built" 0.00 / 10 |
| 4 assemble (192.7 MB, 42 load commands) / sign (ad hoc) | 0.10 / 576, 0.05 / 576 | 0.13 / 412, 0.06 / 362 | 0.11 / 148, 0.06 / 152 | 0.22 / 371, 0.06 / 375 | 0.15 / 11, 0.06 / 26 |
| total | 92.03 s | 96.97 s | 49.67 s | 14.81 s | 0.31 s |

Reading the memory column: the **resident** size of the process does not fall between steps (the pipeline's own line `[mem:before-shaders] release: the allocator handed back 0 MiB`), so the split shows 943-964 MiB in a cold run (about 440 MiB
of its own on top of the ~520 MiB the rewrite left resident; the split alone is 554-637 MiB, `docs/shaders/SHADERSPLIT-RUST.md`) and the build 560-577 MiB, while the build with nothing before it (resumed) peaks at 165 MiB. The kernel's
`phys_footprint` during the cold build is 159 -> 216 MiB (the pipeline's `[mem:shaders n/N]` lines). The translator and SPIRV-Cross children are separate processes and are not in these numbers (`time -l` reports the largest one, not the sum).
Where the time goes in a cold run: 84-89 s of stage 3 is the native tools (the 6 threads wait for their child processes; the CPU total, 447 s, is mostly theirs: the Rust side alone is what warm 2 shows, 1.6 s of user CPU for the same 21,990 shaders
from the cache); the Rust side's own work is the split (0.9 s), the cache check (21,990 digests) and the image write. For comparison, from the earlier packages' docs (not re-measured here): the Python's translation of the same shaders took 733 s wall
(`docs/shaders/SHADERBUILD-RUST.md` section 5), its split 19 s (`docs/shaders/SHADERSPLIT-RUST.md` section 4).

## 3. The checks you asked for

### (1) Exit code 0, every stage ok

```
result: ok (rc 0), stages: assemble=ok, profile=ok, rewrite=ok, shaders=ok          exit=0          (cold 1, cold 2, resumed, warm 1, warm 2: the same line and exit)
```
The Rust test (`cargo test --release -p ibuilder-cli --features shader-host-tools --test grizzy_e2e -- --ignored --nocapture grizzy_end_to_end_with_shaders`, 100.35 s) asserts the four statuses, then the items below except the `libdiff` one.

### (2) The rewrite outputs equal the Python oracle: 21 of 21

`scripts/e2e-grizzy.sh verify cold` (and `verify resume`): `cmp` of `extract/rewrite/<m>/<m>.{patched.bin,fixups,sites,gates,cuts,obs,changed}.tsv` for `main`, `sdk`, `subsdk0` against `out-py/pyrewrite/out/<m>/`:
`(2) rewrite files identical to the Python oracle: 21 of 21, different: 0` (cold 1, resumed). The Rust test makes the same 21 comparisons with `ibuilder_testkit::compare_files`.

### (3) The library `b-b24b25240cad.img` against `$OLD/shader/runs/full-gil-j6/b-b24b25240cad.img`

```
libdiff <out>/shaders/0100DFA027504000/b-b24b25240cad.img $OLD/shader/runs/full-gil-j6/b-b24b25240cad.img --ignore-first-line c1_full.tsv --ignore-first-line cb_extent.tsv
differs: c1_full.tsv
differs: cb_extent.tsv
differs: inputs.tsv (140 vs 140 bytes)
files A 56885 B 56885: identical 56882, identical but for the time stamp line 0, different 3, only in A 0, only in B 0
```
(cold 1, resumed and the library rebuilt from the cache in warm 2 give this same result.) **56,882 of 56,885 files are byte-identical, the same 56,885 names; the 21,990 shaders' 56,882 files are all in the identical set.** The three that differ are the library's
three stamped files, and only on the stamp:

| file | `diff` of the file extracted from each image (`python3 -I imgx.py`, a 15-line DMROMFS1 reader in the scratch folder) | meaning |
|---|---|---|
| `c1_full.tsv` (113,612 B), `cb_extent.tsv` (646,025 B) | **one line each: line 1** `# chain b-b24b25240cad inputs 8ea0535d1d1c 2026-11-19T02:06:26+01:00` -> `... inputs 21d6dabb3032 2026-11-19T10:25:47+00:00` | the digest and the time stamp (UTC now; the Python wrote the local offset, `docs/shaders/SHADERBUILD-RUST.md` section 6). Every other line is equal |
| `inputs.tsv` (140 B) | **one line: line 3** `inputs 8ea0535d1d1c` -> `inputs 21d6dabb3032` | the digest |

**Why the `inputs` digest differs, precisely.** It is `dmsh_chain.inputs`: sha256 over the bytes of eight profile tables (`shader_facts`, `shader_map`, `shader_map.tsv.conflicts`, `shader_pairs`, `pso_pairs`, `bf_want`, `geo_mesh`, `dual_want`; a
missing one counts as `missing`) and the profile's `nvn_*` / `shader_*` rows. The Python image was built from `$OLD/shader/profile/0100DFA027504000` (the stored provisional profile); the pipeline builds from the profile **it derives
in-process** (`Documents/profile-derived/<id>/`, `ibuilder-profile`) plus the four tables its own split writes. Comparing the pack's tables (`<out>/shaders/<id>/`, the exact files the build read) with the stored profile:
`shader_facts`, `.conflicts`, `shader_pairs`, `pso_pairs`, `bf_want`, `pso_manifest` are `cmp`-identical, `geo_mesh` / `dual_want` absent in both, the `nvn_*` / `shader_*` rows of `game.tsv` are the same three rows
(`shader_versions v0`, `shader_groups /Data/=data`, `shader_note ...`); **one table differs: `shader_map.tsv`, in its line 2 only (44 bytes)**: the stored map says `# column 5 records where each key first appears; no PC shader pairing supplied`
(written by an older `shader_split.py`), the pipeline's (our split = the current engine split, `docs/shaders/SHADERSPLIT-RUST.md` section 3) says `# column 5 records where each key first appears, a copy in the game before one in an add-on; no PC shader pairing supplied`.
Proof by recomputation (`python3 -I inputs_digest.py`, the algorithm of `ibuilder-shaderbuild::game::inputs_digest`): over the stored profile it gives `8ea0535d1d1c` (the Python image's stamp), over the pipeline's tables
`21d6dabb3032` (ours); the stored profile with **only** `shader_map.tsv` taken from the pipeline gives `21d6dabb3032`, the pipeline's tables with only the stored `shader_map.tsv` give `8ea0535d1d1c`. So the digest differs because of that comment line and for no other reason; no shader
file depends on it. (`game.tsv` also differs between the two profiles, in its comment lines and in the stored profile's `depth_mode zero` row, which the derived profile lacks: not an input of the library, and not a stage-3 matter, but a real
difference between the two profiles.) Two cold runs of the pipeline agree with each other as well: `libdiff cold1 cold2` -> `identical 56883, identical but for the time stamp line 2, different 0` (cold 2's `cmp` with cold 1 differs at the first time stamp, 1,281,962,347 bytes in).

### (4) The signed framework and `codesign --verify --strict`

```
codesign --verify --strict --verbose=2 <copy of DmGameProbe.framework/DmGameProbe>
DmGameProbe: valid on disk
DmGameProbe: satisfies its Designated Requirement                                       (exit 0)
```
**The Mach-O passes; the bundle does not**, and that is not new: `codesign --verify --strict` on `DmGameProbe.framework` (or on the file in place inside the folder) says `code has no resources but signature indicates they must be present` (exit 1), `codesign -dv`:
`Info.plist=not bound`, `Sealed Resources=none`. `ibuilder-sign` signs the Mach-O only and writes no `_CodeSignature/CodeResources` (`docs/chain/PIPELINE-RUST.md` section 5; on the phone zsign seals the bundle). The signature is also checked in
process (`ibuilder_sign::verify_file`: every page of the 194,209,400-byte file, asserted by the Rust test). Not fixed here (it is `ibuilder-sign`'s).

### (5) Resume after a kill in stage 3

Output of the first kill (a scratch script `kill-run.sh kill1 resume 30`; `scripts/e2e-grizzy.sh kill` is the same commands: its first version killed a subshell and left the CLI running, found by `ps` after a smoke run, fixed with `exec`, and run again: it kills the CLI):

```
before: cache entries 0
stage file at the kill: stage 3/5: shaders: translating for this iPhone (5793/21990 shaders)
killed after 30 s: ibuilder pid 66928 exit status 143 (143 = SIGTERM)
after the kill: cache entries 5907, stray tmp files 0
0100DFA027504000.work:  out  pkg  profile  resume.json  split.ok  splitjobs  tables          split jobs kept: 44
```
Every finished shader is one immutable file renamed into place, so a kill leaves whole entries only (5,907 shaders finished; 0 stray temporary files after this kill and after the second one), the split's 44 job files, `split.ok`, and `resume.json` (the signature of the inputs). A kill that lands while an entry is being written leaves one `.tmp-<pid>-<thread>` file instead: the smoke run of the repo script (25 s, not resumed) had 4,200 entries and 1 such file; the build never reads it and its sweep removes it after an hour (`cache.rs`, `driver.rs` tests). The translator children of a killed CLI finish the call they were in (a few hundred ms) and then exit.
The same command again, **without** `--shader-jobs`, killed at 15 s: the log says `stage 3 shaders: [ORANGE] the previous shader build of 0100DFA027504000 never finished (its work folder was still there): this one reads and translates on 1 thread`,
`the earlier run's work is kept: the same game, DLC and chain, so what it finished is not done again`, `the split of the earlier run is complete (21990 shaders): not read again`, and the cache check found the 5,907 entries: 859 more were
finished on its one thread in the 15 s it ran (6,766 entries). That 1-thread rule is the Python's (an iPhone that was killed for memory must not run wide again), it is why the last run passes `--shader-jobs 6` (the config row wins). The final run:

```
stage 3 shaders: the earlier run's work is kept: the same game, DLC and chain, so what it finished is not done again
stage 3 shaders: the split of the earlier run is complete (21990 shaders): not read again
shader cache: .../resume/shadercache (0.70 GB, limit 2.00 GB): shaders already translated for any game are restored
shaders 21990: ok 21990
shader cache: hit 6766, rebuilt 15224            (6,766 + 15,224 = 21,990: the 6,766 finished shaders of the two killed runs were not translated again)
stage 3 shaders: Documents/shaders/0100DFA027504000 built on this device in 49 s
result: ok (rc 0) ... exit=0                     49.68 s wall (cold: 92-97 s); split 0.41 s (not read again); max RSS 166 MiB
```
`verify resume`: rewrite 21 of 21 identical; library against the Python's: the same `56,882 identical, 3 differ` as (3) (the three stamped files); and **against the cold run's library: `libdiff resume cold` -> `identical 56883, identical but for the time stamp line 2, different 0` (exit 0)**: the same library, interrupted twice.

### (6) A second complete run with everything cached

* **warm 1** (the finished folder, run again): **0.35 s**, 26 MiB. The profile derives in 0.0 s and the three rewrites come from their cache (`main: rewrite taken from the cache`), stage 3 says `Documents/shaders/0100DFA027504000 already holds a library of chain b24b25240cad -- nothing to build`
  (step `3 shaders already built`), the framework is assembled and signed again (0.2 s).
* **warm 2** (the stronger form: the installed pack removed, the 1.59 GB cache kept): **14.82 s**: the split runs again (0.9 s), `shader cache: hit 21990, rebuilt 0`, the library is composed from the cache entries (hard links into the build's stage
  folder) and written (the 1.7 GB image write and 22,000 cache-entry opens and links: 13.6 s, 1.6 s user + 21.5 s system CPU, 371 MiB sampled). Its library: `libdiff warm2 run1` -> `identical 56883, ... time stamp line 2, different 0`; against the Python's: the same 3 stamped files.

## 3b. Memory of the same run (round 3, `docs/chain/MEMORY-RUST.md`)

The step table and `--report` now carry the footprint (`peak footprint`, `left: RSS`, `left: footpr.` per step; the TSV has `peak_footprint_bytes`, `resident_at_end_bytes`, `footprint_at_end_bytes` after the old columns), and the
`ibuilder` binary runs with `ibuilder-alloc`'s allocator. The cold run of `scripts/e2e-grizzy.sh` is the same command and the same checks (21 of 21 rewrite files, 56,882 of 56,885 library files, `codesign`); its peak
footprint is 345 MiB, down from 852 MiB (and RSS 466 MiB from 864 MiB); the per-stage table is in `docs/chain/MEMORY-RUST.md` section 1. The checkout's `out/ref-img-grizzy/RomFS.img` and `out-py/` were deleted during round 3: the script's `IOSBUILDER` points at a
folder with the same layout (`out/ref-out-grizzy/`, `out/ref-img-grizzy/RomFS.img`, `out-load/host-prebuilt-loadable-ios.dylib`, `out-py/pyrewrite/out/<m>/`) when the checkout lacks them, and `ENGINE` at the engine folder.

## 4. Tests added or changed (`cargo test --release`, 2026-11-19, shared Mac)

| suite | result |
|---|---|
| `-p ibuilder-pipeline -p ibuilder-ffi -p ibuilder-mods` | pipeline unit 57, `boot_spec` 1, `chain_spec` 9, `delta_spec` 10, **`pipeline_spec` 53** (52 + 1 new), `resume_spec` 16; ffi unit 6, `c_abi` 1 (three C programs); mods 5 + 7: all pass |
| `-p ibuilder-cli` (default features) and `-p ibuilder-cli --features shader-host-tools` | unit 1, **`cli.rs` 8** (3 old + 5 new), `mods_chain` 4, **`shaders_glue` 3** (2 old + 1 new), with the feature also `shaders_pipeline` 1 (401 shaders through `BuildRequest`: 1.9 s): all pass; `grizzy_e2e` 2 `#[ignore]`d (1 without the feature) |
| `-p ibuilder-shaderbuild --features host-tools,f2-finish` | 11 + 6 + 2 + 1 + 2 + 1 + 1 + 1 = 25 pass, 2 ignored (unchanged: the package B suite) |
| `grizzy_e2e -- --ignored` (the old test: no shaders) | pass, 7.55 s, `shaders=none` as before |
| `grizzy_e2e --features shader-host-tools -- --ignored grizzy_end_to_end_with_shaders` (new) | pass, 100.35 s |

New tests: `cli.rs`: the options are refused before anything is written (each stage-3 option without `--shaders`, no chain, a chain with a `/`, `--shader-jobs 0`, no engine, an engine without the tools, a build without the feature says which feature),
`engine_tool_paths`, the config rows only with `--shaders` and `--set` last, **a mock run with `--shaders` runs stage 3 between the rewrite and the assembly** (labels in order, a pack is installed), the binary's `--help` lists the options and exits 2 on a bad combination;
`meter::labels` (the two stage-3 steps); `shaders_glue`: the pipeline's cache folder name is the one `ibuilder_shaderbuild::cache::Store` creates and the build's sweep removes the same ones; `pipeline_spec`: the cache test rewritten for the folder
(`the_shared_shader_cache`) and `a_symlink_named_like_a_stale_cache_is_removed_and_never_followed`. The runs of `cli.rs` are serialised by a mutex: the meter's stage hook is process-global, two runs in one test process miscounted each other's steps
(found by a failing test, not by a user). iOS: `PATH=/opt/homebrew/opt/rustup/bin:$PATH IPHONEOS_DEPLOYMENT_TARGET=14.0 cargo build --release --target aarch64-apple-ios-sim -p ibuilder-ffi --features real-stages` (finished, 26 s),
`--target aarch64-apple-ios -p ibuilder-ffi --features real-stages` (finished, 24 s, 29 exported `_ibuilder_*` symbols, unchanged) and `-p ibuilder-pipeline` (finished): the default build is still iOS-clean (the `std` object inside the archive carries
its own `Command` code as in any Rust build; nothing of ours uses it; `shader-host-tools` is not in these builds).

## 5. Findings, deviations, what is not verified

* **Found, not changed (Python bug, ported faithfully)**: `shader_pulling_wanted(work/profile, cfg)` reads `work/profile/game.tsv`, but the game table is at `work/profile/<id>/game.tsv`: a profile's `pso_mtl4` row is never seen; pulling libraries only with `shader_pull=1`.
  Not exercised (Grizzy has no `pso_mtl4` row, so the run's "pulling skipped" does not prove it). `docs/chain/PIPELINE-RUST.md` section 9.
* The derived profile lacks the stored provisional profile's `depth_mode zero` row (measured by a check boot on a device). Not a stage-3 input; noted because the two profiles are not interchangeable.
* `ibuilder-sign` leaves the bundle unsealed (`codesign` on the folder fails, on the Mach-O passes): (4).
* **Cache name**: the task text says `cas-<chain>.dmsh`; the code, `ibuilder_shaderbuild::cache`, creates `cas-<chain>.dmsb` ("b" for build), and the pipeline now keeps exactly that (a test ties the two names together).
* **One loss in the record**: cold 1's `peak memory footprint` was overwritten by a later run writing the same log name (its wall, CPU, max RSS and step table are from the run's own output, shown above); cold 2 has all of them.
* **Not verified**: anything on a physical iPhone, the FFI / C# path with the app's in-process translator callbacks (this run uses child processes; the in-process tools are what the phone has, so there its stage-3 time has no child-process start costs, which this run pays for every tool call), the memory/thermal gate under pressure (it never paused here: 25 GB free, no thermal state), a second game, pulling libraries on a real profile, the Python's `libdiff`-style comparison for a profile with the stored map (the
  digest proof above is by recomputation; a library built from a stored-map profile through the pipeline was not made), a quiet-machine benchmark (load 5-18 throughout), the 1-thread resume to its end (stopped at 15 s; the 6-thread resume is the one that finished).
* One read-only `git status --short` was run inside `iOSBuilder` (to see if the checkout is a repository: it printed nothing); git may refresh the index's stat cache there, nothing else was written or run in it. Nothing else outside this repo and the scratch folder was written.

## 6. Files

`crates/ibuilder-cli/src/{lib.rs (CliArgs, check_shader_args, engine_tool_paths, make_config), bin/ibuilder.rs (options), meter.rs (stage-3 steps), adapters.rs (wired())}`, `crates/ibuilder-cli/tests/{cli.rs, shaders_glue.rs, grizzy_e2e.rs}`,
`crates/ibuilder-pipeline/{src/shaderpack.rs (shader_cache_folder), src/shaders.rs (doc), tests/pipeline_spec.rs}`, `scripts/e2e-grizzy.sh`, `docs/chain/E2E-RUST.md`, `docs/chain/PIPELINE-RUST.md` (section 9), `docs/shaders/SHADERBUILD-RUST.md` (two stale notes).
Run data (not in the repo): `$S2/I/{cold,resume}` (output folders), `$S2/I/*.{stdout,time,report.tsv,uptime.*,stages.log}`, `$S2/I/{run,kill-run,verify}.sh`, `imgx.py`, `inputs_digest.py`, `rust-e2e.log`, `tests-final.log`.
