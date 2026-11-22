# Hidden diagnostics (`Documents/.diag`)

Added 2026-11-19 (`app/patches/0050-hidden-diagnostics.patch`, `0052-...`, `crates/ibuilder-pipeline/src/diag.rs`). A folder whose name starts with a dot is not shown by the Files app; nothing here is sent anywhere.

## Fetch it from a Mac (phone on the cable, trusted, developer mode)

```
xcrun devicectl device copy from --device <udid> --domain-type appDataContainer --domain-identifier <bundle id> --source Documents/.diag --destination <folder>
```

The app's own log share (Logs page) also carries the newest three sessions, `previous-run.txt` and the last ten MetricKit payloads (`GameAppLogs.LogFiles`).

## What is in it

| file | written by | content |
|---|---|---|
| `previous-run.txt` | C# (`GameAppDiag.PostMortem`, at the launch that follows) | how the previous launch ended: clean (`exit`), killed in the background, ended in the foreground after memory pressure (jetsam), heat (only when the LAST samples were Serious/Critical), stopped from outside (Xcode / devicectl / an update) or a crash; the lowest `available` memory, the highest footprint, memory warnings, the last 60 events |
| `events-<session>.tsv` | C# | `utc, seconds since launch, phase (launch / build / game), kind, detail`: launch context (build id, shader chain, iOS, `hw.machine`, RAM, entitlements: increased-memory / extended-va / get-task-allow, free disk), `env.txt` / `gameapp.config` / `gameapp.settings` as they were, app lifecycle (`active`, `resign-active`, `background`, `foreground`, `terminate`) each with a memory line, `memory-warning`, `thermal`, `stage` / `stage-progress`, `phase`, EVERY line of the UI log with its time (`ui`), `unhandled` / `unobserved-task` exceptions, the guest's trouble lines (`guest`: `[RED]`, FATAL, panic, SIG*, GPU/Metal errors, stalls; the first 20 of each kind then every 200th), `icon`, `title-from-content`, `metrickit` |
| `samples-<session>.tsv` | C# (every 5 s) | `t, phase, app state, thermal, low-power, free disk MiB, the kernel's memory line (resident, peak, footprint, available)` |
| `pipeline-<session>.jsonl` | Rust (`ibuilder-pipeline/src/diag.rs`) | `chain` (pid, device, kernel, config without secrets, memory), `stage_begin` / `stage_end` (seconds, outcome, memory), `fail`, `stage` (when the step changes and every 30 s), `mem` (every `[mem:tag]` line as numbers), `log` (lines with `[RED]` / FAILED / REFUSED / warning / error / cannot / could not), `env` (the game's diagnostic switches), `boot_begin` / `boot_end` |
| `memstat-<session>.tsv` | the host (`DM_MEMSTAT`) | one line a second while a game runs: footprint, Metal allocation, resident and charged memory by region class |
| `metrickit/*.json` | iOS through MetricKit | crash, hang, CPU and disk-write diagnostics with call stacks; the daily metrics with the exit reasons; the payloads iOS kept from earlier launches |
| `session.live` | C# | exists while a session runs; found at the next launch it means the previous one did not end by `terminate` (the post-mortem) |

The newest six sessions are kept, 48 MB at most; an events file stops taking ordinary lines at 16 MB (lifecycle, stage and memory lines still go in).

## The host's switches turned on for a game (`diag::host_env`)

Only where nothing set them already (a harness, `Documents/env.txt`); `diag_host=0` in `gameapp.config` leaves all of them off: `DM_BOOTPROF=1` (boot timing: mapping, relocation, Metal library, pipelines, uploads, first present), `DM_MEMSTAT=<.diag/memstat-...>`, `DM_PSO_WAITLOG=1` (draws that waited more than 20 ms for a pipeline), `DM_SLOWFRAME=1` (timing of dropped frames), `DM_HITCH_DUMP=2000:20` (no frame for 2 s: every guest thread, once per stall), `DM_FD_LOG=120`, `DM_NVN_CMDSTAT=1`. The guest log (`gameapp.guest.log`) carries their lines.

## Putting a build on a phone that is on the cable

`scripts/usb-install.sh <GameApp.app>` signs a COPY with Xcode's automatic provisioning (bundle id `dev.ibuilder.app.usb`, the team's development certificate, the entitlements already in the executable), installs it with `devicectl` and launches it. It never reads a provisioning profile or a private key. The deliverable `.ipa` stays ad hoc signed for SideStore / FlareStore.
