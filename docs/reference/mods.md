# Mods and cheats (`ibuilder-mods`)

Port of the engine-side `mods.py` (Atmosphere-style mod handling) and the hooks the pipeline calls it from. Library
crate, `#![forbid(unsafe_code)]`; dependencies `sha2` and `ibuilder-nso` (ZBIC decoder); builds for
`aarch64-apple-ios` and `-ios-sim`. The app's importer (UI, zip/folder layouts) is not part of it.

The app keeps a game's mods in `games/<id>/mods/<mod>/{romfs,exefs,cheats}` and the player's choices in
`games/<id>/mods.tsv`.

| Function | Module |
| --- | --- |
| `mods.tsv` state, mod listing, file walk, exefs signature | `store.rs` |
| IPS / IPS32 / RLE, `.pchtxt` (`@nsobid-`, `@flag offset_shift`, `@enabled/@disabled`, `@stop`, comments), build-ID matching | `patch.rs` |
| NSO read, LZ4 / ZBIC segments, patch application, rebuild (stored plain, flags `& ~0xBF`, hashes) | `nso.rs`, `lz4.rs` |
| plan / apply / remove / mark applied over a module set | `chain.rs` (`ModPlan`, `PlanEntry`) |
| cheats folder (`cheats-run/<bid>.{txt,enabled,origin.tsv}`), romfs overlays, boot environment (`DM_ROMFS_OVERLAY`, `DM_CHEATS`) | `boot.rs` (`Environ` trait) |
| Python text and number semantics the logs depend on (`splitlines`, `int(x, 0)`, `repr`, `OSError` messages) | `pyfmt.rs`, `pytables.rs` (generated from CPython 3.14.1, Unicode 16.0) |

Offsets follow the 0x100 header rule (module offset = IPS offset - 0x100). Errors mirror the Python's: a `ModError`
skips the file or leaves the module as dumped; other exceptions propagate to the chain (e.g. an IPS writing into the NSO
header makes the plan fail), and the boot hook catches everything and starts the game without mods. These behaviours are
kept on purpose for byte and log identity.

## Verification

```sh
cargo test --release -p ibuilder-mods -p ibuilder-pipeline -p ibuilder-cli
```

`tests/mods_spec.rs` holds the ported spec checks (each names its Python check in a `// py:` comment); more run through
`ibuilder-pipeline` (`delta_spec`) and `ibuilder-cli` (`mods_chain`). A differential harness (`tools/diff/run.sh`,
feature `diff`, binary `mods_diff`, needs a Python reference environment) compares randomized scenarios, every log
line and the final file tree against the Python; it is a development tool, not part of `cargo test`.

## Known limits

* Only synthetic mods were tested; no real game's mod set.
* Non-UTF-8 file names are skipped; `int(x, 0)` does not accept non-ASCII digits; `repr` uses CPython 3.14.1's
  non-printable table.
* Not run on a physical device.
