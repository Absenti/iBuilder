# Contributing to iBuilder

Thanks for helping. This is a small project; these rules keep it correct and safe to publish. The documents are
indexed in [docs/README.md](docs/README.md); [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) is the place to start.

## What the project is

iBuilder is a Rust rewrite of the toolchain of iOSBuilder (Python, C# and C), which builds a Switch game as a native iOS
game on the iPhone from the user's own dump. The engine host (nxnative) and the Ryujinx / LibHac libraries are third
party and stay as they are. Goals, in this order: (1) **byte-identical output** to the Python toolchain, (2) lower wall
time, (3) lower peak memory (the iPhone's jetsam ceiling is about 3.3 GiB), (4) no embedded runtimes (no CPython, no
CoreCLR) on the phone.

## Build and test

You need a recent stable Rust toolchain (the workspace uses edition 2024) and, for the iOS targets, rustup's
`aarch64-apple-ios` and `aarch64-apple-ios-sim` targets.

```bash
cargo build --release --workspace          # everything that builds on a Mac
scripts/test-report.sh                     # the whole test suite, then what the green number really means
IBUILDER_STRICT_FIXTURES=1 scripts/test-report.sh   # the same, failing on any skipped test
scripts/check-patches-apply.sh             # the app patches apply, in order, to a fresh copy of the C# snapshot
scripts/check-no-spawn.sh <GameApp.app>    # a built app imports no fork / exec / spawn (give it the .app, not one file: the allowed imports are judged by their path)
scripts/check-forbidden-files.sh           # no key, certificate, profile, game container or Switch module in the tree (CI job `forbidden-files`)
```

`scripts/check-forbidden-files.sh` reads the first 516 bytes of every file of the tree (1,263 files at the time of writing), so its cost is the number of files, not their size: about **13 seconds** on a
Mac (measured: 12.7 s wall, 3.3 s user, with bash 3.2, on the tree as it stood on 2026-11-21; it was 95 s before the 2026-11-21 fix of its hex-dump step, which was quadratic in bash). Run it before a
commit; it needs no network and writes nothing in the tree. It accepts a sub-folder of the repository as an argument (the allowlist is then matched on the path relative to the repository).

`scripts/build-ios-ffi.sh` reuses the previous `libibuilder_ffi.a` of a target when nothing that goes into it changed (the crates' sources, `Cargo.lock`, the compiler, the flags, the features, the SDK, the
script itself): it prints `== ffi cache <target>: HIT` or `MISS: <what changed>`. `FFI_CACHE=0` always runs cargo; `FFI_CACHE_DRY=1` shows the key and the decision without building.

Many tests compare against the reference toolchain's outputs or use a game's files, which the repository does not
contain. Such a test calls `ibuilder_testkit::skip(reason)` and is **listed as skipped**, not passed:
`scripts/test-report.sh` says "N passed, K of them skipped for missing fixtures". Quote both numbers in your PR.
CI (`.github/workflows/ci.yml`) runs the same suite with `cargo nextest run --profile ci` (`.config/nextest.toml`) plus `cargo test --doc`,
and counts the skips from the same log; it also runs the C tests of `app/c` (`app/c/test/run.sh`) and a self-test of the no-spawn gate.

Building the app (`scripts/build-app.sh device`) needs Xcode, the .NET SDK with the iOS workload, and **a checkout of
the original iOSBuilder project as an input** (the prebuilt engine host, SPIRV-Cross archives and the zsign, OpenSSL and
zstd sources come from it; it is only read). Copy `scripts/local.env.example` to `scripts/local.env` (ignored by git),
set `IOSBUILDER`, and the scripts and tests find it. A build made with the defaults embeds stand-in fonts derived from
macOS's: it is for your own phone and must never be shared (see [NOTICE.md](NOTICE.md), "Fonts").

## Pull requests

* **Small and focused**: one change, one PR. Say what it does and why.
* **Title**: [Conventional Commits](https://www.conventionalcommits.org/) form, **72 characters at most**: `<type>(<optional scope>): <summary>`, with
  a type among `feat`, `fix`, `chore`, `docs`, `refactor`, `perf`, `test`, `build`, `ci`, `style`, `revert`, a lowercase scope, no final period, and
  `!` after the type or scope for a breaking change. For example `feat(shaders): read EA EASF archives`, `fix: keep the cover after deleting a game`.
  Main takes squash merges only, so the title becomes the commit's subject; the details go in the description. The required check `pr-title`
  ([.github/workflows/pr-title.yml](.github/workflows/pr-title.yml)) refuses any other title.
* **Tests**: a fix comes with a test that fails without it; a reader of untrusted input (a container, a module, a
  profile) comes with a test that feeds it truncated and extreme data and expects an `Err`, never a panic.
* **The byte-compare oracle**: for every valid input, the output of a stage stays **byte-identical** to the reference
  toolchain's (goal 1 above). If a change must alter an output, say so
  explicitly, justify it, and update the pinned hashes in the same PR.
* **iOS constraints**: the library crates ship on an iPhone: no `fork`, `exec` or `std::process::Command`, no
  macOS-only API, prefer `#![forbid(unsafe_code)]` and justify any `unsafe`.
* **No secrets, no game assets**: never commit or paste `prod.keys`, certificates, provisioning profiles, game dumps,
  NSO modules, shader dumps, console fonts or unredacted logs. Fixtures are small and synthetic. `.gitignore` helps, it
  does not replace care.
* **Truthful claims**: in the PR and in the docs, say what you ran, with the command and its result, and what you did
  **not** verify. Separate *measured* from *estimated*. A skipped test is not a pass.
* **Docs**: update the document of the crate or topic you changed (see [docs/README.md](docs/README.md)).
* **CI must be green.** Merging needs one approving review, including the code owner's
  ([.github/CODEOWNERS](.github/CODEOWNERS)).

## Rules for the code

* **The original iOSBuilder checkout is a read-only input** (`$IOSBUILDER`, see [docs/BUILDING.md](docs/BUILDING.md)): tests
  compare against its reference outputs and the app build takes prebuilt pieces from it. Nothing here writes there. Run
  its Python with `PYTHONDONTWRITEBYTECODE=1`, from copies in a scratch folder.
* **Scratch folders** (generated oracle outputs, Python reference environments, cargo target directories) live outside the
  repository; `target/` is ignored.
* **Fixtures that are absent skip, loudly.** A test that needs a file the repository does not contain calls
  `ibuilder_testkit::skip(reason)` and returns (`ibuilder_testkit::have(path)` is the usual check). Never a bare
  `eprintln!("SKIP")` or a silent `return`: libtest counts those as passed. `skip` prints, appends to
  `$CARGO_TARGET_DIR/skipped-tests.log`, and panics when `IBUILDER_STRICT_FIXTURES=1`.
* **A test count is `scripts/test-report.sh`'s count**, not cargo's: "N passed, K of them skipped for missing fixtures".
  Quote both, and the machine.
* **Untrusted input.** Whatever is read from a file (a container, an image, a module, a config) is the file's word: sizes,
  counts and offsets are checked (`checked_*`, `u128`, or against the file's own length) before they slice, index, loop or
  allocate, and damaged input is an `Err`, never a panic (a release build is `panic = "abort"`). Every reader of such input
  has a test that feeds it truncations and extreme values (`crates/ibuilder-rewrite/tests/damaged_module.rs` is the model).
* **iOS constraints.** Core library crates do not use `fork`, `exec`, `std::process::Command` or macOS-only APIs; threads
  (`rayon`) are fine. Prefer `#![forbid(unsafe_code)]` and justify any `unsafe`. Keep crates buildable for
  `aarch64-apple-ios`. Mac-only binaries live in `src/bin/`.
* **Layout.** A Cargo workspace, `members = ["crates/*"]`, names `ibuilder-<thing>`, library first (`src/lib.rs`), an
  optional CLI in `src/bin/`, edition 2024. Dependencies are kept few (`rayon`, `memmap2`, `sha2`, `lz4_flex`, `capstone`,
  `anyhow` / `thiserror`, `libc`, `clap`, `serde` / `serde_json`, `hex`); justify any other in the crate's doc.
* **Toolchains.** The default `cargo` builds for the host. For iOS targets, prefix the command with the rustup toolchain's
  `bin` (for Homebrew: `PATH=/opt/homebrew/opt/rustup/bin:$PATH cargo build --release --target aarch64-apple-ios-sim -p <crate>`);
  without it cargo may pick another rustc and fail with E0463.
* **Measuring.** Wall time and memory with `/usr/bin/time -l`, release builds. A shared machine makes numbers
  indicative: say how loaded it was.
* **Reporting a change.** State what is done, the exact verification commands and results, what is not verified (the
  skipped tests among them), and the numbers (wall time, peak RSS, versus Python).

## Bugs, ideas, security

Use the issue forms for bugs and feature requests. For a vulnerability, follow [SECURITY.md](SECURITY.md): privately,
never in a public issue. By contributing you agree that your contribution is released under the project's
[MIT license](LICENSE).
