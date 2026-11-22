# Profile derivation and the Switch 2 container: `ibuilder-profile`, `ibuilder-nup`, ZBIC in `ibuilder-nso`

## What they do

- `ibuilder-profile` derives the per-game profile (`game.tsv`, `profile.tsv` and the empty shader tables) from a
  game's dump (ExeFS, RomFS, NACP), or renders it from a registered `game.tsv`. Replaces the original's
  `profile_derive.py` and `compare_profile.py`, and the parts of the engine's Python they ran in-process
  (`profile_guess`, `nso`, `nso_syms`, `nacp`, `romfs`, `game.py profile`). The app therefore needs neither CPython nor
  the engine's full `dm/games` tree, only the registered `game.tsv` files and the host's hook list.
- `ibuilder-nup` reads a Switch 2 `.nup` container (stage 1) and writes ExeFS, a `DMROMFS1` `RomFS.img`, control data
  and DLC. Replaces the engine's `pack_nup.py` and `romfs_image.py` and the pipeline's `extract_nup`.
- `ibuilder_nso::zbic` decodes ZBIC-compressed NSO segments (NSO flag bit 7, system 22.0.0 and later), which Switch 2
  titles use. Replaces the original's dependency on the `zbic` Python package.

`ibuilder-profile` forbids `unsafe`; `ibuilder-nup` denies it except one `statvfs` call. None starts a process.

## `ibuilder-profile`

### Source to module

`derive.rs` (profile_derive.py), `facts.rs` + `search.rs` (profile_guess facts and byte/regex scans), `guess.rs` (guess,
load order, report), `nso.rs` (NSO load with LZ4 and SHA-256 check, positional reads), `syms.rs` (hook sets),
`nacp.rs` (own small inflate), `romfs.rs` (image, manifest or folder), `game.rs` (`Game`, `render_profile`,
`write_profile`), `compare.rs`. Entry points: `ibuilder_profile::derive::{derive, DeriveConfig}`, `facts_of`, `guess`,
`game::render_profile`; the pipeline uses it through the `ProfileDeriver` adapter.

### How to run

```
cargo build --release -p ibuilder-profile
ibuilder-profile derive <extract dir> [out profile.tsv] [profile_out dir] --bundle <engine> [--romfs <RomFS.img|romfs_manifest.tsv|dir>] [--hooks-file hooks.txt]
ibuilder-profile compare <game.tsv A> <game.tsv B> [<profile.tsv A> <profile.tsv B>]
ibuilder-profile facts <ExeFS> <romfs> --bundle <engine>        # the facts and the guessed keys, as text
ibuilder-profile render <game dir | title id> <out profile.tsv> --bundle <engine>
ibuilder-profile hooks --bundle <engine> > hooks.txt            # the host's HLE hook names, one per line
```

`<engine>` is the engine layout: `<engine>/dm/games/<id>/game.tsv` and, for a provisional profile, the host sources
(`dm/host/*.c`, `dm/gfx/{*.c,nvn/*.c,vk/*.m}`) that the hook scanner reads. On a phone, ship the hook list once and pass
`--hooks-file` (or `DeriveConfig::hooks`) instead of the C sources.

### Design points

- A registered game is rendered from its `game.tsv` (`registered, not derived`); an unregistered one gets a provisional
  profile derived from the facts (SDK version, system resource, heap and address-space rules, engine kind, shader groups, ...).
  A provisional profile lacks what a registered table knows (tuned memory reservations, frame rate, upload switches).
- Output is byte-identical to the original's, which meant reproducing Python semantics (`int(s, 0)`, `{:#x}`, stable
  `Counter.most_common`, regex behaviour on bytes).
- Modules are loaded from positional reads and scanned in parallel; byte scans of the 75 MB `main` use a vectorizable search.
- `depth_mode` (a Mac-side region check in the original) is not derivable and not emitted.

### Verified

```
cargo test --release -p ibuilder-profile
```

- `tests/unit.rs`: hand-built NSOs including every refusal, rules on a small game, the provisional writer, NACP, RomFS image.
- `tests/oracle.rs` runs the original Python as an oracle (needs a reference Python and the engine folder; skips without
  them): the reference game derived two ways is byte-identical in `game.tsv`, the six empty tables and `profile.tsv`, and the
  `compare`, `facts` and hook-list outputs equal the original's.
- `tests/engine_all_games.rs`: every registered game of the engine (179 at the time) renders a byte-identical `profile.tsv`
  through `derive` and `render_profile`; skips without the engine folder.
