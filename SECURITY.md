# Security policy

## Reporting a vulnerability

Report it **privately**, through GitHub's private vulnerability reporting: open the repository's **Security** tab, then
**Report a vulnerability**. Do not open a public issue or pull request for a vulnerability, and do not post it in a
discussion.

This is a small, volunteer-run project: there is no guaranteed response time, but every report is read. Please give
the build id or commit, what you did, what happened, and (if you can) a minimal input that triggers it.

## Never post these, anywhere

Not in an issue, a pull request, a discussion, a log or a screenshot:

* `prod.keys`, `title.keys` or any console key;
* signing certificates and private keys (`.p12`, `.pfx`, `.pem`, `.key`), their passwords, provisioning profiles;
* game dumps or files taken from a game (`.nsp`, `.xci`, `.nsz`, `.nup`, NSO modules, RomFS contents, shader dumps);
* unredacted logs: the diagnostics of the app (`docs/DIAGNOSTICS.md`) contain device names, file names and paths. Remove
  personal information before you attach one.

If you posted one by mistake, say so in a private report or to the maintainer, and rotate the secret (revoke the
certificate, regenerate the keys) first: deleting a comment does not take it back.

## What is in scope

* **The iBuilder app** built from this repository (`app/`, the C# snapshot, its patches and the C glue): handling of
  imported keys and certificates, file import and export, signing, anything that could leak a secret or write outside
  the app's own storage.
* **The Rust chain** (`crates/`): the parsers of untrusted input (NSP / XCI / NSZ / NUP containers, NSO modules, shader
  and profile data), the rewriter, the Mach-O assembler and the signer. A crafted input that corrupts memory, makes the
  app read or write somewhere it should not, or crashes it, is in scope. Hang and out-of-memory reports are welcome as
  ordinary bugs unless they are a practical denial of service.
* **The build and release tooling**: the scripts, the CI workflows, the release artefacts, and the install source
  (`apps.json`) that points installers at them.

## What is out of scope

* The third-party engine (nxnative, NX2Apple) and the libraries listed in [NOTICE.md](NOTICE.md) (LibHac, Ryujinx's
  shader translator, zsign, OpenSSL, SPIRV-Cross, ...): report to their own projects. If a problem appears only in how
  this repository builds or patches them, it is in scope.
* Apple's platform, iOS itself, and the installers (SideStore, AltStore, FlareStore): report to their authors.
* Games' own behaviour, emulation accuracy and compatibility: open an ordinary issue.
* Anything that needs a jailbroken device or an attacker who already controls the phone.
* Piracy: this project does not support it and will not help with it.

## Supported versions

Only the latest release and the `main` branch receive fixes.
