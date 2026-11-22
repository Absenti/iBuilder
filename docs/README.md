# iBuilder documentation

Every document of the repository lives here. Paths are written from the repository root (`docs/...`) everywhere else.
Start with `CONVENTIONS.md`, then `chain/PIPELINE-RUST.md`.

## Top level

These files are referenced by path from the app patches (`app/patches/`), the engine patches (`engine-patches/`), the C sources and the scripts, so they stay where they are.

* `CONVENTIONS.md`: the rules every contributor (human or agent) follows: mission, read-only repositories, secrets, truthful reports, iOS constraints.
* `APP-RUST.md`: the iBuilder app with the Rust chain (no CPython on the phone): how it is built, what was run, what was not verified.
* `CONFORMITY-AUDIT.md`: conformity of the Rust replacement against the latest iOSBuilder, function by function.
* `REVIEW-TRIAGE-2026-11-19.md`: triage of the external code review of 2026-11-19 (finding, evidence, fix).
* `REVIEW-TRIAGE-2026-11-20.md`: triage of the reviews r3 and r4 of 2026-11-20 and of the two performance lever lists (verdict per claim, measured results).
* `UPSTREAM-AUDIT.md`: seedds/nxnative and LNPANDY/NX2Apple against the engine that runs on the phone: a verdict per item and a port order.
* `ENGINE-UPDATE.md`: the host rebuilt with the 11 audited engine patches (`engine-patches/`); what it changes, what is unproven, how to go back.
* `DIAGNOSTICS.md`: the hidden diagnostics folder `Documents/.diag` and how to fetch it.
* `DEVICE-FINDINGS.md`: what the first real-iPhone logs showed.
* `SIDESTORE.md`: the SideStore / AltStore source (`apps.json`) and how a release updates it.

## `chain/`: the whole chain

* `PIPELINE-RUST.md`: the chain driver in Rust (`ibuilder-pipeline`, `ibuilder-ffi`, `ibuilder-cli`).
* `E2E-RUST.md`: the whole chain on Grizzy through the Mac CLI (profile, rewrite, shaders, assemble, sign): commands and measurements.
* `MEMORY-RUST.md`: where the chain's memory goes and how it was cut without changing an output byte.

## `crates/`: one document per stage crate

* `REWRITE-RUST.md`: the NSO rewriter (`ibuilder-nso`, `ibuilder-a64`, `ibuilder-rewrite`); its progress journal is the appendix.
* `MACHO-RUST.md`: `ibuilder-macho`, the linker-less framework assembler.
* `ibuilder-sign.md`: `ibuilder-sign`, the in-process ad-hoc Mach-O signature.
* `ibuilder-profile.md`: `ibuilder-profile`, the per-game profile derived from the dump.
* `ibuilder-pack.md`: `ibuilder-pack`, the `.nxgame` packager and manifest comparator.
* `MODS-RUST.md`: `ibuilder-mods`, Atmosphere-style mods and cheats.
* `SWITCH2-RUST.md`: Switch 2 support (`ibuilder-nup`, ZBIC modules).

## `shaders/`: stage 3 (shaders)

* `SHADER-STAGE-ANALYSIS.md`: call tree, measurements, design and estimate of the on-device shader translation.
* `SHADER-BACKEND-PLAN.md`: the plan of the Rust stage-3 backend and its work packages.
* `SHADER-RUST.md`: `ibuilder-shader`, the chain of ported steps.
* `SHADERSPLIT-RUST.md`: `ibuilder-shadersplit`, the split of the game's shaders.
* `SHADERBUILD-RUST.md`: `ibuilder-shaderbuild`, the build driver.
* `SHADER-FINISH-RUST.md`: `ibuilder_shader::finish`, the per-shader `finish()`.
* `SHADER-TESS-MESH-RUST.md`: tessellation, task / mesh shaders and passthrough geometry.
