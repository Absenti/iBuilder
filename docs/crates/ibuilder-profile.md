# ibuilder-profile: deriving the per-game profile from the dump

Port of `iOSBuilder/gameapp/profile_derive.py` (212 lines) and `gameapp/compare_profile.py` (68 lines); the main checkout and the
worktree `$IOSBUILDER_WORKTREE` copies are byte-identical (`diff -q`), either was ported. Docs: `iOSBuilder/docs/PROFILE-DERIVED.md`.
Crate: `crates/ibuilder-profile` (`#![forbid(unsafe_code)]`, deps: `sha2`, `rayon`, `lz4_flex`; no process is started; no Python).

`profile_derive.py` is a thin driver: it runs the **engine's** Python in-process (`runpy`). So porting it means porting what it runs:
`profile_guess.facts_of` + `guess` (the rules), `nso.load`, `nso_syms`, `nacp`, `romfs` (image reader) and `game.py profile` (rendering).
All of that is ported here, so the iPhone app needs neither CPython nor the 651 MB `dm/games` tree (only the registered `game.tsv` files
and the host sources / hook list).

## Source -> module

| Python | Rust |
|---|---|
| `gameapp/profile_derive.py`: `derive`, `title_id`, `write_provisional_profile`, `ipa_name`, `EMPTY_TABLES`, `render` | `src/derive.rs` |
| `dm/games/profile_guess.py`: `facts_of`, `kind`, `npdm`, `MARKERS`, `MAGICS`, the byte/regex scans | `src/facts.rs`, `src/search.rs` |
| `profile_guess.py`: `guess`, `engine`, `load_order`, CLI text | `src/guess.rs` (`guess`, `report`) |
| `dm/tools/nso.py`: `load` (LZ4, SHA-256 hash check, all validations) | `src/nso.rs` |
| `dm/tools/nso_syms.py`: `dynsyms`, `area`, `host_hooks` | `src/syms.rs` (`HookSet::from_host_dir`, `from_names`, `parse_list`) |
| `dm/tools/nacp.py`: `name`, `version`, `titles` (+ raw-deflate title area) | `src/nacp.rs` (own small inflate, no `flate2`) |
| `dm/tools/romfs.py` (DMROMFS1 image, validations) + `romfs_files` (manifest / image / folder) | `src/romfs.rs` |
| `dm/games/game.py`: `Game`, `header_one`, `profile` subcommand | `src/game.rs` (`Game`, `header_macros`, `render_profile`, `write_profile`) |
| `gameapp/compare_profile.py` | `src/compare.rs` |
| `__main__` of both + `profile_guess.py` CLI + `game.py profile` | `src/bin/ibuilder-profile.rs`: `derive`, `compare`, `facts`, `render`, `hooks` |

Python semantics that had to be reproduced to stay byte-identical: `int(s, 0)`, `{:#x}`, floor division in the heap size, `Counter.most_common`
(stable, insertion order), `str.split()/strip()/splitlines()`, the macro round-trip of `game.py profile` (the name keeps its `\"` escaping because
`val()` strips quotes but not backslashes), `os.path.splitext`, `\w`/`\b`/`(?i)` regex semantics on bytes (hand-written scanners, `src/search.rs`),
`Path.resolve()` in the provenance line of `profile.tsv`.

Performance changes that do not alter results: modules are loaded one at a time from positional reads (compressed segments are never all in memory)
and scanned in parallel; the 15 marker literals and the other byte scans of the 75 MB `main` run in parallel and use a rarest-byte, auto-vectorized search
(`search.rs`) instead of a byte-at-a-time loop.

## How to run

