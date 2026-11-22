# Shader split (`ibuilder-shadersplit`)

Step 1 of stage 3: read the game's files and cut out every shader program with its control section. Overview of the whole stage: [shaders.md](shaders.md).

Crate `crates/ibuilder-shadersplit`: a library (`#![forbid(unsafe_code)]`, no process is spawned, builds for `aarch64-apple-ios` and `-ios-sim`) and the Mac CLI `shadersplit`.

## What it ports

The Python original is the engine's `shader_split.py` (plus its Unity reader, the RomFS image reader, the NSO reader and the game profile) and the phone-side wrapper around it (job memo, live package check).

| Python | Rust (`src/`) |
|---|---|
| `main` (version loop, key merge, pair detection, record / map / facts / pair writers) | `split.rs`, `scan_types.rs` |
| control-section helpers (`skey`, `same_shader`, `check`, stage detection, record cut, compute facts) | `ctl.rs` |
| `scan_file*`, `scan_blob`, key and pair helpers | `scan.rs` |
| Unity bundles and serialized files, type trees (16 built-in layouts, generated from the Python module by `tools/gen_known_trees.py`) | `unity.rs`, `unity_trees.rs` |
| RomFS image reader, game profile | `image.rs`, `game.rs` |
| GLSLC, BNSH, BAHS and the other NVN container readers | `glslc.rs`, `nvn.rs` |
| archive and compression formats (Yaz0, SARC, gzip, zstd, zip, game-specific paks) | `archive.rs`, `inflate.rs`, `zlibx.rs`, `zstd.rs`, `kit.rs` |
| Vector Unit APF packages (`apf_entries`, `apf_decompress`, `apf_shaders`: magic `FPUV`, any file name) | `apf.rs` |
| Unreal Engine paks (`uepak.py`: footer, index, blocks; `ue_legacy_jobs`, `ue_inline_shaders`; the shader code archives `uepak.py job` / `finish` make on the Mac, `ue_shaders`) | `uepak.rs` |
| Nihon Falcom `.pkg` / `.pka` (`falcom_pkg`, `falcom_pka_index`, `falcom_pka_jobs`) | `falcom.rs` |
| Media.Vision MVGL archives (`mvgl.py`, `mvgl_jobs`) and NXVS / NXFS / NXCS containers (`nxsh_shader`) | `mvgl.rs` |
| NXSD containers (`nxsd_shaders`) | `nxsd.rs` |
| archives that get jobs of their own | `jobs.rs` |
| job memo and thread pool | `memo.rs`, `split.rs` |
| NVN2 ray-tracing module check | `rt.rs` |
| live shader package check | `live.rs` |

New in Rust: `spill.rs` keeps the merged shaders' bytes in a temporary file instead of a map in memory; `record.rs` records which readers a split needed (the targeted re-split, below).

The port follows the engine the app ships (nxnative a3663659 + the NX2Apple patches) and, since 2026-11-22, these later upstream commits: the Unreal pak reader (`uepak.py` as of 184847c5, with 22222648 and b3e0d8c5), 930e9195 (Falcom, Phyre RYHP GLSLC), 131bd013 + 8cc63cc9 (MVGL / NXSH), a63ca651 (NXSD), c9b12271 + f41613f7 (registered sources), and e74c3fa7's check of the Snowdrop bare zlib blocks. Other upstream readers added after a3663659 are not ported.

## Input and output

Input: the game's RomFS (a `DMROMFS1` image or a folder), the `main` NSO, optional update and DLC roots, the profile folder, the title id.

Output, byte-identical to the Python's:
- `pkg/{ps,vs,cs,gs}_{data,main}.bin`: `BTCDBTCD` records (key, code area, control section). Other stages and ray-tracing modules get their own package files (`rt_<group>.bin`).
- `tables/{shader_map,shader_facts,shader_pairs,pso_pairs}.tsv`.

## API and CLI

