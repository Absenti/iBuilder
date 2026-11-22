# Assemble, sign, pack (`ibuilder-macho`, `ibuilder-sign`, `ibuilder-pack`)

The last three stages of the toolchain: join the host dylib and the patched game modules into one framework
executable, give it an ad-hoc code signature, and write the `.nxgame` package. All three are ports of external
Python tools (iOSBuilder, read-only) and are checked for byte-identical output against them. Pure Rust, no process
spawning, no embedded interpreter; all build for `aarch64-apple-ios` and `-ios-sim`.

## ibuilder-macho: framework assembler

No runtime dependencies (`#![forbid(unsafe_code)]`).

| Python | Rust |
| --- | --- |
| `assemble.py` | `src/assemble.rs` (`Plan`, `assemble_files`, `Game`, `Piece`), bin `ibuilder-assemble` |
| `macho_write.py` (from-scratch `MH_DYLIB` writer) | `src/writer.rs`, bin `ibuilder-macho-write` |
| `fix_tlv.py` | `src/tlv.rs`, bin `ibuilder-fix-tlv` |
| `gen_embed.py` | `src/embed.rs`, bin `ibuilder-gen-embed` |
| shared | `macho.rs` (parser, constants), `io.rs` (positioned I/O), `cli.rs`, `error.rs` |

Not ported: the prebuild link of the engine host (clang + Xcode `ld`, a Mac-side build step), the codesign driver
script and the C probe harnesses.

```rust
let report = ibuilder_macho::assemble_files(host, out, &Game::Dir { dir: flat.into(), modules }, "")?;
```

`Game::Dir` reads `<dir>/<name>.patched.bin` and `<name>.fixups.tsv`; `Game::Sources` takes explicit files;
`Game::Synthetic` backs `--synthetic`. `Plan::new` does the layout and every check before anything is written;
`Plan::write` streams into any `WriteAt`.

```sh
cargo build --release -p ibuilder-macho
ibuilder-assemble --host host.dylib --game <dir> --out Game.framework/Game [--modules main subsdk0 sdk] [--tag T] [--synthetic]
ibuilder-macho-write --out probe.dylib [--platform sim|ios] [--name N] [--magic 0x..] [--minos V] [--sdk V] [--uuid HEX32]
ibuilder-fix-tlv natural.dylib segaddr.dylib     # patches the second file in place
ibuilder-gen-embed [--tag T] [--jit IMG] <dir> <out.S> <module...>
```

CLI stdout (`k=v` and `segment ...` lines) mirrors the Python's. `--uuid` is the one addition: the Python draws a
random `LC_UUID`, the Rust CLI does too unless pinned.

Design points:

* Streaming: only the host header, load commands and chained-fixups blob are read up front. The output is pre-sized
  (padding stays a hole) and each piece is copied with `pread`/`pwrite` through one reusable 1 MiB buffer, so memory
  does not grow with the game. `mmap` is not used (it needs `unsafe` and counts toward the process footprint).
* All checks run before the output exists; a failed write removes the partial file; output equal to an input is refused.
* Asserts and write order mirror `assemble.py`. A back patch area (`BACK_LEN`) is computed but never emitted, like the
  Python. Errors are `Error` values instead of tracebacks; the fixups blob must lie inside `__LINKEDIT`.

Measured on 2026-11-20 on an M1 Pro (194.6 MB framework, three modules): about 0.05 s and 3 MiB max RSS, against
about 0.2 s and 767 MiB for the Python.

## ibuilder-sign: in-process ad-hoc signature

Deps `sha2`, `rayon`; `#![forbid(unsafe_code)]`. iOS has no `codesign` and `dyld` refuses an unsigned dylib, so the
toolchain signs the framework itself. Port of `adhoc_sign.py`: one SuperBlob with a CodeDirectory (v0x20400, SHA-256,
4096-byte pages, no special slots), `LC_CODE_SIGNATURE`, and `__LINKEDIT` grown to hold it.

| Python | Rust |
| --- | --- |
| `_load_commands`, `_inspect`, `_slack` | `src/macho.rs` |
| `sign()` | `src/adhoc.rs` (`plan` -> `Plan::apply_to_file`, `sign_bytes`, `sign_file`) |
| `verify()` | `src/adhoc.rs` (`verify_file`, `verify_bytes`) |
| CLI | `src/bin/ibuilder-sign.rs` |
| signer seam (new) | `src/signer.rs` (`CodeSigner`, `AdhocSigner`, `CallbackSigner`) |

```sh
cargo build --release -p ibuilder-sign
ibuilder-sign <dylib> [identifier]          # sign, then verify
ibuilder-sign sign|verify <dylib> [identifier]
ibuilder-sign --shrink-linkedit <dylib>
```

Identifier precedence: argument, then the dylib's `LC_ID_DYLIB` name, then the file name.

* Plan, then apply: the signature is computed first, hashing the file in 1 MiB chunks in parallel through a virtual view
  (patched header, file, zero padding); the header is written last so an interrupted run never leaves a header pointing at
  a missing blob. A 190 MB framework signs in about 24 MiB RSS.
