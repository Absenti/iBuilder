# ibuilder-macho: the linker-less framework assembler in Rust

Crate `crates/ibuilder-macho`. Rust port of iOSBuilder's hand-made Mach-O writer and framework assembler, **byte-identical**
to the Python tools, streaming (about 3 MiB of memory for a 195 MB framework instead of about 770 MiB), pure `std`.

## 1. What it ports

| Python (iOSBuilder, read-only) | Rust | Notes |
|---|---|---|
| `tools/assemble.py` (382 l) | `src/assemble.rs` (`Plan`, `assemble_files`, `Game`, `Piece`) + `src/bin/ibuilder-assemble.rs` | host dylib + the game's patched modules -> one dylib/framework executable, no linker |
| `tools/macho_write.py` (445 l) | `src/writer.rs` (`build_probe`, load-command builders, export trie, arm64 encoders) + `src/bin/ibuilder-macho-write.rs` | the from-scratch `MH_DYLIB` writer (milestone 5 probe) |
| `tools/fix_tlv.py` (74 l) | `src/tlv.rs` (`fix_tlv`, `find_section`) + `src/bin/ibuilder-fix-tlv.rs` | prebuild-time `-segaddr` TLV fix (only the section bytes are rewritten, in place) |
| `engine/dm/ios/tools/gen_embed.py` (72 l) | `src/embed.rs` (`piece_ranges`, `FixupsHeader`, `gen_embed`) + `src/bin/ibuilder-gen-embed.rs` | the section layout the loader `vm_remap`s (`pieces()`); the `.S` generator is ported too so the layout can be cross-checked as `assemble_framework.sh` step 2b does |
| (shared helpers) | `src/macho.rs` (parser, `seg_lc_bytes`, constants), `src/io.rs` (`ReadAt`/`WriteAt`, chunked copy), `src/cli.rs` (argparse-like options), `src/error.rs` | |

**Not ported (not part of the assembler):** `tools/assemble_prebuild.sh` (clang + Xcode `ld` link of the engine host with `-headerpad` /
`-segaddr`: a Mac-side build step on the third-party engine), `tools/assemble_framework.sh` (driver that also runs `codesign` and
`xcrun simctl spawn`), `tools/asm_probe_host.c` / `macho_probe_host.c` / `loader_probe_host.c` (C harnesses that `dlopen` the result in
the simulator). `fix_tlv` is ported because the prebuild calls it, but the prebuild link that produces its inputs is not.

**Which version.** `macho_write.py`, `assemble.py`, `fix_tlv.py`, `assemble_framework.sh`, `asm_probe_host.c` and `docs/{ASSEMBLER,MACHO,LOADABLE,HOST}.md` are
**identical** in `iOSBuilder/` and the worktree `$IOSBUILDER_WORKTREE` (`diff` empty), and `gameapp_pipeline.py::do_assemble` in the
worktree only differs cosmetically (hard-links `flat/` instead of copying, stage numbering); the call `asm.build(host, out, flat, modules)` is the same. The only
differing file is `assemble_prebuild.sh` (worktree: one more symbol, `_front_metal_layer`, excluded from the stand-in generator; build-time, not
ported). `gen_embed.py` exists only in the main tree (the worktree has no `engine/`), so main's copy was ported. Newest hosts used as inputs
come from the worktree (see Oracle).

## 2. API and how to run

```rust
use ibuilder_macho::{assemble_files, Game};
// the on-phone call: assemble.build(host, out, flat, modules)
let report = assemble_files(host, out, &Game::Dir { dir: flat.into(), modules }, /*tag*/ "")?;
report.info.n_load_commands; report.segments; // what Python's info dict / `ordered` hold
```

`Game::Dir` reads `<dir>/<name>.patched.bin` + `.fixups.tsv` (the `flat/` layout); `Game::Sources` takes explicit per-module files (they need not
share a directory); `Game::Synthetic` is `--synthetic`. Lower level: `Plan::new(&host, &modules, tag)` does the whole layout and every check first
(so a failure leaves the output untouched, as in Python), `plan.write(&host, &modules, &mut out)` streams the bytes into any `WriteAt` (a `File`, or
`MemSink` in tests). `ibuilder-pipeline`'s `Assembler` adapter (`AssembleRequest { host, out, flat, modules }`) maps to the first call 1:1
(`n_load_commands` is the `ncmds` it wants).

