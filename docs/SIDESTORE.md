# Installing and updating with SideStore or AltStore (the source)

## What a source is

A **source** is a JSON file that lists apps: for each one its name, bundle id, icon, and every version with the URL of the IPA, its size, its sha256 and the lowest iOS it runs on.
SideStore and AltStore read it, show the app in their *Browse* tab, install the IPA from the URL, and offer an update when the newest listed version is newer than the installed one.
This repository's source is [`apps.json`](../apps.json); the IPAs themselves are the assets of the [GitHub releases](https://github.com/Absenti/iBuilder/releases), not files in the repository.

## Add it

The source URL is

```
https://raw.githubusercontent.com/Absenti/iBuilder/main/apps.json
```

* **By link** (on the phone, with SideStore or AltStore installed): open `sidestore://source?url=https://raw.githubusercontent.com/Absenti/iBuilder/main/apps.json` for SideStore, or
  `altstore://source?url=https://raw.githubusercontent.com/Absenti/iBuilder/main/apps.json` for AltStore. Both clients handle host `source` with the query item `url` in their own code (SideStore's `URLHandler`, AltStore's `AppDelegate`).
* **By hand**: in the client's list of sources, add a source and paste the URL above.

Then open *iBuilder* in the source's app list and install it.
A source whose app lists no version is refused by both clients, so the source only works from the first release on.

## What the install does to the file

The IPA is **ad hoc signed**: it carries no certificate and no provisioning profile. SideStore and AltStore **re-sign it with your own Apple ID certificate** when they install it, and the kind of certificate decides what iBuilder can
do on your phone: see the table in the README, [Which certificate did you install iBuilder with?](../README.md#which-certificate-did-you-install-ibuilder-with)
The size and sha256 in the source are those of the file **as released**, not of what ends up on the phone after the re-signing.

The source also declares what the app asks for (`appPermissions`): the entitlement `com.apple.developer.kernel.increased-memory-limit` and the photo-library usage text. AltStore refuses an install whose declared permissions differ from the IPA's,
so `scripts/update-source.py` compares them with the IPA whenever it can read the IPA's bytes. Whether the raised memory limit really applies depends on the account type, see the README.

## Check a download

Each release carries `<ipa name>.sha256` next to the IPA, and `apps.json` repeats the same digest. Compare all three:

```bash
shasum -a 256 GameApp-rust-<build>.ipa
python3 -I -c 'import json; [print(v["version"], v["sha256"], v["downloadURL"]) for v in json.load(open("apps.json"))["apps"][0]["versions"]]'
cat GameApp-rust-<build>.ipa.sha256
```

## What a release does to the source (the maintainer's side)

1. `scripts/build-app.sh device` makes `dist/GameApp-rust-<build>.ipa`, its `.sha256` and the `<ipa>.verify/` folder (the verifier that built it). The IPA cannot be built in CI: it needs the prebuilt
   engine host and inputs that are not in this repository.
2. Publish a GitHub release (not a pre-release) whose assets are that IPA, that `.sha256` and, optionally, `<ipa>.verify.tar.gz` (`tar -czf <ipa>.verify.tar.gz -C dist <ipa>.verify`).
3. The workflow [`release.yml`](../.github/workflows/release.yml) runs on `release: published`. On macOS it downloads the IPA, checks it against the `.sha256` asset and the digest GitHub computed for it,
   runs `app/verify_ipa.sh` of the tagged tree (and the shipped verifier, when the release has one) and `scripts/check-no-spawn.sh` on the bundle. Only if all of that passes, it runs
   `scripts/update-source.py` (version, build and minimum iOS from the IPA's `Info.plist`, read with HTTP range requests; the date from the release) and opens a **pull request**
   `release/apps-json-<tag>` that changes `apps.json` only. `main` is protected by a ruleset, so nothing is pushed to it.
4. Review and merge that pull request. Merging is what publishes the update: the clients read `main`.

The same by hand, from the IPA on disk (it reads the size, sha256, version, build, minimum iOS and permissions from the file):

```bash
python3 -I scripts/update-source.py --tag v0.1.0 --ipa dist/GameApp-rust-<build>.ipa
python3 -I scripts/update-source.py --validate --require-versions
```

Things to know:

* **Settings, once**: *Settings > Actions > General > Allow GitHub Actions to create and approve pull requests* must be on, or the workflow can only push the branch (it then prints the link to open the pull request).
  A pull request opened by a workflow does not start the `pull_request` workflows (a GitHub rule): if the required checks stay *Expected*, close and reopen it, or run `gh workflow run ci.yml --ref release/apps-json-<tag>`.
* **A new release must raise `CFBundleShortVersionString`.** SideStore offers an update only when the listed `version` is higher than the installed one, compared as a semantic version (`InstalledApp.hasUpdate` in its source);
  AltStore's documentation asks that each entry differ from the previous one in `version` or `buildVersion`. `update-source.py` replaces the entry that has the same version and build, so a release that repeats the pair of the one before
  replaces it in the source and gives the phones nothing to update to. The IPA's own `Info.plist` is what the source copies: the version has to be raised in the app's build, not in `apps.json`.
* `apps.json` keeps the newest 10 versions, newest first, in the v2 format (`versions[]`). It has no legacy top-level `version` / `downloadURL` / `size` on the app: SideStore reads those only when `versions` is absent.
* Pre-releases are not listed automatically. Run the workflow by hand (*Actions > Release > Run workflow*, with the tag) to list one.
