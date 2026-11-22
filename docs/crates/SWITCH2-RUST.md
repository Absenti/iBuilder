# SWITCH2-RUST: Switch 2 support in the Rust chain (package SW2, round 3)

Goal: build The Legend of Zelda: Ocarina of Time (Switch 2, title id `0400486024DEC000`, update v131072) with the Rust chain, from its `.nup` container
(25,622,267,860 bytes, read only). Before this package the Rust toolchain had no NUP reader (stage 1 for a `.nup` was the Python `extract_nup` / `pack_nup.py`
with zstd streaming) and refused ZBIC-compressed NSO modules (flag bit 7: zstd instead of LZ4) in four crates. Read `docs/CONVENTIONS.md` first.

| what | where | status on the real game |
|---|---|---|
| NUP reader (`pack_nup.py`: `Nup`, `scan`, `romfs_bytes`, `pack_version`, `pack_dlc`), RomFS image written straight from the container | new crate `ibuilder-nup` | extracted once into `$S2/ocarina/extract/` (see 8), ExeFS byte-identical to the Python's |
| ZBIC decoder (Nintendo's zstd variant, NSO flag bit 7) | `ibuilder-nso/src/zbic.rs`, used by `ibuilder-nso`, `-rewrite`, `-profile`, `-mods` | `main` (flags 0xff) loads, all three segments pass their SHA-256 and equal the Python `zbic` decode byte for byte |
| function-matched frame step + dynamic-resolution stubs of this game (`MATCH_FRAME_STEP`) | `ibuilder-rewrite/src/stubs.rs` | `main`, `subsdk0`, `sdk` rewritten: every output file and the log equal the Python oracle's |
| `NupReader` adapter, `ibuilder --nup FILE --out DIR` | `ibuilder-cli` (feature `nup`, default on) | whole chain from the real NUP: stages nup, profile, rewrite, assemble, sign ok, 10.1 s |
| shader split of the game | `ibuilder-shadersplit` (not changed) | 42,848 shaders, tables identical to the engine's |

## 1. Sources and what was (not) read

* Python oracle and spec: the engine's `scripts/pack_nup.py` (worktree `$IOSBUILDER_WORKTREE/out-nx2apple/engine/scripts/`, 30,853 bytes, mtime 2026-11-18 15:44), its
  `dm/tools/romfs_image.py` (930 lines, identical in both worktree copies), `gameapp/nup_zstd.py`, `gameapp_pipeline.extract_nup` (155,544-byte file of 2026-11-19 02:24), the
  engine's `nso.py`, `dm/games/0400486024DEC000/{README.md, game.tsv, patches.py}`. The rewriter oracle is a COPY of the engine's `dm/tools` (`patch_nso.py`, `nso_match.py` are
  byte-identical to `iOSBuilder/engine/dm/tools`) and of the game's `patches.py` (identical to the engine's), run from the scratch folder with an isolated interpreter.
* Third party, **documentation only**: https://github.com/Mars7x/echora (GPL-3.0-or-later): `docs/architecture.md` and `docs/research.md` (the "ZBIC format" and "Checked input" /
  "Memory and heap sizes" sections), read through a page reader. No code of echora was read, copied or translated. Those pages say that ZBIC differs from Zstandard in two places
  only (the frame magic, and the coding of the FSE table descriptions: binary interpolative coding as mixed-radix digits, byte 0 = length - 1 and a low-probability flag), without the
  details; they point to the GPLv2 `zbic` package source for them.
* The `zbic` Python package (kinnay/zbic, GPLv2, in the reference venv) was used as a **black box**: `zbic.compress` / `zbic.decompress` generate and check frames. Its C source
  was not read. The rest of the format was measured (section 4). The zstd machinery is RFC 8878, implemented from the specification and checked against libzstd.
* Not used: any key. A NUP is not encrypted.

## 2. The NUP container as implemented (`ibuilder-nup`)

Little-endian. Superblock 0x80 bytes: `NUP` + format byte 1, header size, TOC offset/size/SHA-256, data-blob offset, file size, package count, flags (must be 0), uuid. TOC
(`TOC\x01`, package/folder/file counts, name-pool offset/size), then 0x40-byte package records (title id, root / exefs / romfs folder indices, version, kind 0 base / 1 update / 2 add-on /
3 add-on update / 4 system, flags bit 0 = standalone update, meta block offset/size), 0x18-byte folder records (name, first child slot, count), 0x50-byte file records (name, size,
extent offset and size, SHA-256, codec 0 raw / 1 one zstd frame), the u32 child-slot array (a slot is a folder index, or the folder count plus a file index), the name pool. Every
extent is 16-aligned; extents may be shared (the real container stores each of the two versions' identical files once). The meta block is a TLV list (u16 type, u16, u32 length): 2 = NACP,
3 = icon, 10 = localized icons. All checks of the Python's `_read` are ported with its messages (empty / short file, format version, header, file-size mismatch, TOC/blob overlap, TOC SHA-256,
malformed TOC, name / child / folder bounds, a folder that contains itself, reserved fields, codec, extent bounds); a TOC above 1 GiB is refused before it is read (the Python has no such cap).

