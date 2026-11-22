## What and why

<!-- One change per pull request. What does it do, and why? -->

## Checklist

- [ ] **No secrets, no dumps, no game data**: nothing of `prod.keys`, certificates, provisioning profiles, game files, NSO modules, shader dumps, console fonts or unredacted logs is in this change (files, fixtures, docs, screenshots, patch headers).
- [ ] **Tests**: added or updated, and they fail without the change. A reader of untrusted input has a truncation / extreme-value test.
- [ ] **Byte-compare oracle**: outputs for valid inputs are unchanged (byte-identical), **or** the change alters an output on purpose and this description says why, with the updated pinned hashes.
- [ ] **iOS constraints** kept in library crates: no `fork` / `exec` / `std::process::Command`, no macOS-only API, no new `unsafe` without a justification.
- [ ] **Docs** updated (the document of the crate or topic; `docs/README.md` if one was added).
- [ ] **Claims are truthful**: below, what I ran (command and result) and what I did **not** verify. *Measured* and *estimated* numbers are labelled as such.
- [ ] CI is green. I know this needs one approving review, including the code owner's.

## Verification

<!-- Commands you ran and their results, e.g. `scripts/test-report.sh`: "N passed, K of them skipped for missing fixtures". -->

## Not verified

<!-- What this change does not prove (not run on a device, fixture missing on your machine, ...). Write "nothing" only if it is true. -->