- Synthetic variants (`tests/oracle/gen_variants.py`, random ExeFS and RomFS manifests): about 95% byte-identical, the rest
  refused by both sides, none different.

Measured on 2026-11-19 on a shared M1 Pro (median of 5, `/usr/bin/time -l`): deriving the reference game (75 MB `main`,
RomFS image, NACP) took 0.30 s and 260 MiB peak RSS, against 3.99 s and 798 MiB for the original script.

### Known limits

- NRO modules are not loaded (ExeFS NSOs only). Functions the derive path never calls (`main_modules`, `nro_modules`,
  `shader_sources`, `region_findings`, `check`, ...) are not ported.
- `kind()` ties pick the first in sorted order where the original is arbitrary (only a "not loaded" message differs);
  NACP `isprintable()` is approximated; a RomFS folder is walked in sorted order; invalid UTF-8 in manifest names is
  replaced, not surrogate-escaped; the hook scanners are hand-written.

## `ibuilder-nup`

### The container (as implemented)

Little-endian. A 0x80-byte superblock (`NUP`, TOC offset/size/SHA-256, data-blob offset, file size, package count), a TOC (package, folder and file records, child-slot array, name pool), per-package meta
blocks (TLV: 2 NACP, 3 icon, 10 localized icons). Package kinds: 0 base, 1 update, 2 add-on, 3 add-on update, 4 system. File
records carry size, extent offset and size, SHA-256 and codec (0 raw, 1 one zstd frame). Extents are 16-aligned and may be
shared between packages. Not encrypted.

All structural checks of the original reader are kept with its messages. Added on purpose: a TOC above 1 GiB is refused,
and a folder listed more than once (two parents, or containing itself) is refused in the TOC and the `DMROMFS1` index;
the original walked shared folders once per path, which is exponential. No real container does this.

`scan` finds the programs (program id in each package's `main.npdm`), groups them into one application (NACP save-data
owner id, else the id above the low 12 bits) and picks one: by default the highest program index (a Switch 2 Edition's
Switch 2 program) and the newest update (`--version`, 0 = base). No base is accepted only with standalone updates.

### Output layout

```
<extract>/ExeFS/                  the selected version's files (main, main.npdm, rtld, sdk, subsdk0), kept when every file already has the container's SHA-256
<extract>/RomFS.img               DMROMFS1: 32-byte header, files in breadth-first order, the index (24-byte entries + names) last; written as RomFS.img.part, then renamed
<extract>/RomFS.img.inputs.json   this crate's stamp (source digest and the image's inode, size, mtimes)
<extract>/romfs_programs/<index>.img, control.nacp, icon.jpg    when the container has them
<games>/<title id>/aoc/<index>.img                              each DLC (the CLI passes Documents/games/<id>/aoc)
```

### Design points

- No mmap of a 25 GB file: positioned reads through the `ReadAt` trait (`FileSource`, `MemSource` for tests).
- Every file's offset in the image is known from the container's sizes before the first byte is written, so rayon
  workers `pwrite` each file at its place and the output is identical for any thread count. Memory stays near
  130 MB with 8 workers.