**The real container** (`ibuilder-nup info`): 25,622,267,860 bytes, 2 packages (so not the "update only" container echora's page describes, 25,020,741,061 bytes): package 0 base v65536
(title `0400486024DEC000`), package 1 update v131072 (title `0400486024DEC800`, flags 0: not standalone); 1,199 folders, 372,587 files = 2 x 186,583 + the ExeFS files, **all raw (codec 0):
no zstd file in the container**; each package's tree is 24.47 GiB (the two share their extents: 48.94 GiB of files in a 23.86 GiB container). Meta: the base has TLV 1 (NPDM, 1,828 B),
5 (36 B), 7 (32 B), 8 (4 B); the update has only the NPDM. **No NACP and no icon**: `control.nacp` and `icon.jpg` are not written (the Python writes none either). 0 DLC, 0 other programs,
`aoc` base `0400486024DED000`. The update's ExeFS: `main` 29,953,289 B, `main.npdm` 1,828, `rtld` 10,524, `sdk` 7,833,714, `subsdk0` 533,396; the RomFS holds 186,583 files, 24.43 GiB.

`scan` (port of the Python, same errors): the program ID of each package's `main.npdm` (ACI0 +0x10) tells programs apart; they belong to one application by the NACP's
SaveDataOwnerId, else by the ID above its low 12 bits; the highest program index is packed by default (a Switch 2 Edition's Switch 2 program), `--program N` picks another and its siblings'
RomFS go to `romfs_programs/<index>.img`; the newest update is packed (`--version`, 0 = the base); no base is accepted only with standalone updates (message `NO_BASE`); DLC indices
are title ID - AddOnContentBaseId. Tests (synthetic containers): two programs, no base, no base without the standalone flag, DLC.

### Layout it leaves (as `extract_nup`)
```
<extract>/ExeFS/            the version's files that do not start with '.' (main, main.npdm, rtld, sdk, subsdk0): kept when every file already has the container's SHA-256
<extract>/RomFS.img         DMROMFS1 (romfs_image.py): header 32 B, the files' bytes in breadth-first entry order, the index (24 B entries + names) last; written to RomFS.img.part, renamed
<extract>/RomFS.img.inputs.json   this crate's stamp ({"source": sha256(tag + TOC hash + folder), "image": [inode, size, mtime_ns, ctime_ns]})
<extract>/romfs_programs/<index>.img, control.nacp, icon.jpg   when the container has them
<games>/<title id>/aoc/<index>.img   each DLC (the closure `aoc(title id)`; the adapter passes Documents/games/<id>/aoc)
```
Real result: `RomFS.img` 26,247,576,430 bytes (24.44 GiB), 187,182 entries, 186,583 files. The `.listing` artifacts are left out as in the Python.

### Design (what differs from the Python, on purpose)
* **No mmap of 25 GB**: positioned reads (`pread`) through the `ReadAt` trait (`FileSource`, `MemSource` for tests). Several threads read different files at once.
* **Threads**: every file's offset in the image is known from the container's sizes before the first byte is written, so rayon workers `pwrite` each file at its place (index and header
  last, in the `.part` file); the output is identical for any thread count (tested at 1, 3, 4, 7 threads and with both zstd backends). Memory stays ~130 MB (8 workers).
* **SHA-256 of every file** is checked as it is copied (size and hash, the Python's message); `verify_container` checks all 372,587 files without writing (`ibuilder-nup verify`).
* **Reuse**: `Kept` (stamp matches), `Verified` (another packer's stamp, e.g. the Python's: the image is checked against the container's hashes, then stamped), `Packed`. A damaged image is packed
  again; a cut `.part` is removed. The Python's stamp digest hashes the text of its own scripts and cannot be reproduced: its image is `Verified` once, not `Kept`.
* **Free space**: as the Python (`romfs_bytes + 512 MiB`, here 26,236,422,768 + 536,870,912 = 24.93 GiB) with `statvfs`, or the host's own function (`Options::free_bytes`); one change: an image
  that is already there at the size this container's RomFS gives is not asked for again (the Python refuses a nearly full volume for an image it would only verify), which is what makes a
  repeat run work with 28 GB free.
* **Progress and cancellation**: `Options::progress` (step, bytes, files; every 250 ms by default, `progress_ms`), `Options::say` (the Python's `say` lines), `Options::cancel`
  (checked between chunks; a cancelled extraction leaves no image and no `.part`). The adapter turns progress into the stage line and the pipeline's cancel flag into this one.
* **zstd**: a trait, `ZstdBackend::decode_frame(src: Read, sink)`: streaming, one frame per file, trailing bytes or a cut frame are errors. `NativeZstd` (C libzstd through the `zstd` crate, built
  by `cc`, **builds for `aarch64-apple-ios` and `-ios-sim`**), `Ruzstd` (pure Rust, already in the lock through `ibuilder-shadersplit`), `CallbackZstd` (the app's own streaming binding,
  `_dmgameapp.zstd_stream_feed`). Default: the C library. Measured on this Mac (`examples/zbench.rs`, one thread, decode + SHA-256, 8 MiB frames, level 3): game code (`main`'s text, 63 MB,
  ratio 2.59) **628 MB/s with libzstd against 203 MB/s with ruzstd**; RomFS data (400 MB of movies/packs, incompressible, ratio 1.00: raw blocks) 1,430 against 1,320 MB/s (SHA-256 alone
  1,670 MB/s). For tens of GB of compressible data the C backend is 3x faster; for NSO segments (<200 MB, section 4) speed does not matter and the ZBIC decoder is its own code. No dictionary is
  needed by the container: the `Pack/ZsDic.pack.zs` dictionaries belong to the game's own `.zs` files **inside** the RomFS, which are copied as the bytes they are
  (`ibuilder-nup formats RomFS.img`: 102,185 files named `*.zs`, all start with a zstd frame; the Python's `format_mismatches`; the three damaged `.zs` textures of a PR the engine notes
  describe are not in this container).
* **A folder listed twice is refused (deliberate divergence from the Python reader)**: `Nup::acyclic` refuses a TOC in which one folder is listed by more than one slot (two parents, or twice by
  one), as well as a folder that contains itself; `romfs::read_index` refuses the same on the `DMROMFS1` index. The Python reader accepts such a file (a DAG) and then walks a shared folder once
  per path, which is exponential in the number of stacked "diamonds" (a TOC of a few hundred bytes), so it either loops or runs out of memory. No real container does this (the game's has a
  tree), so output for valid input is unchanged; a file with a shared folder is an `Err` here where the Python would eventually (or never) produce something.
* **unsafe**: one `statvfs` call in `sys.rs` (justified in the module); everything else `#![deny(unsafe_code)]`. No `fork`/`exec`/`Command`.

### Python / Rust map
| Python (`pack_nup.py`, `romfs_image.py`, `extract_nup`) | `ibuilder-nup` |
|---|---|
| `Nup` (`_read`, `_acyclic`, `children`, `child`, `tree_bytes`, `copy`, `read`, `head`), `Package` (`meta`) | `nup.rs`: `Nup::{open, from_source, children, child, tree_bytes, copy, copy_with, read, head, meta}`, `Package` |
| `program_id`, `nacp_ids`, `scan`, `NO_BASE` | `scan.rs`: `scan(&Nup, program) -> Scan`, `program_id`, `nacp_ids` |
| `exefs`, `control`, `localized`, `pick`, `app_package`, `romfs_of`, `sibling_romfs`, `romfs_bytes`, `pack_version`, `pack_dlc` | `extract.rs` (same names; `pack_exefs_only` for `--exefs-only`) |
| `entries`, `romfs_image.{layout, write, stamp, reusable, publish, read_index, FORMAT_HEADS, format_mismatches}`, `verified`, `pack` | `romfs.rs`, `extract.rs::{pack, verify_image, write_image}` |
| `gameapp_pipeline.extract_nup` | `extract_application` (+ `ibuilder-cli`'s `RustNup`) |

CLI: `ibuilder-nup info FILE | extract FILE --out DIR [--aoc DIR --version N --program N --threads N --exefs-only --ruzstd] | verify FILE | bench-zstd FILE | formats RomFS.img`.

## 3. ZBIC in `ibuilder-nso` (shared)

`ibuilder_nso::zbic::{decompress_into, zstd_decompress_into, Decoder}`: a zstd (RFC 8878) decoder written from the spec: raw / RLE / compressed blocks, Huffman literals (1 or 4 streams, treeless
reuse, direct and FSE-compressed weights), FSE sequence tables (predefined / RLE / compressed / repeat), offsets with repeat offsets, matches into the whole frame; no dictionary, no window limit
(the output is one buffer), the content checksum is skipped (the NSO header's SHA-256 is the check). A `Variant` switches the magic and the table-description reader, so **the stock-zstd mode checks all
the shared machinery against libzstd** (`tests/zstd_machinery.rs`: 140 frames of five kinds of data at four levels, 1 B to 1 MB: identical). The ZBIC mode is checked as in section 4.
`ibuilder_nso::load` calls it for flag bit 7 (the messages of the Python: `ZBIC does not decompress (...)`, `ZBIC decompresses to N bytes, not the M of the header`); a ZBIC segment is held only to
its own size (no LZ4 bound; `ibuilder-mods` reserves its buffer by the input, at most 32768 bytes per input byte: a 128 KiB block costs at least 4 bytes, so no valid stream is cut, and a header that claims 2 GiB for a 3-byte stream no longer allocates it). `ibuilder-profile` (own positional-read loader) and `ibuilder-mods` (`nso_image`: a rejected stream is `zbic.error` "ZBIC data is invalid", which `mods.py` does not
catch) call the same function; `ibuilder-rewrite` loads through `ibuilder_nso::load`. Their former "ZBIC refused" paths and tests are replaced by decoding tests.

**Why not stock zstd (`zstd` / `ruzstd`) for NSO segments**: a ZBIC frame is not a zstd frame. Its magic is `ZBIC` and its FSE table descriptions use another coding, so neither library can read
it (a transcoder in front of libzstd would have to rewrite the Huffman header byte, the literals size and the block size around every table). A decoder of our own is ~800 lines, pure Rust (no C, no
new dependency, iOS-clean), and decodes `main`'s 81 MB in 0.38-0.45 s including the three SHA-256 checks (Mac and iOS simulator), against 3.0 s for the Python package's C decoder on the same bytes.
Neither backend choice for the NUP (above) touches it.

## 4. The ZBIC format as measured

A ZBIC frame is a zstd frame with `ZBIC` as magic; the frame header, block headers, literal sections, Huffman and sequence bitstreams, predefined tables and offsets are stock. Only the
**table description** (the three sequence tables of a block, and the table of an FSE-compressed Huffman weight list) differs. Measured with the `zbic` compressor as oracle
(`crates/ibuilder-nso/tools/gen_zbic_frames.py`): for the same data and level, stock zstd's table counts are the ground truth for the description the ZBIC frame holds (the sequence bit streams of the two
frames are identical, so the tables are); 3,562 (counts, description) pairs, then 1.3 million descriptions.

* Byte 0: low 7 bits + 1 = the description's length in bytes (byte 0 included); bit 7 = the table may have low-probability symbols (count -1), which turns on the "slack" below. (It is also set on
  tables that turn out to have none.)
* The other bytes are read **from the last toward the first** into a 64-bit state, a byte at a time: while the state is below 2^40 another byte is appended at its low end (the first byte read is the most
  significant). Digits come out as `d = state % radix; state /= radix` (least significant first), then the state is topped up again.
* Digits: `m - 1` (radix 52; `m` = the largest symbol, so tables of one symbol, which are RLE, are never described), `log - 5` (radix 8), `F - 1` (radix `2^log`; `F` = the sum of the signed
  counts), then the walk.
* The walk: the cumulative counts `C_k = c_0 + .. + c_(k-1)` at positions `0..=U`, `U` the smallest power of two above `m` (`C_0 = 0`, `C_p = F` from `m + 1` on), coded by binary interpolative coding in
  right-first preorder: visit the middle `mid = (l + r) / 2` of an interval with known end values, then its right half, then its left half. Digit = `C_mid - lo`, radix `hi - lo + 1`, where
  `lo = C_l - down(l, mid)`, `hi = C_r + down(mid, r)` and `down(a, b)` = the number of symbols `a..b` that exist (index <= m) when the flag is set, 0 otherwise (a table without low-probability symbols has a
  non-decreasing cumulative count). Positions past the last symbol are coded too (their values are forced, cost bits, and are what the bounds of later nodes read); the counts are the differences.
* No terminal digit: the state ends at 0 with every byte consumed, in every valid description.
* **The window.** The compressor flushes a byte when its state reaches `(2^48 + 2^32) / radix`; the real modules' compressor does so below `2^48`. A state in `[2^40, 2^40 + 2^24)` at a
  top-up is therefore topped up (the `zbic` compressor's streams, 1 description in 25,000 has one) or left alone (the real `main` has one among its 4,881 descriptions, a Match Length table), and the
  decoder cannot tell by itself. Measured: only `2^40 + 2^24` as a constant decodes every compressor description; only `2^40` decodes `main`. The decoder does what a validity check allows:
  it decodes with no top-up in the window, and if the table is not valid with nothing left over (sum of |counts| = 2^log, counts >= -1, state 0 and every byte used) it tries the other choices for the
  states that fell in the window (at most 2^10 combinations; a description has 0 or 1 window state in nearly every case). Over **1,324,246 descriptions** of the compressor
  and the **4,881 of `main`** every description has exactly such a decode (the compressor's streams: 11 need "left alone", 42 "topped up", a few more than one window).

Verification (everything run, 2026-11-19, `$S2/sw2/` harnesses, deleted afterwards; the generators and the `zbicdiff` example stay in the crate):
* `tests/zbic.rs` against the committed oracle frames (`tests/fixtures/zbic`: 102 frames made by the `zbic` compressor from synthetic data, 366 KB, the SHA-256 of what each decodes to), an NSO with ZBIC
  segments, damaged frames (every fixture with a byte changed at 24 places, 3,000+ variants: error or output, never a panic), unit tests of descriptions (including the window cases) and the real
  `main` (skipped when absent; `IBUILDER_ZBIC_REF=<dir with the Python's main.{text,ro,data}.bin>` compares byte for byte).
* Differential against `zbic.decompress` (`cargo run --release -p ibuilder-nso --example zbicdiff <dir>`; frames and manifests from `tools/gen_zbic_frames.py`, plaintexts: slices of `main` and synthetic):
  fresh seeds, 12,000 clean frames (8,000 slices of `main` 100 B-400 KB, 4,000 synthetic) and 4,000 mutated ones: **identical output on every clean frame, 0 outputs that differ, 0 panics**; the frames
  with a flipped byte: the decoder is stricter than the package where a Huffman stream is not used up exactly (as libzstd; 301 + 77 frames the package decodes), looser on 13 tiny ones.
  Earlier sets: 3,000 + 6,000 + 48 slice frames and 2,500 synthetic ones, same result. The 48 frames that first failed (found by hunting 1.3 M descriptions; they are slices of the game's code, so not
  committed) are the window cases that led to the rule above.
* The real `main` of this game: `load()` passes the three header hashes (flags 0xff = ZBIC + hash bits); compared with the Python `zbic.decompress` byte for byte (text 63,201,744, ro 12,603,520, data 5,217,152 B).

Not verified: other games' ZBIC modules (the rule for the window is a decision procedure, not a reading of the reference); NSO flag bit 6 (set in `main`'s 0xff, meaning unknown to us, ignored as in the Python);
ZBIC-compressed NRO modules (`nn::ro` 23.0.0+: still a clear error); a decoder bug on data that the compressor and `main` never produce (long frames with several blocks of every table mode are covered by the
stock-zstd test and 1,000 large frames; 4 GB frames are not).

## 5. Rewriter: this game's `MATCH_FRAME_STEP`

`patches.py` of `0400486024DEC000`: `MATCH_FRAME_STEP['main']` = one rule (name "frame ratio from the host's step", function of 0x9b4 bytes found by its fingerprint `6ae2aff5...fbcd1d`) with two `others`
(`dynamic resolution` 0x294 bytes, `dynamic resolution update` 0xb24 bytes, found the same way). Before: `MATCH_FRAME_STEP rules of this game profile are code and are not ported to Rust yet`.

* `stubs.rs`: `FsRule` / `FsMatch`, `locate_stub` (a port of `nso_match.locate_stub` for fingerprinted rules: validate nothing at run time, `Image::locate` of the rule and of each other, exactly the Python's log
  lines `frame step "name": matched the function at 0x6e6be0` / `dynamic resolution at 0x8065e0` / `dynamic resolution update at 0x805aa0`, the red lines when not exactly one matches, an `other` that misses is
  left out), `StubCtx::{others, other}`, the `[function match]` suffix of the cut reasons, `has_frame_step` true for a matched build (state page after the key page, patch area 64 KB later when needed),
  and the game's stubs ported by hand: `ocarina_frame_step` (the `fdiv s8,s1,s0` at function +0xe4 gets the host's step x base rate / 60; the original division runs while the host has decided nothing or
  fps_target is 0) and `ocarina_dynamic_resolution` (the level function branches to a 61-word controller assembled with clang in the Python file, its words copied by a script, not typed; capability
  `DYNAMIC_RESOLUTION` declared). Other games' `MATCH_FRAME_STEP` / `MATCH_STUBS` still stop with the "not ported" message.
* **Differential test = the real module.** The three modules of the game rewritten by the Python (`patch_nso.py`, copy of the engine's, `DM_GAME` = the profile, fork pool of 6 for `main`, one Python job at a
  time) and by `ibuilder-rewrite`: `cmp` on `patched.bin`, `fixups`, `sites`, `gates`, `cuts`, `obs`, `changed`, `match` and a `diff` of the printed log: **all identical for `main` (8 files),
  `subsdk0` (7), `sdk` (8)**. `crates/ibuilder-rewrite/tests/ocarina.rs` keeps the SHA-256 and size of every Python output and checks the Rust output against them (skipped without the extracted game):
  `main` at 8 and at 1 threads, `subsdk0`, `sdk`: pass. No synthetic differential exists for these two stubs (the fingerprints are the game's own code, which cannot be committed).

What the rewriter says about this game (`ibuilder-rewrite` stdout, identical to the Python's):
* `main`: text 63.2 MB (0x3c461d0) < 128 MB: **no FAR / BIG_TEXT / hops / GAP / TWO layout**; PATCH_OFF 0x4eb0000 (patch area 2,430,884 bytes), KEY_PAGE_OFF 0x4ea0000, state page `FPS_STATE_OFF` 0x4ea4000 (patched.bin 84,940,708
  B); sites: plain 48,889, cond 861, tpidr_r 1,264, pcrel 541, blr 5, br 94, spin 66, frame_step 2; 66 gates; `cuts` 2 (the two stub sites, main+0x6e6cc4 and main+0x8065e0, both `[function match]`); obs 0;
  capability `DYNAMIC_RESOLUTION`; one SDK-wide function patch is skipped (`Unity 2018 ThreadedStreamBuffer`: 4 functions, 0 matches: not this engine).
* `sdk` (nnSdk, SDK 22.2.8): 2 cuts and 5 obs from the SDK-wide rules (`AddressRegionManager node area 1 GB -> 62 MB` matched, implementation 7 of 9; `AllocateAddressRegion` rule skipped, 0 matches);
  `subsdk0` (NintendoSDK_movie): none.
* `ZBIC`: only `main` (flags 0xff); `sdk` and `subsdk0` are LZ4 (0x3f). Nothing unsupported was met.

## 6. Profile

`ibuilder-profile derive <extract> ... --bundle <engine copy>`: `source: dm/games/0400486024DEC000/game.tsv (registered, not derived)`, 40 rows rendered in 0.29 s. The provisional derivation
(forced with a bundle without the game; `facts` on the real ExeFS loads the ZBIC `main` through `ibuilder-nso`) reports: main build `e0f3ccfaf50065ae4ea01eb5aeb13b1a`, bss 1.3 MB, 1,183 imports, 65 exports; `sdk`
nnSdk 28,698 exports; `subsdk0` NintendoSDK_movie; **SDK 22, system resource 48 MB**, and writes `mem_reserve 0x140000000` (5.0 GiB), `mem_alias 0x118000000` (4.375 GiB), `mem_stack 0x10000000`, `app_pool 0x240000000` ("the sdk is
built for the Switch 2 (spec: Ounce): its 9 GB pool"), `sys_resource 0x3000000`; `romfs_marker Event/Movie/Dm_StaffCredit_0000.webm`; name = the title id (no NACP in the container); `build v0 ?`. It is **not** a
substitute for the registered table, which has what the game needs and a provisional profile cannot know: `mem_reserve 0x320000000` (12.5 GiB), `mem_alias 0x100000000` (4 GiB), `mem_map_limit on`, `fps_target 60`,
`draw_barriers game`, `tex_flush_upload` / `tex_dirty_upload`, `shader_groups /Shader/ /Effect/ /Lib/ /UI/`, `romfs_marker Pack/ZsDic.pack.zs`, `build v131072` (the chain therefore needs the registered `game.tsv` in the bundle).

## 7. Shader split (stage 3, split only)

`shadersplit --romfs RomFS.img --main ExeFS/main --profile <engine games> --game 0400486024DEC000 --jobs 8` (the full shader build was not run): **42,848 shaders** (compute 4,125, mesh 500, pixel 22,352, task 20, vertex 15,851;
sources effect 22,403, shader 14,712, lib 5,566, other 167; 28,178 variation pairs; 4,125 compute answers), 11,662 jobs, **0 unreadable files and no `unsupported container` refusal**, wall **65.5 s**, max RSS 1.18 GB
(peak footprint 1.32 GB), 180 s user. README: "about 42,850 ... including 500 mesh and 20 task shaders": matches. The Rust tables against the engine's registered ones (Python split of the same game):
`shader_facts.tsv` (4,126 lines), `shader_pairs.tsv` (28,179), `pso_pairs.tsv` (6,472) **byte-identical**; `shader_map.tsv` (42,851 lines) identical except its second comment line (the wording of the column 5 note). The 14 `pkg/*.bin`
files (548 MB: ps_shader 270 MB, ps_effect 100 MB, vs_effect 85 MB, ...) have no Python oracle here; the tables derived from the same scan agree.

## 8. The extraction and the whole chain on the real game (numbers)

Mac M1 Pro, 8 cores, 16 GB, shared with other agents (load average 218 at the start of the extraction, 4-6 later: **times are indicative**).

| step | Rust | Python reference |
|---|---|---|
| ExeFS only (5 files, 40 MB) | 0.9 s, files `diff -r`-identical and `shasum`-identical to the Python's (`main` 7db9ddfd..., `main.npdm` 94c154d8..., `rtld` 219f25ef..., `sdk` 3ae3cd4c..., `subsdk0` 0d32c4bd...; the same hashes echora's page lists) | 1.9 s (copy of `pack_nup.py`, `zstd` from Python 3.14) |
| **full extraction** `ibuilder-nup extract` (8 workers, libzstd) | **105.0 s wall**, 24.43 GiB of RomFS (186,583 files) + ExeFS: average **252 MB/s** (814 MB/s in the first seconds, the machine's disk and the other agents' load after), 16.9 s user + 67.4 s sys, **max RSS 129 MB** (footprint 133 MB) | not run (no second copy of 24 GB) |
| container-level verification of all 372,587 files (48.94 GiB, both packages), no write: `ibuilder-nup verify` (4 threads) | **26.2 s**, 133 MB RSS: every file matches its size and SHA-256 | hash-only mode of `pack_nup.Nup.copy` over every file, one thread: **136.0 s**, 48.94 GiB verified, no error (RSS 5.3 GB of mapped pages, footprint 216 MB) |
| repeat run on the extracted folder | `Kept` in 0.2 s | |
| `ibuilder-profile derive` (registered) / `facts` | 0.29 s / 0.8 s, 139 MB | |
| rewrite `main` | 6.1 s (8 threads, 254 MB RSS, 202 MB footprint), 12.2 s (1 thread, 350 MB, 266 MB) | **53.7 s** (fork pool of 6, 6 workers), RSS 979 MB, peak footprint 932 MB, 121 s user |
| rewrite `sdk` / `subsdk0` | 1.4 s / 0.4 s | 15.1 s / 2.4 s (2 workers) |
| shader split | 65.5 s, 1.18 GB RSS | not run (Grizzy's: 13.9-19.7 s for 21,990 shaders) |
| `ibuilder --nup FILE --out DIR` (stage 1 kept, profile, rewrite 3 modules, assemble 109,260,048 B / 42 load commands, sign ad hoc) | **10.1 s**, exit 0, high-water 264 MB RSS | |

Disk: `df -h /System/Volumes/Data` before the extraction 61 GiB available (53 GiB when the task began), at the end 123 GiB (other agents deleted theirs meanwhile). What this package leaves is the one
extraction (`$S2/ocarina/extract/`: `RomFS.img` 26,247,576,430 B, `ExeFS/` 37 MB, `RomFS.img.inputs.json`; 24 GiB on disk); every other intermediate (Python oracle outputs, harness copies of the engine
scripts, decoded ZBIC segments and fuzz sets, test extractions, split output, the build directory) was deleted once verified and recorded here. The container was never modified, moved or copied (size and mtime unchanged).

## 9. What the game needs, and what blocks it on a phone (collected numbers)

* **Memory** (game.tsv, README, the extracted NPDM). `main.npdm`: 64-bit, 39-bit address space, optimized allocation, **system resource size 0x3000000 = 48 MiB** (virtual address memory), main thread
  priority 44 core 0, main stack 1 MiB. `game.tsv`: `mem_reserve 0x320000000` (**12.5 GiB** of address space reserved), `mem_alias 0x100000000` (4 GiB), `mem_stack 0x10000000` (256 MiB), **`app_pool 0x240000000`
  (9.0 GiB, "the Switch 2's 9 GB", as Donkey Kong Bananza)**, `sys_resource 0x3000000`, `mem_map_limit on`; `nninitStartup` asks `SetHeapSize 0x18ce00000` = 6,658,457,600 B (**6.2 GiB**). README, Mac (16 GB): resident
  10.9-11.4 GB, wired about 9.4 GB; 6.4 GB guest memory + 3.8 GB Metal textures and targets + 1 GB IOAccelerator; a render-target pool of 305 MB; footprint on an M6 in the village 3.70-3.80 GB after the fix for
  drawn-over targets (5.35-5.47 GB before). README, iPhone: "the first run (12 GB iPhone ...) was ended for memory about 9.5 minutes in, at about 28 fps, at an area load ... 5.96 GB of room reported at start,
  the jetsam limit's figure"; "No iOS shader build or iPhone run has been done. Its memory use makes an iPhone unlikely as it stands." The repository's own figure for the ceiling the chain must respect is
  ~3.3 GiB (`docs/CONVENTIONS.md`). Our tools fit: extraction 129 MB, rewrite 254-350 MB, split 1.18 GB RSS, but the guest's pool (9 GiB) and its textures do not: **this is the blocker for the phone**.
* **Disk**: `RomFS.img` 24.44 GiB (26.25 GB) plus the 512 MiB margin: 24.93 GiB free on the volume for the extraction alone (the app's Documents), plus the shader library (a Mac library of the 7,276 keys seen in play
  took ~10 minutes to build; the full 42,848-shader library was not built here).
* **Shaders**: 42,848 (500 mesh + 20 task: the shader/finish agent's mesh support was not finished; those return an explicit `unported` today); README: about 2,300 shaders fail with `Attribute offset 0x5C is not valid`
  (a vertex shader's write the translator drops, probably a shading rate); no iOS shader build exists; the full build is a large run (the Grizzy library of 21,990 shaders took 72 s at 6 threads in Rust: this one has twice the shaders
  and Ampere NVN2 code through the C# translator and SPIRV-Cross).
* **Chain**: only the registered `game.tsv` gives the right memory and graphics rows (section 6); the patches are matched by function fingerprints, so only `v131072` is covered (`main` build ID `e0f3ccfa...`; the rule also
  matches any build that keeps the three functions).
* The movies are AV1 (a dav1d / NDVE path in the host), not touched by the toolchain.

## 10. Tests and commands

```
export CARGO_TARGET_DIR=<scratch>/target-sw2
cargo test --release -p ibuilder-nup -p ibuilder-nso -p ibuilder-rewrite -p ibuilder-profile -p ibuilder-mods          # green (see the final report for the counts)
cargo test --release -p ibuilder-cli --test cli                                                                         # 11 tests, 3 for --nup
PATH=/opt/homebrew/opt/rustup/bin:$PATH cargo build --release --target aarch64-apple-ios-sim -p ibuilder-nup -p ibuilder-nso   # ok (also aarch64-apple-ios)
ibuilder-nup extract "<game>.nup" --out <dir>                    # stage 1 alone (writes <dir>/{ExeFS,RomFS.img})
ibuilder-nup verify "<game>.nup"                                  # every file against its SHA-256, nothing written
ibuilder --nup "<game>.nup" --out <dir> --bundle <engine> --host <host.dylib>      # stage 1, then profile, rewrite, assemble, sign
IBUILDER_OCARINA_EXEFS=<ExeFS> IBUILDER_ZBIC_REF=<python decoded segments> cargo test --release -p ibuilder-nso --test zbic the_real -- --nocapture
IBUILDER_OCARINA_EXEFS=<ExeFS> cargo test --release -p ibuilder-rewrite --test ocarina
```
Tests that need the game skip with a message when it is absent. `ibuilder-nup`'s synthetic container writer (`tests/synthetic.rs`: base + update + DLC, two programs, no base, zstd files, damage, cancellation,
space check, reuse) has no game data. Both iOS targets build (`libibuilder_nup.rlib` 1.9 MB with the C libzstd, `libibuilder_nso.rlib` 222 KB); in the booted iPhone 17 simulator (`xcrun simctl spawn`) the `ibuilder-nup` binary
reads the real container (`info`, `extract --exefs-only`: the five ExeFS files with the same SHA-256 as on the Mac) and the `loadnso` example decodes the ZBIC `main` (0.455 s, three hashes ok). Timings in the
simulator are the Mac's CPU, not the phone's.

## 11. Not done / not verified

* A full Python extraction (RomFS.img) for a byte comparison of the images: not made (no second 24 GB copy); the image is compared through the container's own SHA-256 of every file (checked on the way in, and the
  image is read back and checked by `verify_image`'s logic in the tests), and the index/header layout by tests against `read_index`. A `Verified` run of this crate on the Python's image (and the reverse) has not been done:
  the stamps differ on purpose.
* The ZBIC window rule (section 4) is a decision procedure; it has no reference reading. Other Switch 2 titles' modules are not available here.
* `MATCH_FRAME_STEP` anchored rules (`calls` / `find`) and `MATCH_STUBS` of other games are still "not ported": only this game's fingerprinted rule is.
* Nothing ran on a physical iPhone; the iOS runs are in the simulator. The app's callback zstd (`CallbackZstd`) is a trait implementation without an app to plug into.
* The shader pkg files of the split were not compared with a Python split (only its tables); the build of the library was not run (as asked).
* The container's `uuid`, the superblock's reserved bytes and the TLV types other than 2, 3, 10 are read or ignored but not interpreted.