* Default output is byte-identical to the Python. The Python only ever grows `__LINKEDIT`, so re-signing a file that had
  a larger signature leaves a segment past end of file, which `codesign --strict` rejects. `--shrink-linkedit`
  (`SignOptions { shrink_linkedit: true }`) fixes that and is opt-in.
* `CodeSigner` is the swap point for a device-identity signer (CMS signature, team ID, entitlements from the provisioning
  profile), which cannot be done by an ad-hoc signer. `CallbackSigner` lets the app plug an external signer in without
  this crate depending on it. `AdhocSigner` does not write `CodeResources` or bind `Info.plist`.

Measured on 2026-11-19 on an M1 Pro (190.8 MB framework, includes verify): 0.06 s and 24 MiB, against 0.38 s and
385 MiB for the Python.

## ibuilder-pack: `.nxgame` packager and comparator

Deps `sha2`, `rayon`, `libc`; `#![deny(unsafe_code)]` with one `unsafe` call (`clonefile`). Port of `pack_nxgame.py`
and `compare_nxgame.py`.

A `.nxgame` is `<title ID>.nxgame/` holding `DmGame<title ID>.framework/` (executable, `game.manifest.json`,
`profile.tsv`, `dm/`, `msl/b-<chain>.img`, `icon.png`, `Info.plist`), `romfs.img`, `aoc/`, and `package.json` with the
SHA-256 of every framework file.

| Python | Rust |
| --- | --- |
| `read_profile`, `console_of`, `nro_count` | `src/profile.rs` |
| RomFS identity (FNV-1a 64 over header + index) | `src/romfs_id.rs` |
| `build_manifest`, `write_manifest`, `main` | `src/pack.rs` (`pack`, `PackArgs`, `PackOutcome`) |
| `json.dumps` / `json.loads` spellings | `src/json.rs` |
| `--clone` | `src/clone.rs` (`clonefile(2)`, copy fallback) |
| Python `str` semantics | `src/pyutil.rs` |
| `compare_nxgame.py` | `src/compare.rs` |

```sh
cargo build --release -p ibuilder-pack
ibuilder-pack pack --out <title>.nxgame --profile profile.tsv --framework <exe|dir> --dm-dir <dir> --msl b-<chain>.img \
    --icon icon.png --romfs-img RomFS.img --aoc <dir> --nro-table nro.tsv --package-version v0 [--clone] [--force] [--json]
ibuilder-pack pack --manifest-only --out <dir|file.json> --profile profile.tsv --package-version v0 --chain b-xxxx
ibuilder-pack compare <reference> <candidate> [--reason KEY=TEXT]... [--json-out FILE] [--label TEXT]
```

Options, defaults, refusals (`[RED] pack_nxgame: ...`, exit 1) and report lines follow the Python; only `--opt value`
and `--opt=value` are accepted (no argparse abbreviations).

JSON is hand-written, not `serde_json`: output is compared byte for byte, which needs insertion-ordered objects and
Python's spellings (`ensure_ascii` off, `1e+16`, `" : "`). The manifest matches `json.dumps(indent=2)` plus a newline;
`package.json` matches the Mac app's Swift encoder (sorted keys, no trailing newline). `package.json` file lists sort
component-wise, as Python sorts `Path` objects.

## Verification

```sh
cargo test --release -p ibuilder-macho -p ibuilder-sign -p ibuilder-pack
```

Fixture-dependent oracle tests print `SKIP ... : reason` and pass when their inputs are absent; the unit tests always run.

* `ibuilder-macho`: unit tests (encoders, load-command layout, export trie, `fixups.tsv` rules, a synthetic miniature host
  assembled in memory and checked structurally). `tests/oracle.rs` assembles real hosts plus Grizzy modules and compares
  size and SHA-256 with the Python output and with a stored framework; it also checks `macho_write`, `gen_embed`
  fixtures (`tests/fixtures`, no game data), the refusals, and peak RSS below 96 MiB. `scripts/macho-oracle.sh` and
  `scripts/macho-verify.sh` regenerate and compare the Python side.
* `ibuilder-sign`: unit tests on hand-made Mach-O files; `tests/oracle.rs` signs copies with the Python and the Rust
  (clang-built dylibs, host dylibs, a full framework), requires identical bytes, then `codesign --verify --strict`
  (macOS only).
* `ibuilder-pack`: `tests/oracle.rs` runs the Python and Rust packers on synthetic trees and requires identical output
  trees, stdout and refusal messages (21 pack cases, 8 compare cases, JSON dumps in 4 configurations); needs a Python
  reference. A re-pack of a real package reproduced its `package.json` and framework tree byte for byte.

## Known limits

* macho: only the Grizzy modules (one FAR-layout main, two normal-layout modules, no NROs, no back area) were compared
  with the Python; other layouts rest on unit tests. The Rust-assembled framework was not itself booted in this
  repository's tests. Host segments other than `__TEXT`/`__DATA_CONST`/`__DATA`/`__LINKEDIT` are dropped, as in Python.
* sign: ad-hoc only (no entitlements, requirements or CMS), 64-bit thin Mach-O only. Acceptance by a physical iPhone's
  kernel is not covered by these tests (the simulator does not re-hash pages).
* pack: integers beyond 128 bits in JSON are refused; directory metadata is not copied; the Swift package verifier
  and a device import were not part of the test runs.