```
cargo build --release -p ibuilder-profile
# == GAMEAPP_BUNDLE=<engine> python3 gameapp/profile_derive.py <extract> <out profile.tsv> [<profile_out>/<title ID>]
ibuilder-profile derive <extract dir> [out profile.tsv] [profile_out dir] --bundle <engine> [--romfs <RomFS.img|romfs_manifest.tsv|dir>] [--hooks-file hooks.txt]
ibuilder-profile compare <app game.tsv> <orchestrator game.tsv> [<app profile.tsv> <harness profile.tsv>]    # == compare_profile.py
ibuilder-profile facts <ExeFS> <romfs> --bundle <engine>                                                      # == profile_guess.py CLI
ibuilder-profile render <game dir | title ID> <out profile.tsv> --bundle <engine>                            # == game.py profile
ibuilder-profile hooks --bundle <engine> > hooks.txt                                                         # the HLE hook names, one per line
```
`<engine>` is the engine layout: `<engine>/dm/games/<id>/game.tsv` and, for a provisional profile, `<engine>/dm/host/*.c`, `dm/gfx/{*.c,nvn/*.c,vk/*.m}`
(what `nso_syms.host_hooks` reads). On the phone, ship `hooks.txt` (199 names) and pass `--hooks-file` / `DeriveConfig::hooks` instead of the 2.4 MB of C sources.
Library entry: `ibuilder_profile::derive::{derive, DeriveConfig}`, `facts_of`, `guess`, `game::render_profile`.