```rust
let mut o = SplitOptions::single(romfs, main_nso, pkg_dir, tables_dir, profile_dir, game_id);
o.versions.push((update_romfs, update_main));   // optional: output is the union
o.dlc = vec![aoc_1_img];                        // optional
o.jobs = 6; o.memo_dir = Some(&work.join("splitjobs"));
let stats = ibuilder_shadersplit::split(&o, &Ruzstd, &gate, &|line| log.line(line))?;
```

- `split_version()` returns `rs<10 hex>` from `SPLIT_REV`, the GLOBAL revision (and the crate version 0.1.0 it started with, frozen: a release of the crate does not re-split every game). Bump `SPLIT_REV` only for a change that can alter every game's output (the merge, the record cut, the tables); a change to one reader bumps that reader's revision in `record::READERS`.
- `o.record = Some(path)` writes the split's reader record there (see below); `o.registered = Some(map)` is `DM_SPLIT_REGISTERED_MAP` (below). Both default to `None`.
- zstd is a trait (`Zstd`); the CLI and tests use the pure-Rust `Ruzstd`, the app may pass its own callback.
- `JobGate` lets the caller throttle each job (the pipeline's memory/thermal gate).
- `live::package` checks entries captured during play (same refusal texts as the Python).

```
cargo run --release -p ibuilder-shadersplit --bin shadersplit -- \
  --romfs RomFS.img --main ExeFS/main --pkg out/pkg --tables out/tables \
  --profile <profile dir> --game <title id> [--jobs 8] [--memo <dir>] [--dlc a.img,b.img] \
  [--update-romfs R --update-main M] [--registered shader_map.tsv] [--record split.record]
```

## Design points

- **Order is part of the output.** Results are merged in the Python's job order (main, archive jobs, plain-file batches, deferred Unity files); the first copy of a key wins. The output is the same for any thread count and for a resumed memo.
- **Files are not read whole.** A file is a `Source` (positioned reads, no memory map). Large files with no recognised magic are only searched in 1 MiB windows; Unity serialized files are read through their tables and the objects of the wanted classes.
- **Resume.** Each finished job is written to `<memo_dir>/<digest>.job` (temp file + rename, own format `SSMEMO01`); damaged files are deleted and the job runs again.
- **Unity decoder budget.** One decode may produce at most 8 values per byte of the object plus 4,096; past that the object is reported as a `[RED]` error (the Python would run out of memory). Real layouts stay far below.
- **Unsupported containers fail loudly.** A container the Python reads and this port does not ends the split with `unsupported container: <name> (<file>)`. A damaged file is left out with a `[RED]` log line and counted in `SplitStats::unreadable`.
- **In-process zstd.** The port decodes `.zs` files itself; the original's newer code shells out to a `zstd` command, which does not exist on a phone.
- Allocator: the `shadersplit` binary (and the `ibuilder` CLI) run with `ibuilder-alloc`, which returns big blocks to the OS. The library does not install it; the host must.

## Verification

```
cargo test --release -p ibuilder-shadersplit
```

| suite | what it checks |
|---|---|
| `tests/grizzy.rs` | full split of a real game against the Python's output (11 files equal) for 1, 2 and 8 threads and with a memo fresh / complete / partly damaged. Skips without the game files (`SHADERSPLIT_ROMFS`, `SHADERSPLIT_MAIN`). |
| `tests/differential.rs` | 3,312 vectors made with the engine's own functions: control sections, keys, stages, containers, Yaz0 / SARC, the Unity type-tree interpreter, Unity files and bundles, live package. |
| `tests/games.rs`, `tests/synthetic.rs` | 14 synthetic games (generated, no game data) split by both implementations; output hashes and log lines compared at several thread counts. |
| `tests/rebase.rs`, `tests/snowdrop.rs` | newer engine behaviour: ray-tracing control sections, newer container revisions, Snowdrop zlib caches (including 2,400 zlib streams against zlib itself and 1,057 mutated slices). |
| `tests/uepak.rs`, games `ue-v8-kakarot`, `ue-v8short-v9`, `ue-v11` | Unreal paks (below): 3 synthetic games split by upstream's Python (`tools/gen_uepak.py`: `uepak.py job` / `finish`, then `shader_split.py`) and by the port, byte-identical; 497 damaged-pak vectors (`tools/gen_uepak_vectors.py`, `vectors-uepak.jsonl.gz`). |
| `tests/sync_readers.rs`, games `sync-nxsd`, `sync-falcom`, `sync-mvgl` | NXSD, Falcom, Phyre RYHP GLSLC, MVGL / NXSH: synthetic games split by upstream's `shader_split.py` (`tools/gen_sync_readers.py`) and by the port, byte-identical; damaged containers never panic and never allocate from a size they claim. |
| game `sync-registered` | the registered shader map (`tools/gen_registered.py`, upstream's Python with `DM_SPLIT_REGISTERED_MAP` and a DLC folder). |
| `tests/record.rs` | the reader record and the pipeline's verdict with this build's reader table on every synthetic game (which ones a library without a record would re-split); `--ignored`: the same on a real game (`RECORD_ROMFS`, `RECORD_MAP`). |
| `tests/apf.rs` | a synthetic Vector Unit APF package (`FPUV` v7: header and directory FNV-1a sums, entries stored, zlib and LZ4 block, an unknown compression) split end to end, and ten damaged packages, each left out with its `[RED]` line. |

Measured on 2026-11-19 on an M1 Pro (shared machine, indicative), one real game with 21,990 shaders and a 2.5 GB RomFS: Python 6 threads about 19 s wall and 2 GB resident; Rust 6 threads 0.9 - 1.4 s wall, peak footprint 160 MiB. The iOS simulator run produced identical output; nothing was measured on a device.

## Vector Unit APF packages

Beach Buggy Racing 2: Island Adventure (`0100C43012344000`) keeps every asset in one `RomFS/Assets.apf` (343 MB, 7,749 entries). Read by its magic, whatever its name (`apf.rs`):

- Header, 0x3c bytes, little-endian: `FPUV`, u32 version (7), u32 directory offset, u32 entry count, u32 directory length, u32 FNV-1a 32 of the directory, the platform's name (`Switch`, 32 bytes NUL-padded), u32 FNV-1a of the 0x38 bytes before it. The entries' data follows; the directory is last.
- Directory entry: the asset's name (`<asset type>/<asset path>`, NUL-terminated), u32 offset, u32 length, u32 stored length, u32 FNV-1a of the decoded data, u16 the asset type's version, u16 compression (0 stored, 1 zlib, 5 one LZ4 block). `Assets/AssetData` (zlib) lists the assets by type, without offsets.
- Every entry is decoded and scanned like a file (`scan_blob`, then whole GLSLC outputs), one at a time; the package is never read whole. An unknown compression is counted in one `[WARN]` line per package. The two sums of the header and directory are checked; the entries' sums are not (the Python would hash 750 MB byte by byte).
- The shaders are the 233 `VuShaderAsset` entries (LZ4): a 0x18-byte header, then GLSLC outputs of a vertex and a pixel program each. Measured 2026-11-22: 1,088 programs, 496 unique keys (229 vertex, 267 pixel), 410 variation pairs; before, 0 (the raw bytes hold the GLSLC magics, but LZ4-compressed). The same reader in the engine's `shader_split.py` (a change proposed upstream, not merged when this was written) gives byte-identical packages and table rows on this game.

## Known limits

- **Only one real game was available.** Many container readers (see the list in the source) are ported by reading and checked only against synthetic files built to the layouts the Python expects.
- Refused with `unsupported container`: XNB with LZX compression, Next Level Games `.dict` + `.data`, Unreal Engine IoStore containers (`.utoc`), Unreal paks with an encrypted index, Unreal shader code archives with Oodle shaders, Oodle-compressed entries elsewhere (Kraken, Mermaid, Leviathan), zip64 / bzip2 / LZMA zip members and encrypted members, Game Freak Trinity archives, containers whose readers live in other engine modules (CPK, CASC, LSPK, RPF6, Frostbite, and similar), and NSO files with ZBIC compression.
- libzstd and `ruzstd` differ on a few damaged frames; one of 1,057 mutated slices differs in how it ends.
- A BERG data slice is read whole (as in the Python).
- The split's output is only as current as the Python it follows; an engine change to `shader_split.py` needs the vector generators in `tools/` re-run and `SPLIT_REV` (or the reader's revision in `record.rs`) bumped.
- Upstream's newer `shader_map.tsv` comment line 2 ("names the chosen source of each key") is not taken: the port keeps "records where each key first appears", so the libraries' tables do not change; the generators record the Python's hash with the port's line.
- No device run: jetsam limits and the memory gate with this crate are untested.

## Unreal Engine paks

DRAGON BALL Z: KAKAROT (`010051C0134F8000`, issue #14) keeps its files in version 8 paks (`AT/Content/Paks/pakchunk0-Switch.pak`, Zlib blocks, index not encrypted); upstream extracted 43,113 shaders from 3,776 of its files. Port of `uepak.py` (`uepak.rs`):

- A pak is a `.pak` file whose footer magic `0x5A6F12E1` sits where its version puts it (KPKA and LSPK `.pak` files keep their own readers). Versions 4, 8 (ordinary: 5 method names; short: 4 names, a one-byte method, 50-byte entry headers), 9 (serialized or frozen index), 11 and 12 (encoded entries, full directory index; a single-block entry's block size is its raw size, b3e0d8c5).
- Blocks: stored, Zlib (bounded to the entry's raw size), Zstd (frames with or without a content size, bounded, 22222648), LZ4 (255:1 checked before any allocation). Any other method (Oodle, LZ5): the Python's text when the Mac host is missing (`Oodle- or LZ5-compressed game files: set DM_OODLE_HOST ...`), for that package only: it is left out with its `[RED]` line, the split goes on.
- Paks whose shaders are inline (versions 4 and ordinary 8, Kakarot; version 11 without shader code archives; the inline half of short 8 / 9): `ue_legacy_jobs`: one view of a folder's paks (a `_P.pak` patch replaces the same path), about 64 jobs by raw size, planned after the KPKA jobs. Each package is decoded and scanned on its own (`ue_inline_shaders`: zlib streams with their length in front, stored LSLG records, LZ4 shader-map blocks, `FShaderMapResourceCode` tables with stored first rows); a package that fails costs only itself, at most 8 `[RED]` lines per job. Labels `/<pak>#<path>#UE<offset>`.
- Shader code archives (`.ushaderbytecode`, version 1, and 2 with stored or LZ4 shaders) are read from the paks in the split (upstream decodes them on the Mac first, `uepak.py job` / `finish`) and appended to that version's `main` shaders as `/ue_shaders/<name>#UE<n>`; a `ue_shaders` folder beside the RomFS is still read the Python way.
- Refused (`unsupported container`): a pak with an encrypted index (the AES key in `main` is not searched for), an archive with Oodle-compressed shaders, IoStore containers (`.utoc`).
- Not verified: no real Unreal game here; every file of Kakarot's 6.4 GB of paks is decoded and scanned (as upstream): time and memory on the phone are unmeasured. Error texts of damaged data differ from the Python's in the decoders' own words only.

## Falcom, MVGL, NXSD, Phyre

- **NXSD** (TETRIS THE GRAND MASTER 4, a63ca651): a file starting with `NXSD`, whatever its name (checked before the Next Level Games `.dict` rule), version 0x100: descriptors to 0x880-byte control sections and code areas that must tile the code region. Labels `#NXSD@<i>`, single stage.
- **Falcom** (Trails into Reverie, 930e9195): `.pka` archives (magic `0dcff77f`) in about 64 jobs by stored size, after the KIT jobs, never read whole, each blob read once; headerless `.pkg` packages recognised by their table. Entries stored (0), raw LZ4 blocks (1, 255:1 check) or one zstd frame (0x10); `.dds.phyre`, `.dae.phyre` and `.xml` are not read; other flags: one `[WARN]` line per package. Each entry goes through `scan_blob`, then whole GLSLC outputs.
- **Phyre** (930e9195): an `RYHP` container in which `phyre_shaders` finds no program but that holds GLSLC outputs gives those outputs.
- **MVGL** (Digimon Story, 131bd013 + 8cc63cc9): files starting with `MDB1`, wide (64-bit, LZ4) or legacy (32-bit, doboz) layout, planned after the PKZ jobs; only `.shad` entries are decoded and scanned. NXVS / NXFS / NXCS (/ NXGS / NXHS / NXDS) containers are read at the start of a blob (strict) and found inside any blob, `main` included (only a fully valid container counts). Labels `#NXSH@<offset>`, single stage.
- Decoders are capped at 1 GiB where the Python has only its memory. Not verified on real data (no Falcom, Digimon or Tetris files here).

## Registered sources

`DM_SPLIT_REGISTERED_MAP` (c9b12271 + f41613f7, `SplitOptions::registered`, `--registered`): when a shader is found in several places, the copy at the source a registered `shader_map.tsv` names (same key and stage) is kept instead of the first, in `main` and across files, once every copy is checked; the game's copy still wins over an add-on's, and the add-on side never wins. The rows per source group of the summary line are counted over the final keys. The phone does not pass a registered map (its library is built from its own split's tables), so its output does not change.

## Snowdrop bare zlib caches (e74c3fa7)

South Park: The Fractured But Whole stores its shader caches as bare zlib streams (`#SDFZLIB@`). Since e74c3fa7 every stream of a cache must decode to exactly 64 KB, the last one to the rest (`... block @0x.. did not finish at <n> raw bytes`); a cache in streams of other sizes, which the older reader took, is refused. The 1 GiB cap stays (checked before each block). `vectors-snowdrop.jsonl.gz` was regenerated with upstream's Python: 290 of its 1,057 damaged slices now end with the new text; no valid slice changed.

## Targeted re-split (`record.rs`)

A library built on the phone records the split's version in `pack.txt` (`split`). Before 2026-11-22 any change of it removed every game's library. Now:

- `SPLIT_REV` (the `split` digest) is the global revision: when it differs, the library is split again.
- Each reader family has a revision (`record::READERS`): apf, falcom, generic, ipk, kit, kpka, mvgl, nxsd, phyre, snowdrop, ue3, unity, unreal-pak, zip (`generic` is everything else: the blind scan, BNSH, BAHS, GLSLC, SARC, zstd ...).
- The split writes a record (`SplitOptions::record`; the app's backend writes `<work>/split.record`), which stage 3 keeps in `pack.txt` or the no-shader marker: `split-revs` (every reader's revision), `split-uses` (the readers the game needed: a top-level file the reader recognises by its magic, layout or name, or a shader label with the reader's tag), `split-unknown` (the files no reader recognised, as `<extension>#<first four bytes in hex>`, at most 8 per extension, then `<extension>#*`).
- A library is split again only when a reader it uses has another revision now, or when a reader whose revision moved claims one of its unknown kinds (`Reader::claims`: an extension or a magic prefix). Otherwise its record takes this app's revisions and it is kept.
- A library from before records existed is taken to have the revisions every reader had at `SPLIT_REV` 3 (`record::LEGACY`), and its game's files are classified again once (`record::probe`, about 3 s for Ocarina of Time's 186,583 files on an M1 Pro), with the labels of its shader map; the answer is written into `pack.txt`.
- This change: new readers falcom, mvgl, nxsd, unreal-pak (revision 1), phyre 2, snowdrop 2. Measured: Ocarina of Time (`0400486024DEC000`) and Beach Buggy Racing 2 keep their libraries; a game with Snowdrop data slices (South Park, Kingdom Battle), Phyre files or KIT archives, or files of the new readers is split again.
- Limits: a container inside another one (a SARC in a zstd file) is seen only through the labels of the shaders it gave; a reader that may read files it does not recognise at the top must list them in its claims, or its change must bump `SPLIT_REV`. A memoised job and an interrupted work folder are tied to every reader's revision.
