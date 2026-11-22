# Third-party notices

iBuilder's own code is under the MIT license (see [LICENSE](LICENSE)). This file lists what else the project uses or
embeds, what for, and under which license. It is not legal advice: each upstream's own license text is authoritative.
Where this repository cannot verify a license, the line says **check upstream** instead of guessing.

## What is NOT in this repository, and never will be

* No Nintendo code, no keys, no firmware, no fonts, no game data, no shader dump, no decrypted content.
* Users bring their own game dump and the `prod.keys` of their own console. The project never downloads any of it.
* No code of LiveContainer (AGPL-3.0): `app/c/inapp_shim.cpp` is written against zsign's own headers, as its header says.
* "Nintendo" and "Switch" are trademarks of Nintendo. This project is not affiliated with, authorised or endorsed by
  Nintendo or Apple.

## 1. The emulation engine the app embeds

The engine is third-party. Its source is **not** in this repository: the app embeds a prebuilt, loadable build of it (the
"host", `host-prebuilt.dylib` in the IPA) to which the user's game is attached on the phone.
`scripts/build-host.sh` rebuilds that host from an engine checkout given as an input.

| Component | Author | License | What it is used for here |
|---|---|---|---|
| nxnative (<https://github.com/seedds/nxnative>) | seedds | MIT, Copyright (c) 2026 seedds | The engine: ARM64 host, HLE services and the Metal graphics layer. Verified from the LICENSE file of the pinned engine tree. |
| NX2Apple (<https://github.com/LNPANDY/NX2Apple>) | LNPANDY and contributors | MIT (its license names seedds, jhaizhou, LNPANDY and NX2Apple contributors; see `docs/UPSTREAM-AUDIT.md`) | The pinned engine tree is nxnative plus NX2Apple's patch queue. |
| `engine-patches/*.patch` | upstream authors (0401 to 0411), this project (0412) | MIT; each header names the upstream commit, its author and its license | Patches applied to the engine tree by `scripts/build-host.sh`. The attribution lines of every upstream patch are kept as the license asks. |

Parts of the engine tree that the engine's own documentation (`dm/third_party/README.md` of the pinned tree) lists as used
by the host: dav1d (BSD-2-Clause), libvpx (BSD-3-Clause, with its patent grant), stb_vorbis (MIT or public domain),
bcdec (MIT or Unlicense), Vulkan-Headers (Apache-2.0 or MIT), code adapted from MacNativeGames (MIT), a texture layout
algorithm ported from deko3d (zlib license). Which of them end up in the host binary you use: **check upstream**.

## 2. The .NET application (what the IPA bundles)

The app is a .NET iOS application (`app/gameapp/`, C#); the IPA therefore contains the .NET runtime. The conversion chain
itself (rewrite, assembly, signing, shader steps) is Rust and uses no Python.

| Component | License | What it is used for here |
|---|---|---|
| .NET runtime (CoreCLR, base class libraries) and the iOS workload (Microsoft.iOS) | MIT (Microsoft). The runtime pack carries its own third-party notices: **check upstream** for the exact file of your SDK version | Runs the app's interface and the engine's extractor and shader translator, as bundled `System.*.dll`, `libcoreclr.framework`, `Microsoft.iOS.dll` |
| LibHac 0.19.0, Alex Barney | MIT (NuGet metadata) | Reads NSP / XCI / NCA containers with the user's own `prod.keys` (the engine's extractor `nx_extract` and `app/gameapp/GameAppIcons.cs`) |
| Ryujinx 1.3.2 (`Ryujinx.Graphics.Shader`, a subset of `Ryujinx.Common`, `Spv.Generator`), Ryujinx Team and Contributors | MIT (the LICENSE file of the engine's `dm/third_party/ryujinx`) | The Switch-to-SPIR-V shader translator `dmsh_xlate`, compiled into the app with the engine's local changes listed in that folder's README. `app/gameapp/xlate/XlateCommonShim.cs` is a small stand-in written in this project for one `Ryujinx.Common` class |
| SSH.NET 2026.0.1, Renci | MIT (NuGet metadata) | Used by the app's network storage code (`app/gameapp/GameAppNas.cs`) |
| BouncyCastle.Cryptography 2.7.0, Legion of the Bouncy Castle Inc. | MIT (NuGet metadata) | Pulled in by SSH.NET, bundled as `BouncyCastle.Cryptography.dll` |
| Microsoft.Extensions.Logging.Abstractions | MIT (Microsoft): **check upstream** | Pulled in by SSH.NET, bundled |

`app/gameapp/` is a pinned snapshot (`app/SNAPSHOT.tsv`) of the app's C# sources, changed only by `app/patches/*.patch`.
**License of the snapshot: TO BE CONFIRMED BY THE MAINTAINER** (the repository's MIT license covers it only if the
maintainer holds the rights).

## 3. Native libraries linked into the app (build inputs, not stored here)

| Component | License | What it is used for here |
|---|---|---|
| zsign (zhlynn), a locally patched copy | MIT (the LICENSE file of the pinned copy) | Signs what the app builds, in process (`app/c/inapp_shim.cpp` is this project's adapter) |
| OpenSSL, as the OpenSSL-Universal xcframework | Apache-2.0 for OpenSSL 3.x (older versions: the OpenSSL and SSLeay licenses); the version inside the pinned xcframework: **check upstream** | Cryptography for zsign, embedded as `OpenSSL.framework` |
| zlib / minizip | zlib license: **check upstream** for the copy the build uses | Dependencies of zsign |
| SPIRV-Cross (Khronos), pinned `f11ba9f` | Apache-2.0 (the LICENSE file of the pinned copy; its `LICENSES` folder also lists the Khronos free-use license, MIT and CC-BY-4.0 for some files) | SPIR-V to Metal Shading Language; static library driven in process by `app/c/spvc_cli.c` |
| zstd (Meta) | BSD-3-Clause option (zstd is dual-licensed BSD / GPL-2.0; the BSD option is the one used) | Standard zstd decoder (`app/c/stdzstd.c`) for Switch 2 packages |

`app/c/nsz_decompress.c` is this project's own converter from NSZ back to NSP, written from the NCZ layout that the
`nsz` tool defines (as its header says).

## 4. Rust dependencies

All 17 workspace crates are MIT (`license.workspace = true`, checked in every `crates/*/Cargo.toml`). `Cargo.lock` pins
61 external packages. This table groups the licenses read from `cargo metadata --format-version 1 --filter-platform
aarch64-apple-darwin` and `--filter-platform aarch64-apple-ios` (54 packages), plus the five host-only build packages
read from their `Cargo.toml` in the local registry (proc-macro2, quote, serde_derive, syn, unicode-ident). Run
`cargo metadata --format-version 1` again to regenerate it; a dependency update can change it.

| License expression | Packages | Crates |
|---|---|---|
| `MIT OR Apache-2.0` | 38 | block-buffer, cc, cfg-if, cpufeatures, crc, crc-catalog, crc32fast, crossbeam-deque, crossbeam-epoch, crossbeam-utils, crypto-common, digest, either, find-msvc-tools, flate2, hashbrown, hybrid-array, itoa, jobserver, libc, memmap2, pkg-config, proc-macro2, quote, rayon, rayon-core, regex, regex-automata, regex-syntax, serde, serde_core, serde_derive, serde_json, sha2, shlex, syn, tar, typenum |
| `MIT` | 7 | capstone-sys, lz4_flex, lzma-rs, ruzstd, simd-adler32, twox-hash, zmij |
| `Apache-2.0 OR MIT` | 3 | const-oid, equivalent, indexmap |
| `Unlicense OR MIT` | 3 | aho-corasick, byteorder, memchr |
| `BSD-3-Clause` | 2 | zstd, zstd-safe |
| `(MIT OR Apache-2.0) AND Unicode-3.0` | 1 | unicode-ident |
| `0BSD OR MIT OR Apache-2.0` | 1 | adler2 |
| `BSD-3-Clause AND BSD-2-Clause` | 1 | zstd-sys (bundles the zstd C library) |
| `MIT OR Zlib OR Apache-2.0` | 1 | miniz_oxide |
| `MIT/Apache-2.0` (old spelling of MIT OR Apache-2.0) | 1 | filetime |
| `Zlib` | 1 | foldhash |
| not readable offline: **check upstream** | 2 | getrandom 0.4.3, r-efi 6.0.0 |

**Copyleft check.** None of the 59 licenses read is GPL, AGPL, LGPL or MPL, and every expression offers a permissive
option. The two packages that could not be read are `getrandom 0.4.3` and `r-efi 6.0.0`. Neither is in the dependency
graph of `aarch64-apple-darwin` or `aarch64-apple-ios` (`r-efi` is for UEFI targets, reached through `getrandom`).
**Check their license upstream**, in particular whether `r-efi` offers an LGPL option next to a permissive one, before the
first release.

`capstone-sys` (MIT, the Rust binding) compiles the bundled Capstone C library (BSD-3-Clause; its LLVM-derived files
carry the University of Illinois/NCSA license, see `LICENSE.TXT` and `LICENSE_LLVM.TXT` in the crate). `zstd-sys`
compiles the zstd C library (BSD).

## 5. Fonts

The console's shared system fonts belong to Nintendo and are **not** included, distributed or derived here.
A game that waits for a system font does not boot, so `scripts/build-app.sh` makes stand-ins at build time, with the
engine's `make_shared_fonts.py` (fontTools, MIT), into the bundle's `dm/font/` folder:

* the five text cuts (`standard.ttf`, `chinese_simplified.ttf`, `chinese_simplified_ext.ttf`, `chinese_traditional.ttf`,
  `korean.ttf`) are unhinted subsets of ONE source font. By default that is **macOS's Arial Unicode, a proprietary
  font (Monotype / Apple)**. `STD_FONT=<path to a .ttf>` names another source;
* `nintendo_ext.ttf` (controller-button icons) is drawn by the engine's `make_ext_font.py`; the letters inside its icons
  are outlines taken from **macOS's Arial Rounded Bold, also proprietary**. `STD_FONT` does not change that source (the
  engine's script passes it to the text cuts only).

Consequences:

1. A **private build** made with the defaults embeds fonts derived from proprietary ones. It is for the person who built
   it, on their own phone. It must **never be redistributed**, and neither must an IPA exported from it.
2. A **published release IPA** must be built with freely licensed source fonts only.

Published release IPA font source: TO BE FILLED BY THE MAINTAINER

Published release IPA button-icon letter font: TO BE FILLED BY THE MAINTAINER
