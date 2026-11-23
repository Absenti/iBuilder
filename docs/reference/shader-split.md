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
| REDengine 3 caches (`rdhs_shaders`, `fzlc_shaders`), RAGE WAD VFP arrays (`noire_wad_shaders`, `noire_vfp_shaders`), NXCX archives (`nxcx_shaders`), Telltale `.ttarch2` (`ttcz_chunks`, `ttarch2_shaders`), GIANTS SHARC (`sharc_shaders`), Virtuos VBF (`vbf.py`) | `redengine.rs`, `rage_wad.rs`, `nxcx.rs`, `telltale.rs`, `sharc.rs`, `vbf.rs` |
| liblzma's raw LZMA1 decoder as `lzma.LZMADecompressor(format=FORMAT_RAW)` runs it (SHARC members without an end marker) | `lzma1.rs` |
| archives that get jobs of their own | `jobs.rs` |
| job memo and thread pool | `memo.rs`, `split.rs` |
| NVN2 ray-tracing module check | `rt.rs` |
| live shader package check | `live.rs` |

New in Rust: `spill.rs` keeps the merged shaders' bytes in a temporary file instead of a map in memory; `record.rs` records which readers a split needed (the targeted re-split, below).

The port follows the engine the app ships (nxnative a3663659 + the NX2Apple patches) and, since 2026-11-22, these later upstream commits: the Unreal pak reader (`uepak.py` as of 184847c5, with 22222648 and b3e0d8c5), 930e9195 (Falcom, Phyre RYHP GLSLC), 131bd013 + 8cc63cc9 (MVGL / NXSH), a63ca651 (NXSD), c9b12271 + f41613f7 (registered sources), and e74c3fa7's check of the Snowdrop bare zlib blocks; since 2026-11-23 every reader of `shader_split.py` at 033793f0 (the audit below), but for the two still refused there.

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

## Frostbite superbundles and TT Games DAT archives

Ported from nxnative (`frostbite.py` 682fdd26; `ttdat.py` 9f834e41 + 5cd38d39), in `frostbite.rs` and `ttdat.rs`; reader families `frostbite` and `ttdat` (revision 1) in `record.rs`.

