# Contributing to iBuilder

Thanks for helping. This is a small project; these rules keep it correct and safe to publish. Start with
[docs/CONVENTIONS.md](docs/CONVENTIONS.md) (the rules every contributor follows) and [docs/README.md](docs/README.md)
(the index of every document).

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
* **Tests**: a fix comes with a test that fails without it; a reader of untrusted input (a container, a module, a
  profile) comes with a test that feeds it truncated and extreme data and expects an `Err`, never a panic.
* **The byte-compare oracle**: for every valid input, the output of a stage stays **byte-identical** to the reference
  toolchain's (goal 1 of [docs/CONVENTIONS.md](docs/CONVENTIONS.md)). If a change must alter an output, say so
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

## Bugs, ideas, security

Use the issue forms for bugs and feature requests. For a vulnerability, follow [SECURITY.md](SECURITY.md): privately,
never in a public issue. By contributing you agree that your contribution is released under the project's
[MIT license](LICENSE).