Inputs for the reference game: `out/ref-img-grizzy/{ExeFS/,RomFS.img,control.nacp}` (the app's shape) or `out/ref-out-grizzy/{ExeFS/,control.nacp,romfs_manifest.tsv}` (survey shape).

## Verification (commands and results, this Mac, 2026-11-19)

`cargo test --release -p ibuilder-profile`: 17 lib tests, 6 unit tests (`tests/unit.rs`: hand-built NSOs incl. every refusal, facts and rules on a
tiny game, the provisional profile writer and its reuse, registered path, NACP, RomFS image), 7 oracle tests (`tests/oracle.rs`, ~13 s): all pass.
The Python oracle is run from the scratch folder copies (`ref/venv` Python 3.14, `-B`), engine bundle = `ref/engine` (+ a copy of `dm/host`, `dm/gfx`):

1. **Grizzy, derived two ways, byte-identical to the Python**: `game.tsv`, the six empty tables (`bf_want`, `pso_manifest`, `shader_facts`, `shader_map`,
   `shader_map.tsv.conflicts`, `shader_pairs`) and `profile.tsv` (40 rows) for `out/ref-img-grizzy` (image) and for the manifest `out/ref-out-grizzy/romfs_manifest.tsv`.
2. **Against the stored 2026-11-17 Python output** (`out-prof/derived-game.tsv`, `derived-profile.tsv`): equal after the provenance lines.
3. **Against the Mac orchestrator's profile** (`out-prof/harness-profile.tsv`): `compare` reports `fields equal 37, different 0, not derivable 0, extra 0`
   for `profile.tsv` (the "37 data rows identical" of `docs/PROFILE-DERIVED.md` section 3), for both RomFS shapes.
4. `facts` prints the `profile_guess.py <ExeFS> <manifest>` text byte for byte (modules, `SDK 21, system resource 16 MB, GLSLC outputs in main 11, ...`, guessed keys).
5. **All 179 registered games** (`dm/games/*/game.tsv` of the engine, via a fake `RomFS.img.id` extraction): `profile.tsv` byte-identical for 179/179, through `derive()`
   and through `render_profile`.
6. **Synthetic variants** (`tests/oracle/gen_variants.py`: random ExeFS with NSOs that exercise sysres / heap / giant bss / Switch 2 pool / dropped subsdks /
   GPU-driver modules / Unreal / RE Engine / Hagi rules, random RomFS manifests with name collisions and ties, plain and deflated NACP titles, `RomFS.img.id`
   JSON): 57 of 60 variants byte-identical in `profile.tsv`, `game.tsv` and the six tables, the other 3 refused by both (no title ID). A larger run of 300 variants
   by hand (same generator): 289 identical, 0 different, 11 refused by both.
7. `compare` prints `compare_profile.py`'s text byte for byte on three comparisons.
8. `HookSet::from_host_dir` equals `nso_syms.host_hooks` for both `forced=False` (199 names) and `forced=True` (1,324 names) on the engine's `dm/host` + `dm/gfx`.

The `18/18 fields, 1 not derivable` claim of `docs/PROFILE-DERIVED.md` section 3 is against the orchestrator's `game.tsv`
(`.../0100DFA027504000_build/profile/0100DFA027504000/game.tsv`), which **does not exist on this machine any more** (and the engine has no
`dm/games/0100DFA027504000/`), so it was not re-run. What is verified instead: byte identity with the Python's derivation (items 1-2) and the 37
runtime rows against the orchestrator's rendered profile (item 3). `depth_mode` (written by the Mac-side region check) is, as documented, not derivable
and is not emitted.

## Numbers (indicative: shared Mac, `/usr/bin/time -l`, median of 5)

| `derive` on Grizzy (75 MB main, RomFS.img, NACP) | wall | max RSS |
|---|---|---|
| `python gameapp/profile_derive.py` | 3.99 s | 798 MiB |
| `ibuilder-profile derive` | 0.30 s | 260 MiB |

Where Rust spends it (`main`, 75,035,192 B): LZ4 + SHA-256 header check ~185 ms, symbols < 1 ms, SDK/component scans ~55 ms, marker + other scans ~100 ms.
The first version used a Horspool search and took 1.1 s for `main`; the anchor-byte search cut the whole command from ~2.0 s to 0.30 s. RSS is dominated by the
`main` image (75 MB) plus the compressed segment buffer; `rtld`, `sdk`, `subsdk0` are processed in parallel.

## iOS portability

Builds for `aarch64-apple-ios` and `aarch64-apple-ios-sim`. Run inside the booted iPhone 17 simulator
(`xcrun simctl spawn <SIMULATOR-UDID> .../ibuilder-profile derive out/ref-img-grizzy ... --bundle <engine>`) the Grizzy `game.tsv` and `profile.tsv`
equal the Python's (`diff` of lines 4+ / data rows). `std::os::unix::fs::FileExt` is the only OS API beyond std. Simulator timings are the Mac's CPU (0.5 s wall from `simctl`).
Not run on a physical iPhone, so the on-phone memory ceiling is not measured.

## Not done / not verified (named)

* **ZBIC NSO segments** (NSO flag bit 7, system 22.0.0+): `nso::load` returns an error naming ZBIC instead of a wrong image (the Python needs the `zbic` package);
  **NRO modules** are not loaded (profile derivation reads ExeFS NSOs only). Grizzy and all synthetic inputs are LZ4/uncompressed NSOs.
* Not ported because `profile_derive.py` never calls them: `main_modules`, `nro_modules`, `modules_supplying`, `shader_sources`, `build_sources`, `heap_signals`,
  `depth_signal`, `region_findings`, `check`; in `game.py`: the multi-program `header()`, `UPSCALER_KEYS` (reads `shader_map.tsv`, which the bundle does not ship) and the
  `BUILD_MODULES` macro's failure modes. None can change `profile.tsv`.
* `kind()` ties: the Python takes `Counter.most_common(1)` over a hash-ordered `set` (arbitrary on a tie, varies per process); this port takes the first in sorted symbol
  order. Only matters for a module with no SDK component, no `glslc` export and two equally frequent `nn::` areas, and only in the "not loaded" message text.
* `str.isprintable()` (NACP names) is approximated without the Unicode category tables: control, whitespace other than space, common format characters, private use
  and noncharacters are non-printable; unassigned code points count as printable. A RomFS **folder** is walked in sorted order (the Python uses file-system order, which only
  affects ties in the shader-group list).
* Invalid UTF-8 in manifest paths / file names is replaced (U+FFFD) instead of surrogate-escaped; a non-file entry in `ExeFS/` is skipped (the Python would raise).
* The host hook scanners (`\bHLE\(\s*"([^"]+)"`, `\bL\((\w+)\)`, `^VKB?\((vk\w+),`) are hand-written; they equal the Python's set on the current engine sources (item 8) but a future
  source layout that the regexes read differently (e.g. a hook name containing `HLE(`) is not covered.