- Frostbite (EA SPORTS FC 27): a `Data/layout.toc` with the toc magic makes its folder a Frostbite root. The plan reads `layout.toc`'s install chunks, every `.toc` of `Data/NX` and `Patch/NX` and each bundle's manifest in its cas file (read where it is, never the whole cas); the jobs are the CompiledBytecode resources, by cas file, in about equal stored sizes. Every `.toc` and `.cas` below the root is taken. A damaged table ends the split (the Python's planner raises). Stored blocks (codec 0) are read; an Oodle Leviathan block (0x19) ends the split with `unsupported container`: every item of FC 27's cas files is Oodle, so FC 27 still ends there, now with the block it stopped at.
- TT Games (LEGO Star Wars: The Skywalker Saga): a `.dat` whose first word points at a `.CC40TAD` index. Only `.nnx_shaders` members are read: stored, stored `OODL` / `DFLT` chunks, DFLT chunks (one final deflate block whose dynamic type is written 0). An Oodle chunk ends the split with `unsupported container` (the game's own decoder runs in the Mac host for the Python). An index the reader recognises and cannot read is one red job; the split goes on. Before, such a DAT was scanned as a plain file and gave no shader and no message.
- main (9f834e41): a loose control section that nothing references takes a code copy whose address the code builds next to a reference to it (ADRP/ADD x1, MOV w2, BL), else a unique adjacent code area (`adjacent_shader`). `generic` stays at revision 1: a game without TT DATs whose main has such sections keeps its tables until it is split again.
- Checked against upstream's Python: `tests/data/games/sync-ttdat` and `sync-frostbite` (tools/gen_ttdat_frostbite.py) byte-identical, logs identical; 549 damaged containers (tools/gen_ttdat_frostbite_vectors.py) answered as the Python answers. No real game was read.

## Known limits

- **Only one real game was available.** Many container readers (see the list in the source) are ported by reading and checked only against synthetic files built to the layouts the Python expects.
- Read since the nxnative sync of 2026-11-23: CRI CPK archives (`cpk.py`: stored and CRILAYLA entries, masked `@UTF` tables, the TOC before or after the data) and Diablo III's packages (`d3cpk.py`: the zlib streams of the SNO assets), `src/cpk.rs`; Unreal IoStore containers and encrypted paks; Super Smash Bros. Ultimate's data.arc and Game Freak's Trinity packs; Frostbite superbundles and TT Games DATs; Larian LSPK, Blizzard CASC, Camelot CRA, phx PDAT and Rockstar RPF6 archives. No container the Python's `plan_jobs` plans is refused before its reader runs any more (`detect_unsupported` is gone); what a reader cannot decode ends the split there.
- Refused with `unsupported container`: XNB with LZX compression, Next Level Games `.dict` + `.data`, Unreal shader code archives with Oodle shaders, Unreal IoStore shader libraries that need Oodle, Unreal paks with an encrypted index whose key `main` does not hold, Oodle-compressed entries elsewhere (Kraken, Mermaid, Leviathan: a Trinity pack's Oodle files, Frostbite cas blocks and TT Games DAT chunks among them), zip64 / bzip2 / LZMA zip members (Star Fox's chained-LZ4 stored members are read since 2026-11-23), EA chunk blocks of type 2 (RefPack) and 5 (LZMA), and NSO files with ZBIC compression.
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
- Encrypted paks (index, directory index and entries; AES-256 ECB, `aes.rs`): the key is found in `main` as upstream finds it (`Keys`: a key another pak opened with, then two 16-byte literals the code loads and stores together, then every 4-aligned 32-byte window of the read-only and data segments with at least 20 distinct bytes, whichever decrypts the index start to a mount point). It stays in memory; job memos record only that a key was used.
- IoStore containers (`uestore.rs`, `.utoc` / `.ucas` and its `_s<n>.ucas` partitions, encrypted ones with the key of the pak beside them or the first key found): the export bundles of the containers beside a pak with inline shaders are scanned like its packages (Unreal Engine 4.27 numbers them type 2, Unreal Engine 5 type 1; the newest copy of a chunk ID in mount order, labelled by the directory index's path or the chunk ID); Unreal Engine 5 shader libraries (`ShaderCodeLibrary` / `ShaderCode` chunks, SHA-1 or 8-byte hashes) become archives named by CityHash64 against the RomFS manifests and are read with the paks' archives. Blocks: stored, Zlib, LZ4.
- Oodle: upstream decodes it with the game's own `OodleLZ_Decompress`, run on the Mac in the host's tool mode with the rewritten modules loaded. On the phone the split runs before the game's modules are assembled, signed or loaded (`ibuilder_host_symbol` is null then), and no Oodle decoder under a licence the repository can take exists (ooz and its ports are GPL). So an Oodle block in a package costs that package (the Python's no-host line), and an archive or IoStore shader library that needs Oodle is refused. Games this blocks, from upstream's notes: Octopath Traveler 0 (archive shaders), FINAL FANTASY RESONANCE (IoStore blocks and shader groups), Yoshi and the Mysterious Book (shader-map streams), Mario & Luigi: Brothership (IoStore blocks).
- Refused (`unsupported container`): an archive with Oodle-compressed shaders, an IoStore shader library that needs Oodle or sits in an encrypted container (upstream's job reads none either), a pak routed to archive extraction whose AES key `main` does not hold.
- Parity: synthetic games `ue-io-lib`, `ue-io-bundles`, `ue-aes-window`, `ue-aes-code` (`tools/gen_uepak_io.py`) split byte-identical with upstream's `uepak.job` + `finish` + `shader_split.py`. A damaged TOC whose tables run past its end is a ValueError here, where the Python slices short and may stop with an IndexError.
- Not verified: no real Unreal game here; every file of Kakarot's 6.4 GB of paks is decoded and scanned (as upstream): time and memory on the phone are unmeasured. Error texts of damaged data differ from the Python's in the decoders' own words only.

## Falcom, MVGL, NXSD, Phyre

- **NXSD** (TETRIS THE GRAND MASTER 4, a63ca651): a file starting with `NXSD`, whatever its name (checked before the Next Level Games `.dict` rule), version 0x100: descriptors to 0x880-byte control sections and code areas that must tile the code region. Labels `#NXSD@<i>`, single stage.
- **Falcom** (Trails into Reverie, 930e9195): `.pka` archives (magic `0dcff77f`) in about 64 jobs by stored size, after the KIT jobs, never read whole, each blob read once; headerless `.pkg` packages recognised by their table. Entries stored (0), raw LZ4 blocks (1, 255:1 check) or one zstd frame (0x10); `.dds.phyre`, `.dae.phyre` and `.xml` are not read; other flags: one `[WARN]` line per package. Each entry goes through `scan_blob`, then whole GLSLC outputs.
- **Phyre** (930e9195): an `RYHP` container in which `phyre_shaders` finds no program but that holds GLSLC outputs gives those outputs.
- **MVGL** (Digimon Story, 131bd013 + 8cc63cc9): files starting with `MDB1`, wide (64-bit, LZ4) or legacy (32-bit, doboz) layout, planned after the PKZ jobs; only `.shad` entries are decoded and scanned. NXVS / NXFS / NXCS (/ NXGS / NXHS / NXDS) containers are read at the start of a blob (strict) and found inside any blob, `main` included (only a fully valid container counts). Labels `#NXSH@<offset>`, single stage.
- Decoders are capped at 1 GiB where the Python has only its memory. Not verified on real data (no Falcom, Digimon or Tetris files here).

## EA archives (BIG4, EASF, chunk containers, NXSHADER)

FIFA 18 (`0100F7B002340000`, EA's legacy "RNA" engine) keeps its shaders in `datax.big` → `data/fifarna/shader.big` and `shaderboot.big`. Port of upstream's `easf/` reader (nxnative cb6eb28b, PR #1696, adapted there from Touchline23, MIT; `easf.rs`, reader family `ea`):

- **BIG4**: `BIG4`, u32 (not read), u32 BE member count, u32 BE end of the index, then `{u32 BE offset, u32 BE size, latin-1 name, NUL}` per member. Only the index and each member's first 8 bytes are read; a top-level archive is planned in jobs of 1,024 members (`ea_jobs`, after the LINKDATA jobs). Its files are not scanned as loose files (as upstream).
- **EASF**: `EASF`, u32 BE plaintext size, 8-character key ID, SHA-256 of the first 32 plaintext bytes; AES-128-CBC from +0x30, IV = key, PKCS#7. The key is a `{id[8], key[16]}` record in the version's own `main` (add-on content: the newest one); only a candidate that decrypts the first two blocks to the header's digest is used. No key ships; the log names a key by the first 16 hex digits of its SHA-256. The digest covers 32 bytes only, not the file.
- **Chunk containers** (`chunkzip`, `chunzstd`, `chunlzma`, `chunkref`): magic, u32 BE version 2, total size, chunk size, chunk count, alignment, 12 zero bytes; per block `{u32 BE stored size, u32 BE type}` placed so that the data starts on the alignment. Types 1 (raw deflate) and 6 (zstd) are decoded; a block whose stored size is its output size is stored. Types 2 (RefPack) and 5 (LZMA) are refused (`block type 5 (LZMA) is not decoded by this port`). A container is decoded only when its first block's first bytes are an EA container. In the RomFS itself only `chunkzip` containers are looked into, as upstream (FIFA 18: 62,811 `chunzstd`, 3,786 `chunlzma`, 382 `chunkref` are counted in one log line each; peeking at the zstd ones cost 30 s on an M1 Pro and found nothing).
- **NXSHADER** (`.fxo`): `NXSHADER`, 8 version bytes, `GLSL` + u32 LE size of the one GLSLC output that follows at +0x18, then `REFL`. Read by the GLSLC reader. Labels `/<file>#<member>…#NXSHADER#GLSLC@0x18#var`.
- **Pairs**: upstream tags these `#NXSHADER@<offset>` in `SINGLE_STAGE`: every vertex shader is paired with the pixel cover (on FIFA 18 one pixel shader). Here a `.fxo`'s vertex and pixel shader, compiled by GLSLC as one program, pair as that program (`#var`): `vs_pad` pads each vertex shader to the pixel shader(s) it is compiled with. Same keys and packages as upstream; `shader_pairs.tsv` differs (160 program pairs instead of 160 vertex × 1 cover).
- Measured on FIFA 18 v0 (M1 Pro, 8 jobs): 330 shaders (160 vs, 160 ps, 10 cs), 160 pairs, 2.1-2.9 s, peak footprint 89 MB; packages byte-identical to the split of the decoded `.fxo` files; shaderbuild 330/330. Not verified: FIFA 19-23 or other EA games (upstream checked FIFA 23), a phone run.

## Super Smash Bros. Ultimate data.arc, Game Freak Trinity packs

- **data.arc** (`smash_arc.rs`, upstream `smash_arc.py`, `smash_arc_jobs`): an `.arc` file whose first u64 is 0xABCDEF9876543210 (MT Framework's ARC archives share the extension, not the magic). The file system table (one zstd frame, decoded up to the size its header gives, refused past 1 GiB) is read at planning time; each path is followed to its file info, so only the current copy of a file is read (the update kept the superseded ones in place), every region's data for a regional file. The files, in offset order, go to 128 jobs of about equal stored size (upstream `parts=128`); a job reads one file at a time, never the 17.6 GB archive. Each file goes through `scan_blob`, then whole GLSLC outputs. Labels `/<rel>/<kind>#<path Hash40>` (an SSBH file's resource type, e.g. `SHDR`, else its first four bytes; `data` when not alphanumeric). Planned after the dvdbnd jobs, before PKZ.
- **Trinity** (`trinity.rs`, `trpfs_jobs` / `trinity_shaders`): `data.trpfs` (`ONEPACK`) with `data.trpfd` beside it, FlatBuffers read the way `struct.unpack_from` reads them (a negative offset counts from the end). Packs in 64 jobs by the descriptor's pack sizes, in offset order, after the KPKA jobs; both files are taken out of the plain scan. A file whose data is its length is scanned (`scan_blob`); another length is the engine's red line for its range. Labels `/<rel>#<pack name>#<file hash>`. Refused (`unsupported container: Oodle Mermaid (Trinity pack ...)`): a file of compression 3, which upstream decodes with the game's own decoder in the Mac host; Pokémon Legends: Z-A's packs are Oodle, so this port cannot split that game. A FlatBuffers vector of more than 2^24 entries is refused as damaged.
- Parity: the synthetic games `sync-smash` and `sync-trinity` (`tools/gen_smash_trinity.py`) are byte-identical with upstream's Python at 1, 4 and 8 threads; `err-smash-*` and `err-trinity-unlisted` end with the engine's text; `vectors-smash-trinity.jsonl.gz` (`tools/gen_smash_trinity_vectors.py`) holds damaged copies of the three containers, each split end to end by upstream's Python (`tests/smash_trinity.rs`). Not verified: no Smash or Trinity data here.

## LSPK, CASC, CRA, PDAT, RPF6

Ports of upstream's `lspk.py`, `casc.py`, `cra.py`, `rpf6.py` and `pdat_jobs` (`lspk.rs`, `casc.rs`, `cra.rs`, `pdat.rs`, `rpf6.rs`, `aes.rs`), planned in the Python's order (CASC and CRA after the MVGL jobs, PDAT, RPF6 and LSPK after the ARH jobs):

- **LSPK** (Divinity: Original Sin 2): a `.pak` whose last 8 bytes end in `LSPK`, version 13, its LZ4 file table, numbered parts (`<stem>_<n>.pak`); only `.bshd` members are read, each by its own range. A BSHD record's NX stages are GLSLC outputs (labels `#BSHD#var`, its vertex and pixel stages pair); other platforms' stages are left alone. A package that cannot be read is one job of its error, before the others.
- **CASC** (Diablo II: Resurrected): a folder with `build_key.txt`, a `.psv` manifest and `<chunk>-<archive>.data` archives; the manifest naming the build key lists the current files (encodings `n`, `z`, BLTE). Every file is decoded and scanned (SHAD containers); a file that does not decode is logged and not scanned; a folder without a current manifest is logged and its archives scanned as loose files.
- **CRA** (Mario Tennis Fever; Mario Golf: Super Rush's 2021 layout): `rom:/42`, its file table read from the first NRO in `DM_NRO_DIR` (`SplitOptions::nro_dir`) that holds one, found by the instruction patterns of the code that reads it; files unscrambled and checked against their checksums; FSTM / EBML files left out by their first block. NARC containers (anywhere `scan_blob` meets one, not only in the archive) are read and their members unpacked (LZ4, zstd, zlib / gzip, raw deflate, Camelot's LZ, chunks). Without `nro_dir` a game with a CRA archive ends with the Python's red line. The phone passes none: that NRO is in no file of the game (Mario Tennis Fever's `main` is a boot stub that decodes it at run time; upstream's NSP script captures it by booting the game in the Mac host, `DM_NRO_CAPTURE`, `capture_nro`), and the phone's pipeline neither captures nor loads NROs yet.
- **PDAT** (Kirby Air Riders): `.dat` archives in byte ranges of at least 16 MiB, read through an 8 MiB window (the Python reads the whole range); PMAP records found by their zstd frames and by NVN2 control sections outside the frames. As in the Python, a frame's extent is taken as its start + 1 MiB minus what follows it in the 1 MiB read, so near an archive's end a stored record after a frame is not read.
- **RPF6** (Red Dead Redemption): the entry and name tables are AES-256 ECB decrypted 16 times with a key found in the game's `main` at split time (the key setup's constants first, then every 32-byte window of its read-only and data segments, about 0.8 M windows/s on an M-series Mac); no key is stored anywhere. Files are zstd, zlib, raw deflate, RSC5 / RSC85 resources or encrypted scripts; each is scanned like a loose file (the `rgxd` effects).
- Parity: seven synthetic games (`sync-lspk`, `sync-casc`, `sync-pdat`, `sync-rpf6`, `sync-rpf6-setup`, `sync-cra`, `sync-cra2021`, `tools/gen_sync5.py`, `gen_sync_rpf6.py`, `gen_sync_cra.py`) byte-identical with upstream's Python at 1, 4 and 8 threads, logs included; 3,200 damaged-input vectors (`vectors-sync5`, `vectors-rpf6`, `vectors-cra`) give the Python's answers, decoders' wording aside (LZ4, libzstd, zlib's fast path). Not verified on real data: none of these games is here.
- Differences kept on purpose: a damaged CRA prefix chain on which the Python loops for ever ends with an error here; recursion (RPF6 directories, CRA chunks) is cut at a depth of 900; decoded members are capped at 1 GiB.

## Plan order and the planners' inputs

`jobs::plan_env(files, log, path_of, &PlanEnv { mains, nro_dir, zstd })` (`jobs::plan` with `PlanEnv::default()`: no mains, no NRO folder, `ruzstd`) plans the archives in the Python's `plan_jobs` order: linkdata, [ea], kpka, trpfs, ue, bdt, smash_arc, pkz, cpk, [mvgl], d3cpk, casc, cra, zip, arh, [frostbite], pdat, rpf, lspk, then kit, ttdat, falcom_pka. The bracketed ones are planned in `split.rs` and put at their place (`Plan::mvgl_at`, `Plan::fb_at`; EA after the LINKDATA jobs), so the first copy of a key is the Python's. `mains` is each version's `main` (an encrypted Unreal pak's key, an RPF6 key), `nro_dir` is `SplitOptions::nro_dir` (`DM_NRO_DIR`), `zstd` the split's zstd decoder (Smash's file system table). One `aes.rs` serves every reader: AES-256 ECB (Unreal, RPF6) and AES-128 CBC (EASF), checked against the FIPS-197 and SP 800-38A vectors.

## Registered sources

`DM_SPLIT_REGISTERED_MAP` (c9b12271 + f41613f7, `SplitOptions::registered`, `--registered`): when a shader is found in several places, the copy at the source a registered `shader_map.tsv` names (same key and stage) is kept instead of the first, in `main` and across files, once every copy is checked; the game's copy still wins over an add-on's, and the add-on side never wins. The rows per source group of the summary line are counted over the final keys. The phone does not pass a registered map (its library is built from its own split's tables), so its output does not change.

## Snowdrop bare zlib caches (e74c3fa7)

South Park: The Fractured But Whole stores its shader caches as bare zlib streams (`#SDFZLIB@`). Since e74c3fa7 every stream of a cache must decode to exactly 64 KB, the last one to the rest (`... block @0x.. did not finish at <n> raw bytes`); a cache in streams of other sizes, which the older reader took, is refused. The 1 GiB cap stays (checked before each block). `vectors-snowdrop.jsonl.gz` was regenerated with upstream's Python: 290 of its 1,057 damaged slices now end with the new text; no valid slice changed.

## Targeted re-split (`record.rs`)

A library built on the phone records the split's version in `pack.txt` (`split`). Before 2026-11-22 any change of it removed every game's library. Now:

- `SPLIT_REV` (the `split` digest) is the global revision: when it differs, the library is split again.
- Each reader family has a revision (`record::READERS`): apf, bahs, casc, cpk, cra, d3cpk, ea, falcom, frostbite, generic, ipk, kit, kpka, lspk, mtarc, mvgl, nxcx, nxsd, pdat, phyre, rage-wad, redengine, rpf6, sharc, smash-arc, snowdrop, telltale, trinity, ttdat, ue3, unity, unreal-pak, vbf, zip (`generic` is everything else: the blind scan, BNSH, BAHS, GLSLC, SARC, zstd ...).
- The split writes a record (`SplitOptions::record`; the app's backend writes `<work>/split.record`), which stage 3 keeps in `pack.txt` or the no-shader marker: `split-revs` (every reader's revision), `split-uses` (the readers the game needed: a top-level file the reader recognises by its magic, layout or name, or a shader label with the reader's tag), `split-unknown` (the files no reader recognised, as `<extension>#<first four bytes in hex>`, at most 8 per extension, then `<extension>#*`).
- A library is split again only when a reader it uses has another revision now, or when a reader whose revision moved claims one of its unknown kinds (`Reader::claims`: an extension or a magic prefix). Otherwise its record takes this app's revisions and it is kept.
- A library from before records existed is taken to have the revisions every reader had at `SPLIT_REV` 3 (`record::LEGACY`), and its game's files are classified again once (`record::probe`, about 3 s for Ocarina of Time's 186,583 files on an M1 Pro), with the labels of its shader map; the answer is written into `pack.txt`.
- This change: new readers falcom, mvgl, nxsd, unreal-pak (revision 1), phyre 2, snowdrop 2. Later: ea 1 (claims `BIG4`, `EASF`, `NXSH`, `chun` first bytes: a game whose `.big` files no reader read, FIFA 18, is split again); casc, cpk, cra, d3cpk, frostbite, lspk, pdat, rpf6, smash-arc, trinity, ttdat 1 and unreal-pak 2 (the nxnative sync of 2026-11-23: a game that holds their files, or whose unknown kinds they claim, is split again; before, most of them ended with `unsupported container`). The sync with nxnative 033793f0: bahs, mtarc, nxcx, rage-wad, redengine, sharc, telltale, vbf 1 (new), ipk 2, ue3 2, zip 2. bahs claims BAHS, SARC, Yaz0 and zstd first bytes, because agl's BAHS files ship inside those packs (a library recorded before has no `bahs` in its uses): every game with such a pack is split again once (most Nintendo titles), which a bump of `generic` would also have done for every other game. Measured (tests/record.rs, synthetic): plain, misc, zip and the rest-* games are split again; sync-registered, kpka, linkdata keep theirs. Measured earlier: Ocarina of Time (`0400486024DEC000`) and Beach Buggy Racing 2 keep their libraries; a game with Snowdrop data slices (South Park, Kingdom Battle), Phyre files or KIT archives, or files of the new readers is split again.
- Limits: a container inside another one (a SARC in a zstd file) is seen only through the labels of the shaders it gave; a reader that may read files it does not recognise at the top must list them in its claims, or its change must bump `SPLIT_REV`. A memoised job and an interrupted work folder are tied to every reader's revision.

## Audit against nxnative 033793f0 (2026-11-23)

Every container or format upstream's `shader_split.py` reads at 033793f0 (`plan_jobs` and its `*_jobs`, every magic `scan_file_in` and `scan_blob` dispatch on, `main()`'s own inputs), against the Rust split. Status before this sync → after. "parity" means tests/games.rs splits a synthetic game of it to the Python's bytes (pkg, shader_map, facts, pairs, pso_pairs at 1, 4 and 8 threads, and its `[RED]` lines); "vectors" means damaged inputs against the Python reader (tests/split_rest.rs and the older vector tests).

| Format (upstream reader) | Upstream commit | Rust before → after | Evidence |
|---|---|---|---|
| agl BAHS, validated binary array drops stray descriptors (`bahs_shaders`, `bahs_binary_records`, `bahs_variations`) | 5df3dbe8 | partial (old filter: a stray descriptor gave a false shader) → read | rest-bahs; vectors (bahs + variations) |
| UbiArt IPK v5 (`ipk_shaders`: 0x30 header, name then directory, directory join) | e0c30043 | refused (`only version 7`) → read | rest-ipk; vectors |
| REDengine 3 RDHS material cache (`rdhs_shaders`), FZLC static cache (`fzlc_shaders`) | a4c82979 | missing (scanned as loose files, records not decoded) → read | rest-redengine; vectors |
| PACx 403 header / root chunk table / PACx 402 root header bounds (`pacx4_parts`) | 760c7fe8 | partial (same result, other error text) → read | rest-pacx; vectors |
| Unreal short v8 paks and v1 shader archives (`uepak.py`, `ue_legacy_jobs`) | 637f1dfe | read (ported with uepak.py as of 184847c5) → read | ue-v8short-v9 (earlier sync) |
| RAGE WAD v1 `out.wad.nx`, VFP GLSLC arrays (`noire_wad_shaders`, `noire_vfp_shaders`) | 19c5379d | missing → read | rest-wad; vectors |
| NXCX texture archives, searched in chunks (`nxcx_shaders`) | 0ec204f5 | missing (the loose-file path, same shaders in the tested layouts, the file read whole) → read (16 MiB windows, never whole) | rest-nxcx; vectors; a window-crossing unit test |
| Telltale `.ttarch2` NCTT / ZCTT (`ttcz_chunks`, `ttarch2_shaders`) | c714bb92 | missing → read (an archive that names no `.t3fxpack` is not read whole) | rest-telltale; vectors |
| GIANTS SHARC v11 (`sharc_shaders`, raw LZMA1 without end marker), in `scan_file_in` and `scan_blob` | f3950c50 | missing → read (`lzma1.rs`) | rest-sharc; vectors; 1,320 LZMA streams cut / damaged against liblzma |
| zstd decoded in process, concatenated frames (`unzs`) | 6aaf73ba | partial (`ruzstd` stopped after the first frame: a file of two frames lost its shaders) → read (every frame, skippable frames, garbage after a frame is an error) | rest-zstd |
| MT Framework ARC v9, headed SPK, bounded zlib members (`mtarc_members`, `mtspk_headed_shaders`) | c931e584 | refused (`only version 7 is known`; headed SPK rejected) → read | rest-mtarc; vectors |
| UE3 packages padded with zeros after the last chunk (`ue3_body`) | 02614c26 | missing (red) → read | rest-ue3pad |
| Virtuos VBF / SRYK (`vbf.py`, `phyre_shaders`) | bee218d9 (before a3663659) | refused → read | rest-vbf-zip-play; vectors |
| Star Fox `data.zip` chained-LZ4 stored members (`viper_zip_member`) | before a3663659 | refused → read | rest-vbf-zip-play |
| play captures, `sys.argv[9]` (`glslc_play`, the `/glslc_play/` merge rule) | ae5d8f51 | missing → read (`SplitOptions::glslc_play`, `--glslc-play`) | rest-vbf-zip-play |
| shared archive probes, `plan_jobs` (`file_heads`, `starting(...)`) | 588a5d72 | planned in the same order (`jobs.rs plan_env`) | every game |
| XNB with LZX compression (`xnb_decompress`, the `EffectReader` first-block test) | before a3663659 | refused → **still refused** (`unsupported container`) | — |
| Next Level Games `.dict` / `.data` / `.patch`, revisions 1 and 4 (`nlg_shaders`, `nlg4_shaders`) | before a3663659 | refused → **still refused** | — |
| Oodle (Kraken, Mermaid, Leviathan: GFPAK, PKZ, Trinity, Frostbite, TT DAT, UE3 blocks, Unreal archives) | — | refused: the game's own decoder in the Mac host | — |
| the `main holds GLSL sources and imports glslcCompile` warning (`main_imports`, `glslc_module`) | — | not ported: a log line only, it needs the NSO's dynamic symbols and the profile's modules | — |
| Every other reader of `scan_blob` / `scan_file_in` / `plan_jobs` (SHN, NXSH, CIVBSF, SUMO, Monolith, PACx, SLCT, Yaz0, SARC, NARC, rgxd, NDSR, NDVE, sead, TFXO, SHAD, MSHD, PlatinumGames DAT / shader objects, Viper, Phyre, VIPR, BNSH, MGFX, Needle, NVNsh, NXSD, EA, TT DAT, LSPK, Falcom, KIT, ARH, IDRK, Trinity, PDAT, zip, Unreal, Frostbite, RPF6, PKZ, CRA, CASC, D3 CPK, CPK, MVGL, KPKA, Smash ARC, BDF3, LINKDATA, bnd2, Heaps, KTGL, UE3, RFRM, BERG, BEA, GFPAK, gzip, Gust, HAL `.cmp`, XNB LZ4, Unity) | — | read | the earlier games and vectors |

Not upstream: Vector Unit APF packages (`apf.rs`) come from the NX2Apple patches. Error texts: zlib errors now come from the model of zlib (`zlibx.rs`) whenever the fast decoder refuses a stream or stops short, so they are zlib's own words; lz4 and libzstd error texts still differ (the vector tests normalise them).

What is measured: the synthetic games and vectors above, `cargo test --release -p ibuilder-shadersplit -p ibuilder-pipeline`. Not verified: no real game of any of these formats was available (The Witcher 3, L.A. Noire, TSUKIHIME, Sam & Max, Farming Simulator 26, Just Dance, Resident Evil HD Remaster, Star Fox, FFX/X-2, FFXII), so real layouts beyond what upstream's tests and docstrings describe are unchecked; liblzma's behaviour on a dictionary smaller than a distance is modelled, not measured.

Regenerating: `tools/gen_split_rest.py` (the games) and `tools/gen_split_rest_vectors.py` (the vectors) with Python 3.14 + lz4 against an upstream checkout.
