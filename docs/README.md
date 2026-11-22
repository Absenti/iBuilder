# iBuilder documentation

Start with [ARCHITECTURE.md](ARCHITECTURE.md). Paths are written from the repository root elsewhere.

* [ARCHITECTURE.md](ARCHITECTURE.md): the chain end to end (stages, crates, the app, the engine host, where data lives on the phone, the exported IPA layout).
* [BUILDING.md](BUILDING.md): building and testing, the IPA, fonts, the verify folder, CI, releases and the SideStore / AltStore source.
* [ENGINE.md](ENGINE.md): the engine host, `engine-patches/`, how the host is rebuilt and tested, the `DM_*` switches.
* [DIAGNOSTICS.md](DIAGNOSTICS.md): the app's hidden diagnostics folder and how to fetch it.
* Contributor rules: [../CONTRIBUTING.md](../CONTRIBUTING.md). Security: [../SECURITY.md](../SECURITY.md).

## Reference (one document per part of the chain)

* [reference/pipeline.md](reference/pipeline.md): chain driver, C ABI, Mac CLI, memory design.
* [reference/profile-and-nup.md](reference/profile-and-nup.md): profile derivation and the Switch 2 `.nup` container.
* [reference/rewrite.md](reference/rewrite.md): the NSO module rewriter.
* [reference/shaders.md](reference/shaders.md): stage 3, split to library.
* [reference/shader-split.md](reference/shader-split.md): the shader split step.
* [reference/assemble-sign-pack.md](reference/assemble-sign-pack.md): Mach-O assembler, signer, `.nxgame` packager.
* [reference/mods.md](reference/mods.md): mods and cheats.
