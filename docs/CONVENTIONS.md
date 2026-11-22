# iBuilderRust: conventions for every contributor (human or agent)

## Mission

iOSBuilder (Python + C# + C, a separate, original project) builds a Nintendo Switch game as a **native iOS game, on the iPhone**, from the user's own dump: extract the NSP →
**rewrite** the ARM64 NSO modules (`patch_nso.py`, CPython + capstone) → (translate shaders) → assemble a Mach-O framework with no linker (`assemble.py`, `macho_write.py`) → sign it
in-process → boot the guest in the same process. The engine itself (nxnative, C host) is third-party, pinned, and stays as it is.

iBuilderRust is a **Rust rewrite of the toolchain** (everything except the third-party engine host and the Ryujinx/LibHac C# libraries). Goals, in this order: (1) **byte-identical output**
to the Python toolchain, (2) lower wall time, (3) lower peak memory (the iPhone's jetsam ceiling is ~3.3 GiB, and the Python rewrite peaks at ~2.2 GiB on the phone / 4.1 GiB RSS on the Mac
when serial), (4) no embedded runtimes (no CPython, no CoreCLR) so the phone has more RAM and address space left for the game.

## Hard rules

1. **The original iOSBuilder checkout (`$IOSBUILDER`, with any worktree of it, `$IOSBUILDER_WORKTREE`) is a read-only reference input.** The tests compare against its Python reference outputs and
   the app build takes prebuilt pieces from it; nothing in this repository writes there. That includes `__pycache__`: run Python with `PYTHONDONTWRITEBYTECODE=1`, and from copies in a scratch
   folder (see "Reference environment"). The same holds for any other third-party checkout you read (for example an engine tree).
2. Write only inside this repository (your own crates and docs) and in a **scratch folder** outside it (`$SCRATCH`): generated oracle outputs, Python reference environments, cargo target
   directories. Do not edit another contributor's crate without agreeing it with them; if a change is needed there, say so in your report.
3. **Never commit secrets, keys, dumps, certificates or game data.** That means `prod.keys` and any other Switch key file, `.p12` files, provisioning profiles, private keys, tokens, game
   dumps, NSO / RomFS / NCA images and anything extracted from them. Do not read, copy or print a key file or a certificate in a tool, a test or a log. The small hand-made fixtures that
   are in the repository are synthetic (see the crate docs). Big fixtures are referenced by path, outside the repository (`scripts/local.env`, below). A test that needs one calls
   **`ibuilder_testkit::skip(reason)`** and returns when it is absent (never a bare `eprintln!("SKIP ...")`, never a silent `return`: libtest counts such a test as PASSED and hides its
   stderr, so the run looked like a verification that did not happen). `skip` prints, appends `crate<TAB>test<TAB>reason` to `$CARGO_TARGET_DIR/skipped-tests.log`, and PANICS when
   `IBUILDER_STRICT_FIXTURES=1` (a machine that has every fixture, or a check before a release, runs with it). `ibuilder_testkit::have(path)` is the usual `exists()` + skip. Small
   hand-made unit fixtures are fine, and are what makes a check run on every machine.
4. **Truthfulness.** Report only what you ran. Every claim needs the command and its output. Name what is NOT verified. Never discard a diagnostic (`2>/dev/null`, `| tail` hiding a failure).
   A failing test is a result; report it. **A test count is `scripts/test-report.sh`'s count, not cargo's:** "N passed, K of them skipped for missing fixtures" (it runs the whole
   workspace, counts the skips `ibuilder_testkit::skip` recorded, lists them by crate, and fails in strict mode when K > 0). Quote both numbers and the machine they were measured on.
   Cargo's "N passed" alone also counts every test that compared nothing because the Python oracle or a game's files were not there.
5. iOS constraints (the code ships on an iPhone): core library crates must not use `fork`, `exec`, `std::process::Command`, shelling out, or macOS-only APIs; threads are fine (`rayon`).
   Prefer `#![forbid(unsafe_code)]`; justify any `unsafe`. Keep crates buildable for `aarch64-apple-ios` (no `std::os::unix`-only assumptions beyond what iOS has). Binaries/CLIs for the
   Mac may live in `src/bin/`.

## Local configuration (`scripts/local.env`)

No machine path is written in the scripts or the tests. The inputs of your machine come from environment variables, or from the optional, gitignored file **`scripts/local.env`** (copy
`scripts/local.env.example` and edit it). Every script sources it first, and so do the Rust tests (`ibuilder_testkit::local_env`), so a plain `cargo test` and `scripts/test-report.sh` see the
same inputs. The variables: `IOSBUILDER` (the original iOSBuilder checkout), `IOSBUILDER_WORKTREE` (the tree with its newest work, default `$IOSBUILDER`), `IBUILDER_SCRATCH` and
`IBUILDER_SCRATCH_OLD` (scratch folders with generated oracle outputs), and a few optional ones documented in the example file. A script that needs one that is not set stops and names it;
a test that needs a missing input skips. The Python tools under `crates/*/tools` read the environment only (`. scripts/local.env` first).

In the documents, `$REPO` is the repository root, `$IOSBUILDER` and `$IOSBUILDER_WORKTREE` are the variables above, `$SCRATCH` and `$SCRATCH_OLD` stand for `$IBUILDER_SCRATCH` and `$IBUILDER_SCRATCH_OLD`
(the oracle outputs of the first part of the work are under `$SCRATCH_OLD`; the two may be one folder), `<SIMULATOR-UDID>` is the UDID of a booted simulator (`xcrun simctl list devices booted`), `$SIM_DATA` is that simulator's data folder (`~/Library/Developer/CoreSimulator/Devices/<UDID>/data`)
and `<DATA-UUID>` / `<BUNDLE-UUID>` are the folder names of an app's data and bundle containers in it.

## Layout

Cargo workspace at the repo root, `members = ["crates/*"]` (just add `crates/<name>/`). Names: `ibuilder-<thing>`. Library first (`src/lib.rs`), an optional CLI in `src/bin/`. Edition 2024.
Allowed deps: `rayon`, `memmap2`, `sha2`, `lz4_flex`, `capstone`, `anyhow`/`thiserror`, `libc`, `clap`, `serde`/`serde_json`, `hex`; anything else: justify it in your doc. Shared test helpers:
`ibuilder-testkit` (`compare_files`, `first_diff`, `sha256_hex`, `peak_rss_bytes`, `skip`, `have`, `local_env`, `grizzy_exefs`, `grizzy_oracle`).

**Use your own cargo target dir** so parallel builds do not block each other: `export CARGO_TARGET_DIR=$SCRATCH/target-<your-name>`, and build/test with `--release` when timing.

Docs: one `docs/<crate-or-topic>.md` per deliverable: what it ports (source file → Rust module), how to run it, what is verified (exact commands and results), what is not, measured time/memory.
"Verified" means a test that ran and compared, not one that skipped.

Input read from a file (a container, an image, a module, a config) is the file's word, not ours: sizes, counts and offsets are checked (`checked_*`, `u128`, or against the file's own length)
BEFORE they are used to slice, index, loop or allocate, and a damaged input is an `Err`, never a panic (a release build is `panic = "abort"`: a panic is the app dying). Every reader of such
input has a test that feeds it truncations and extreme values (`ibuilder-rewrite/tests/damaged_module.rs` is the model).

## Toolchains (host and iOS)

* The default `cargo`/`rustc` builds and tests for the host (`aarch64-apple-darwin`).
* **iOS targets** need a rustup toolchain with `aarch64-apple-ios`, `aarch64-apple-ios-sim` (and `aarch64-apple-ios-macabi`). With Homebrew's `rustup` keg, prepend it to PATH **for the command
  only**: `PATH=/opt/homebrew/opt/rustup/bin:$PATH cargo build --release --target aarch64-apple-ios-sim -p <crate>`. Without the prefix cargo picks the other rustc and fails with E0463 "can't
  find crate for `core`". Do not change PATH globally.
* Run an ios-sim binary in an already-booted iOS simulator: `xcrun simctl spawn <simulator UDID> <binary>` (it shares the host filesystem; do not boot, shut down or erase a simulator you did
  not start). Simulator timings are the Mac's CPU, not the phone's.
* A rayon + sha2 probe builds for device (arm64, LC_VERSION_MIN_IPHONEOS) and for the simulator (platform 7) and runs in the simulator (`os=ios`, 8 threads).

## Reference environment (Python oracle)

* Python 3.14 + capstone **5.0.7** + lz4 + zbic: `$SCRATCH/ref/venv/bin/python` (the exact capstone version matters: the rewriter compares `op_str` strings, and `capstone-sys 0.18`
  bundles 5.0.6, so verify decode equality before trusting it).
* A copy of the engine's Python tools and game profiles: `$SCRATCH/ref/engine/dm/{tools,games}` (a writable copy; edit or instrument the copy freely, never the original).
* Grizzy and the Lemmings (`0100DFA027504000`) is the reference game. Inputs (read-only): `$IOSBUILDER/out/ref-out-grizzy/ExeFS/{main,sdk,subsdk0}` (NSO files; main 75,035,192 B, sdk
  6,314,191 B, subsdk0 3,353,537 B) and the RomFS image `$IOSBUILDER/out/ref-img-grizzy/`. They are the user's own dump, never in this repository.
* **Oracle outputs** (the Python rewrite; byte-identical to the engine's own `patched_v0/`): `$IOSBUILDER/out-py/pyrewrite/out/<module>/<module>.{patched.bin,fixups.tsv,sites.tsv,gates.tsv,cuts.tsv,obs.tsv,changed.tsv,match.tsv}`
  plus `build_src.sha256`. Python timing on an M1 Pro (8 cores, 16 GB) for `main`: **serial 200.5 s wall, 4.13 GB max RSS, 12.5 GB peak footprint; with the fork pool 69.6 s wall, 1.94 GB
  RSS**. On the iPhone (A17 Pro, serial): `main` 113–155 s, footprint ~2.2 GiB; `sdk` ~10–13 s; `subsdk0` ~5–7 s.
* Python source of truth to port: `$IOSBUILDER/engine/dm/tools/*.py`, `$IOSBUILDER/tools/*.py` (`assemble.py`, `macho_write.py`, `pack_nxgame.py`, `shader_post.py`, ...), `gameapp/`, `pipeline/`.
  The newest work (on-phone shader chain, live shaders, warm-up, ...) is in `$IOSBUILDER_WORKTREE`: read it when a file exists there and not in the main checkout.
* Project docs worth reading: `$IOSBUILDER/docs/{REWRITE,MILESTONE-3,MEMORY,ASSEMBLER,MACHO,SIGN-IN-APP,PACKAGE,PROFILE-DERIVED}.md`, `engine/docs/reference/rewriter.md`, and in the worktree
  `docs/{PERF-RESEARCH,DECISIONS,STATE-GAMEAPP}.md`.

## Measurement protocol

Wall time and memory with `/usr/bin/time -l <cmd>` (macOS: `real`, `maximum resident set size` in bytes, `peak memory footprint`). Run the Rust binary in `--release`. A shared machine makes
numbers *indicative*: say how loaded it was, and re-run the official benchmark on a quiet machine. Do not run the 200 s / 4 GB Python `main` reference more than needed (use the stored oracle
outputs); never run two heavy Python jobs at once.

## Reporting a change

State what is done, the exact verification commands and results (identical / how many files), what is NOT done or NOT verified (the skipped tests among them: `scripts/test-report.sh`), the
numbers (wall, peak RSS, versus Python), and the paths of your docs and crates.