CLIs mirror the Python ones, stdout included (`k=v` lines and `segment ...` lines, diffed against Python in the verification below):

```sh
export CARGO_TARGET_DIR=$SCRATCH_OLD/target-macho
cargo build --release -p ibuilder-macho
ibuilder-assemble --host host-prebuilt.dylib --game <dir with <name>.patched.bin/.fixups.tsv> --out Game.framework/Game \
                  [--modules main subsdk0 sdk] [--tag T] [--synthetic]
ibuilder-macho-write --out probe.dylib [--platform sim|ios] [--name DmProbe] [--magic 0x5A17BEEF] [--minos V] [--sdk V] [--uuid HEX32]
ibuilder-fix-tlv natural.dylib segaddr.dylib          # patches the second file in place
ibuilder-gen-embed [--tag T] [--jit IMG] <dir> <out.S> <module...>
```

`--uuid` is the one addition: `macho_write.py` takes `LC_UUID` from `uuid4()`, so its output differs run to run; the Rust CLI draws a v4 UUID from
`/dev/urandom` by default and `--uuid` pins it (the oracle comparison pins Python's `uuid4` the same way).

## 3. Streaming design (memory)

Python builds the image in RAM: the host, every piece (`section_bytes` concatenations: `bytes` + zero padding), a `bytearray` of the whole
output, then `bytes(out)` for the write: about four copies of a 195 MB framework (measured 767 MiB max RSS on the Mac).

Rust never materialises the image:

1. `Plan::new` reads only the host's 32-byte header, its load commands (7 KB) and its chained-fixups blob, and computes every offset.
2. `Plan::write` pre-sizes the output (`set_len`: unwritten ranges, i.e. the zero padding, are holes that read as zeros), then copies each piece
   from its source to its final offset with `pread`/`pwrite` through **one reusable 1 MiB buffer** (`io::copy_range`): host `__TEXT`, the six game
   `x0`/`x1` sections from `*.patched.bin`, the W segments, the host's `__DATA_CONST`/`__DATA`, and `__LINKEDIT` around the replaced fixups blob.
3. The write order is Python's (host prefix, then the rebuilt load commands over it, ...), so overlaps resolve the same way.

**`pread`/`pwrite`, not `memmap2`.** The task allowed mmap; it is not used: mapping a file needs `unsafe` (the crate is `#![forbid(unsafe_code)]`),
mapped input pages count toward resident size, and dirty pages of a `MAP_SHARED` output are charged to the process footprint (the jetsam ceiling on
the phone), whereas `pwrite` data lands in the page cache. `FileExt::{read_exact_at,write_all_at}` are std and exist on iOS. No mmap variant was
timed, so this is a design argument plus the measured result below, not an A/B. `rayon` is not used: the copy is memory-bound and takes ~50 ms.

Safety properties kept or added: all checks run before the output is created (a bad host leaves no file); a write failure removes the partial
output; `out` equal to an input (same device/inode) is **refused**, because Python happens to work there (it reads everything first) but a streaming
copy would truncate its own input.

## 4. Oracle

Python was run from copies (`$SCRATCH_OLD/macho/py`, `PYTHONDONTWRITEBYTECODE=1`, `python -I`) of the originals with `scripts/macho-oracle.sh`
(regenerates everything under `$SCRATCH_OLD/macho/oracle`). Inputs, all read-only references:

* **game modules:** `iOSBuilder/out-py/pyrewrite/out/{main,sdk,subsdk0}/<m>.{patched.bin,fixups.tsv}` (main 167,018,996 B with the FAR layout
  `PATCH_OFF -0xc60000`; sdk, subsdk0 with the normal layout). Their sha256 are pinned in `tests/oracle.rs`.
* **hosts** (prebuilt loadable host dylibs; frozen copies in the scratch folder, sha256 pinned in the test): `out-asm/host-prebuilt-sim.dylib` (2.9 MB),
  `out-load/host-prebuilt-loadable-{ios,sim}.dylib`, the device host bundled in the GameApp build `0d72694d...` (4.76 MB), and the **newest**,
  from the worktree: `out-load-ui/host-prebuilt-loadable-{ios,sim}.dylib` (6.66 / 4.43 MB) and the worktree's `GameApp.app/host-prebuilt.dylib` (6.2 MB).
* **stored framework (independent oracle):** `iOSBuilder/out-load/device/DmGame0100DFA027504000.framework/DmGame0100DFA027504000`
  (192,703,664 B, unsigned, produced by the original pipeline on 2026-11-16). Python `assemble.py` on the hosts above reproduces it **byte for byte**
  from host `0d72694d...` + the oracle modules, so that framework is a fixed point for both implementations, not only this session's Python run.
  (The signed `out-asm/DmGameProbe.framework` cannot be compared bytewise; its unsigned size and every layout number in
  `out-asm/assemble-framework.log` equal the Rust output's: `size=190820040 game_bytes=133939200 w_bytes=54001664 n_load_commands=41
  header_slack_used=6568`.)

## 5. Verification (commands and results)

All run on this Mac (M1 Pro) while other agents worked. Rust built with Homebrew `cargo 1.100.0`, `--release`, `CARGO_TARGET_DIR=$SCRATCH_OLD/target-macho`.

**Byte identity** (`scripts/macho-verify.sh`, which also rebuilds; every line is a `cmp` of the Rust output with the Python output):

```
IDENTICAL  assemble dev-4762784 ( 192703664 bytes)        IDENTICAL  macho-write sim
IDENTICAL  assemble asm-sim ( 190820040 bytes)            IDENTICAL  macho-write ios  (--name DmGameX --magic 0x12345678 --minos 26.1 --sdk 27.2.1)
IDENTICAL  assemble load-ios ( 192703760 bytes)           IDENTICAL  macho-write name/magic/version (--name A --magic 0xFFFFFFFF ...)
IDENTICAL  assemble load-sim ( 190838384 bytes)           IDENTICAL  gen-embed base/tag/jit/nro (.S and flags, 8 files)
IDENTICAL  assemble wt-ios ( 194600976 bytes)
IDENTICAL  assemble wt-sim ( 192375296 bytes)
IDENTICAL  assemble wt-app-ios ( 194154600 bytes)
IDENTICAL  assemble tag-sdk-sub ( 25668872 bytes)         --host load-ios --modules sdk subsdk0 --tag T   (order and tag)
IDENTICAL  assemble main-only ( 173694984 bytes)          --host wt-ios --modules main
IDENTICAL  assemble syn-asm-sim ( 3026632 bytes)          --synthetic
IDENTICAL  assemble syn-tag-wt-sim ( 4532728 bytes)       --synthetic --tag G
IDENTICAL  assemble dev-4762784 vs the framework stored by the original pipeline (out-load/device)
```

The CLI's stdout (`k=v` and `segment` lines) was also diffed against Python's for every assemble case: no differences (except `path=`).
`fix_tlv`: a host copy with `0x7fec0000` added to the 16 TLV offsets of `__DATA,__thread_vars` was fixed by Python and by `ibuilder-fix-tlv`:
identical files, equal to the natural link; second run prints `already correct`; with a mismatching natural host both refuse with the same
message (`__thread_vars sizes differ`) and leave the file untouched.

**Tests:** `cargo test --release -p ibuilder-macho`: **31 unit + 16 integration tests, all pass** (host). Unit tests cover: `align`/`uleb`/`fixed`,
`LC_SEGMENT_64` byte layout and parse round trip, load-command sizes/padding, export-trie lookups through a dyld-style decoder, arm64 ADRP/ADD/LDR/RET
encodings, the probe's page alignment and header slack, UUID being the only non-determinism, the `fixups.tsv` header and layout rules (FAR / normal /
back patch area), `abspath`, argparse-like options, and a synthetic miniature host (built with the writer primitives) assembled in memory and
checked structurally: every `__LINKEDIT`-pointing offset re-pointed (+8 past the grown fixups blob), `seg_count` and the `seg_info_offset` array
remapped by segment name, 16K alignment, `SG_READ_ONLY` preserved, `__bss` offset kept 0, data and padding bytes, and the error paths
(not a Mach-O, section name > 16 bytes, header slack, writable segment not high). The integration tests (`tests/oracle.rs`) assemble the 11 cases above and
compare size and **SHA-256 with the Python output** (hashes recorded in the test; additionally a streaming byte compare naming the first
differing offset when the Python file is present), compare with the stored device framework, check `macho_write` (sha256 of three Python runs),
check `gen_embed` text against fixtures in `tests/fixtures`, check that output == input is refused and a failing plan creates no file, and
assert peak RSS < 96 MiB. A missing/changed fixture makes a case print `SKIP ... : reason` and pass.

**Structure (macOS tools), on the Rust output** of the newest device host (`wt-ios`, 194,600,976 B) and the sim host (`asm-sim`):

```
otool -hv      MH_MAGIC_64 ARM64 ALL DYLIB, ncmds 44, sizeofcmds 7120, NOUNDEFS DYLDLINK TWOLEVEL ... MH_HAS_TLV_DESCRIPTORS
vtool -show-build   platform IOS, minos 26.0, sdk 27.2
otool -l       7 segments: __TEXT (20 sections: 14 host + __dmmainx0 0x7384000 / subsdk0x0 0x360000 / sdkx0 0x690000 / x1 0x148000 0x60000 0xa0000),
               __DMmainW/__DMsubsdk0W/__DMsdkW (rw, 0x2a80000/0x310000/0x5f0000, align 2^14), __DATA_CONST, __DATA, __LINKEDIT; rc 0
codesign --force --sign - (copy)   rc 0;  codesign --verify --strict --verbose=2:  "valid on disk", "satisfies its Designated Requirement"
               CodeDirectory v=20400 flags=0x2(adhoc) hashes=11878+2; the signed sim framework is 191,170,512 B = the size of the stored out-asm DmGameProbe
```

(`asm-sim`'s host carries a stale linker signature; `assemble.py` re-points its `LC_CODE_SIGNATURE` like any `__LINKEDIT` command and `codesign` prints
"replacing existing signature". Same in Python.) The probe dylibs (`macho_write`): `otool -hv` 13 commands, `vtool` platform IOSSIMULATOR 27.0 / IOS 26.1,
`dyld_info -exports` lists `_dm_probe_entry` 0x590 and `_dmgame_image_marker` 0x4000, `codesign` verify rc 0, `otool -l` text identical to Python's.
Not done: `dlopen` (an iOS image cannot load on macOS); see 8.

## 6. Numbers (indicative: shared Mac, `/usr/bin/time -l`, same inputs, output on the same disk)

Newest device host (`wt-ios`), 3 modules, 194,600,976 B output, 5 runs each:

| | wall (first, cold cache / warm) | max RSS | peak memory footprint |
|---|---|---|---|
| Python 3.14 `assemble.py` | 0.41 s / **0.20-0.21 s** | **767 MiB** | 760 MiB |
| Rust `ibuilder-assemble` | 0.13 s / **0.05-0.06 s** | **2.8-2.9 MiB** | 2.3 MiB |

About 4x faster warm and about 270x less memory. (First Python run on `asm-sim`: 0.43 s, 754 MiB max RSS / 747 MiB footprint; on `load-ios`: 0.26 s, 761 MiB.) The phone's Python figure from the
brief (0.2-0.5 s, 2.8 GiB footprint at this step) is larger than the Mac's; not re-measured here and the cause is not investigated. By construction the footprint does not grow with the game (one 1 MiB buffer plus the header and fixups blob); measured only up to 195 MB. Inside the full test process (harness, SHA-256 buffers, several tests at once) peak RSS is 25 MiB on the host and 20 MiB in the simulator.

## 7. iOS portability

* `cargo build --release --target aarch64-apple-ios -p ibuilder-macho` and `--target aarch64-apple-ios-sim`: both build (lib + 4 CLIs), with
  `PATH=/opt/homebrew/opt/rustup/bin:$PATH` prefixed for the command only. Library: no `unsafe`, no `fork`/`exec`/`Command`, std only, positioned I/O
  through `std::os::unix::fs::FileExt` / `MetadataExt` (both exist on iOS).
* **Run in the iOS simulator** (`xcrun simctl spawn <SIMULATOR-UDID>`, the already-booted iPhone 17; the sim shares the host filesystem):
  the **whole test suite runs inside the simulator** via `CARGO_TARGET_AARCH64_APPLE_IOS_SIM_RUNNER="xcrun simctl spawn <udid>"`: **31 unit + 16
  integration tests pass**, including the byte-for-byte assemblies. The sim-built `ibuilder-assemble` (platform IOSSIMULATOR) run through `simctl spawn`
  on the `wt-ios` and `dev-4762784` hosts produced files **identical to the host run and to the Python oracle** (`cmp`), and `ibuilder-macho-write` likewise.
  Wall 0.25 s / 0.56 s including `xcrun simctl spawn` start-up (the Mac's CPU: indicative only); the simulator child's own RSS was not measured
  (`time -l` sees `simctl`), only the test process's 20 MiB peak.
* Not run: on a physical iPhone (none attached); the device build was only compiled and linked.

## 8. Differences from Python, gaps, what is not verified

**Behaviour mirrored deliberately (so the bytes match):** `assemble.py`'s asserts (not `gen_embed.py`'s stricter ones: `gen_embed` mode adds `KEY_PAGE_OFF % 16K`
and `size % 16K` with a back area, used only by the `.S` generator); a back patch area (`BACK_LEN`, the `x2` piece) is computed but `assemble.py` never
emits it, so neither does this (a game with `BACK_LEN` would hit the loader's "embedded sections missing"; same gap as Python); the host's segments other
than `__TEXT`/`__DATA_CONST`/`__DATA`/`__LINKEDIT` are dropped (the fixups-blob remap errors if a dropped one has chains); the host's own `LC_CODE_SIGNATURE`
(stale in simulator hosts) is re-pointed, not removed; later duplicate segment names win; `fixups.tsv` lines each need exactly two tokens after
dropping two characters; `int(x, 16)` syntax. `LC_UUID` of `macho_write` is random in Python and here unless pinned.

**Where Rust is stricter or differs (error paths only, never output bytes):** Python's `assert`/`KeyError`/`struct.error` tracebacks are `Error` values with a
message (CLI: `ibuilder-assemble: error: ...`, exit 1; usage errors exit 2 like argparse); the chained-fixups blob must lie inside `__LINKEDIT`
(Python would silently build garbage); a 32-bit field overflow (> 4 GiB file) is an error; negative `BACK_LEN` is refused; output == input is refused;
a failed write deletes the partial file; the NRO table line without a tab is accepted (Python keeps the newline in the name and then fails to open the
file). argparse prefix matching (`--mod`) and `--opt=value` are supported; other argparse features are not.

**Not verified / not done:** (1) no `dlopen`/boot of the *Rust-assembled* framework by the engine loader (`loader_probe_host`, `PROBE_MODE=boot`) and nothing
on a device: the claim is "byte-identical to what Python produced, which `docs/ASSEMBLER.md` records as booting to `nnMain` in the simulator"; I did not re-run
that boot. (2) Only the Grizzy modules (one FAR-layout main + two normal-layout modules, no NROs, no back patch area, three modules) were used as game
input; other layouts are covered by unit tests and by the `gen_embed` fixtures, not by a Python-compared assembly. (3) Hosts other than the six listed
(and a host whose segments are not in the prebuild's shape) were not tried. (4) `memmap2` variant not timed (section 3). (5) The prebuild itself
(`assemble_prebuild.sh`, the Xcode link) is not ported and was not run.

**For the orchestrator:** (a) the repo workspace (`members = ["crates/*"]`) did not load while I worked because other contributors' crates were
half-scaffolded (`ibuilder-pipeline` manifest "no targets specified", earlier `ibuilder-profile` without a `Cargo.toml`), so `cargo test -p ibuilder-macho` from
the root failed for reasons outside this crate; I built and tested through a private scratch workspace (`$SCRATCH_OLD/macho/ws`, symlinks to this crate and the
testkit, same package/profile settings), which is what every result above used. Re-run from the root once the other crates load. (b) `ibuilder-testkit::peak_rss_bytes`
multiplies `ru_maxrss` by 1024 on every non-macOS target, but iOS reports bytes like macOS: on iOS it returns 1024x too much (it printed 10 GiB for a 10 MiB
process in the simulator). Not mine, not changed; `tests/oracle.rs` divides it back on iOS. Fix: `cfg!(any(target_os = "macos", target_os = "ios"))`.

## 9. Files

```
crates/ibuilder-macho/Cargo.toml                       no runtime dependencies; dev: sha2, ibuilder-testkit, libc
crates/ibuilder-macho/src/{lib,error,io,macho,embed,assemble,writer,tlv,cli}.rs
crates/ibuilder-macho/src/bin/{ibuilder-assemble,ibuilder-macho-write,ibuilder-fix-tlv,ibuilder-gen-embed}.rs
crates/ibuilder-macho/tests/oracle.rs                  Python-oracle integration tests (hashes, stored framework, fixtures)
crates/ibuilder-macho/tests/fixtures/ge-*.S.tmpl       gen_embed.py outputs with the directory replaced by @DIR@ (no game data)
scripts/macho-oracle.sh                                regenerate the Python oracle (scratch folder copies, nothing written to iOSBuilder)
scripts/macho-verify.sh                                build + byte-compare every CLI output with the Python oracle
```