- Each file's size and SHA-256 are checked as it is copied; `ibuilder-nup verify` checks all files without writing.
- Reuse: `Kept` (stamp matches), `Verified` (another packer's image is checked against the hashes, then stamped), `Packed`.
- Free space needed: image size plus 512 MiB, unless the image is already there at the right size.
- Progress, the original's `say` lines and cancellation are `Options` (a cancelled extraction leaves no image and no `.part`);
  `RustNup` maps them to the stage line and the pipeline's cancel flag.
- zstd decoding is behind `ZstdBackend::decode_frame` (streaming, one frame per file; trailing bytes or a cut frame are
  errors): `NativeZstd` (C libzstd, feature `native-zstd`, default, builds for iOS), `Ruzstd` (pure Rust, `--ruzstd`), and
  `CallbackZstd` for a host binding. Measured on an M1 Pro in 2026-11 (`examples/zbench.rs`, one thread): game code
  decoded at 628 MB/s with libzstd against 203 MB/s with ruzstd.
- The game's own `.zs` files inside the RomFS are copied as bytes; the container needs no dictionary.

### How to run

```
cargo build --release -p ibuilder-nup
ibuilder-nup info FILE
ibuilder-nup extract FILE --out DIR [--aoc DIR] [--version N] [--program N] [--threads N] [--exefs-only] [--ruzstd]
ibuilder-nup verify FILE                  # all files against their SHA-256, no output written
ibuilder --nup FILE --out DIR ...         # stage 1 inside the whole chain (docs/reference/pipeline.md)
```

### Verified

```
cargo test --release -p ibuilder-nup
```

`tests/synthetic.rs` builds small containers in memory: programs, base and update handling, DLC, refusals of the reader,
identical images across thread counts and both zstd backends, reuse and cancellation. `tests/romfs_damaged.rs` covers damaged `DMROMFS1` images.
Against the real container (25.6 GB, 372,587 raw files; not in the repository), on an M1 Pro in 2026-11 (shared machine,
indicative): ExeFS files byte-identical to the original's; full extraction 105 s with 8 workers (24.4 GiB RomFS) at 129 MB
max RSS; `verify` 26 s against 136 s for the original's hash-only mode; repeat run `Kept` in 0.2 s; the whole chain from
the container 10.1 s.

### Known limits

- This container had no NACP and no icon, so `control.nacp` and `icon.jpg` are not written for it (as in the original).
- A stamp written by the original's packer cannot be reproduced: its image is `Verified` once, then `Kept`.
- Only one reference container was available; other layouts (several programs, DLC) have synthetic tests only.

## ZBIC in `ibuilder-nso`

`ibuilder_nso::zbic::{decompress_into, zstd_decompress_into, Decoder}`: a zstd (RFC 8878) decoder written from the
specification, about 800 lines, no C and no dependency: raw/RLE/compressed blocks, Huffman literals (1 or 4 streams,
treeless reuse, direct and FSE-compressed weights), FSE sequence tables (predefined, RLE, compressed, repeat), repeat
offsets. No dictionary; the output is one buffer; the content checksum is skipped (the NSO header's SHA-256 is the
check). A `Variant` switches the magic and the table-description reader, so the same machinery runs as stock zstd for
testing. `ibuilder_nso::load` calls it for flag bit 7; `ibuilder-profile`, `ibuilder-rewrite` and `ibuilder-mods` load
through the same function. A ZBIC segment is bounded to 32768 output bytes per input byte.

Why its own decoder: a ZBIC frame has magic `ZBIC` and a different FSE table-description coding, so neither libzstd nor
ruzstd reads it. Decoding the 81 MB `main` of the reference Switch 2 game with its three SHA-256 checks took 0.38-0.45 s
(measured 2026-11), against 3.0 s for the Python package.

### Format, as measured

A ZBIC frame is a zstd frame with magic `ZBIC`; headers, block headers, literal sections, Huffman and sequence
bitstreams, predefined tables and offsets are stock. Only the table descriptions (the three sequence tables of a block, and
the table of an FSE-compressed Huffman weight list) differ. Derived by measurement with the reference compressor as an
oracle (`crates/ibuilder-nso/tools/gen_zbic_frames.py`), comparing with stock zstd's counts for the same data:

- Byte 0: low 7 bits plus 1 is the description's length (byte 0 included); bit 7 says low-probability symbols (count -1)
  may be present. The other bytes are read last to first into a 64-bit state, topped up one byte at a time while it is
  below 2^40; digits come out as `state % radix`.
- Digits: `m - 1` (radix 52, `m` the largest symbol), `log - 5` (radix 8), `F - 1` (radix `2^log`), then the walk: the
  cumulative counts coded by binary interpolative coding in right-first preorder. The state must end at 0 with every byte used.
- The top-up window: whether a byte is appended when the state is in `[2^40, 2^40 + 2^24)` depends on the compressor and
  cannot be read from the stream. The decoder decodes without a top-up there and, if the table is invalid, tries the other
  choices (at most 2^10). All 1,324,246 descriptions from the reference compressor and the 4,881 of the real module decode.

### Verified

```
cargo test --release -p ibuilder-nso
```

- `tests/zstd_machinery.rs`: the stock-zstd variant against libzstd on 140 frames (five kinds of data, four levels, 1 B
  to 1 MB).
- `tests/zbic.rs`: 102 committed oracle frames (`tests/fixtures/zbic`, made by the reference compressor from synthetic
  data), an NSO with ZBIC segments, damaged frames (about 3,000 byte-changed variants: an error or an output, never a panic),
  window cases, and the real module's segments (skipped when absent; `IBUILDER_ZBIC_REF=<dir>` compares them byte for
  byte).
- A differential run against the reference decoder (`--example zbicdiff`): 12,000 clean frames identical, no panic on 4,000
  mutated ones. The real module's three segments equal the reference decoder's output byte for byte.

### Known limits

- Only one game's ZBIC modules were available. The window rule is a decision procedure found by measurement, not a
  reading of a reference.
- ZBIC-compressed NRO modules are still refused with a clear error.
