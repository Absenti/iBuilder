# Triage of the external reviews of 2026-11-20 (r3, r4) and of the two lever lists

Inputs (written by another model, outside this repository): review r3 `iBuilderRust-review-2026-11-20.md`, review r4 `iBuilderRust-review-2026-11-20-r4.md` (a delta over r3), its companion
`iBuilderRust-optimisations-2026-11-20.md`, and the lever lists `iBuilderRust-accelerations-2026-11-20.md` / `iBuilderRust-leviers-detail-2026-11-20.md` (built on an Ocarina of Time device log).
Method as in `REVIEW-TRIAGE-2026-11-19.md`: every claim re-read in the code; a behaviour change gets a test; perf changes are proven output-neutral (before/after binaries on the same inputs).
Verdicts: **CONFIRMED** (fixed), **PARTIAL**, **REJECTED** (evidence), **OPEN** (true, not fixed: why). Nothing here ran on an iPhone; no IPA was built. A reboot on 2026-11-21 wiped the scratch
folder, with the only Mac copy of the extracted Ocarina game: the real-game identity checks taken before it are noted as such.

## 1. Correctness

| Id | Claim | Verdict | Action and evidence |
|---|---|---|---|
| r4 3.1 / 4.3 | the rewrite suite is red: the synthetic manifest was never regenerated, two generators write two formats | **CONFIRMED** | one writer (`tools/synth_manifest.py`), manifest regenerated from both Python oracles; suite green (29 passed) |
| r3 3.2 | the rewriter differs from the shipped Python on jump tables | **CONFIRMED, FIXED (R6)** | `jump_solver.py` ported (`ibuilder-rewrite/src/jump_solver.rs`, set order `pyset.rs`, integration `scan.rs`; older recognisers removed): a 32-bit guard on a 64-bit index now stops the module like the shipped Python (`jump table N: the index reaches 0xffffffffffffffff`); manifest regenerated, 21/21 synthetic cases at parity with the shipped engine (17 files, 4 stops), 0 mismatches on 7,000 `tools/jsfuzz.py` modules (1,550 with landing pads) + jt/tfuzz/Grizzy-chunk corpora; Grizzy and Ocarina identical to their oracles; cost: Ocarina main +0.8 s (1 thread) / +0.4 s (8), +70 MiB peak at 8 threads |
| r3 3.3, 5.19, 5.20 | bounded reads of the dynamic table; dead code | **CONFIRMED** (fixed earlier, verified) | regression test passes |
| r3 3.1 / r4 2.1, 2.2, 4.1 | a stage-3 failure is reported as success, the game boots without a shader library, the app says "Ready" | **CONFIRMED** | new rc 4 `IBUILDER_RC_NO_SHADER_LIBRARY`: no boot (build and boot-only), app patch 0078 shows the failure; 15/15 mutations caught; assemble/sign failures get their own code (5) |
| r4 4.1 | `install_pack` has no recovery of `<id>.old` after a crash and can delete the only copy | **CONFIRMED** | `<id>.old` restored at the next start, never deleted when it is the only copy |
| r4 4.1 | two writers still use a fixed `.tmp` name (`timings.rs`, `profile.rs`) | **CONFIRMED** | per-writer temp names |
| r4 4.4 | the r3 4.2 guard of `unity::read_node` bounds one array, not a product (3.8 KB -> 600 k leaves) | **CONFIRMED** | a global budget per decode (8 values per byte + 4,096); the probe is refused; shipped layouts stay under 1.5 values per byte |
| r3 4.8, 4.13, 5.10-5.13, 5.21-5.25 | shader crate items | **CONFIRMED** (11 of 12 were in place; verified) | plus two bugs found on the way: a test that panicked instead of skipping, and `finish/unit.rs` slicing past a shared entry body (fixed, with a test) |
| r4 4.2 | ZBIC branch allocates the declared size; a `.last()`; DAG divergence undocumented | **CONFIRMED / REJECTED / CONFIRMED** | ZBIC buffer bounded by the input (3 bytes announcing 2 GiB refused); the `.last()` is gone; `docs/crates/SWITCH2-RUST.md` documents the DAG refusal |
| r4 7.1-7.3 | `app/c`: NCZ `offset + size` overflow, `spvc_cli` silent truncation, 2 x 256 MB allocations before validation | **CONFIRMED** | overflow refused, failures propagate to rc 1, header validated before allocating; host tests in `app/c/test/` (real NCZ files not available: the compressed-block path is reasoned, not run) |
| r4 3.2, 4.5 F2-F5 | spawn gates: two different symbol lists, `_forkpty`/`_daemon` pass, no symbol-count guard per Mach-O, 0644 Mach-O skipped, single-file answer differs | **CONFIRMED** | one shared list (`scripts/lib-spawn.sh`), Mach-O found by content, guard on every Mach-O, single file exits 2 with the reason |
| r4 3.3 | the PASS count of the shipped IPA is not reproducible | **PARTIAL** | the current script gives 194, as the MANIFEST says (not 195); the MANIFEST now records the verifier's sha256 next to the count |
| r4 4.5 | 0077: `Exports` case-sensitive, `Subfolders` counts attempts, cap wording | **CONFIRMED** | fixed in 0077 |
| r4 1 / 4.5 | CI red: `fmt` and `clippy -D warnings` | **CONFIRMED** | lints fixed crate by crate; the workspace `cargo fmt` is the last step before the next build (see `STATE-2026-11-20.md`) |

## 2. Performance (output-neutral; measured on this Mac, indicative under load)

| Item | Verdict | Result |
|---|---|---|
| r4 6.2 `check-forbidden-files.sh` quadratic substitution | **CONFIRMED** | 1:35 -> 12.7 s wall (C locale; the 13:41 of the review was under fr_FR.UTF-8), identical output |
| r4 6.3 / optimisations 3: regex per call (`ins.rs full`), allocations per decoded instruction (`a64 cs.rs`), clones in `matching.rs` | **CONFIRMED** | Grizzy `main`: 9.62 -> 7.60 s (1 thread), 4.27 -> 2.45 s (8); byte-identical on 129 fixtures, 860 generated modules and Grizzy |
| `fde_ranges` / `collect_got` re-parsed 6-9 times | **REJECTED** | 4 x 5 ms and 1 call on real modules: nothing to gain |
| `rewrite()` re-runs the phase up to 3 times | **NOT REPRODUCED** | it never happens on the modules available |
| shader regexes in loops (`entry.rs`, `fetch.rs`, `reflect.rs`, `unit.rs`), shadersplit sort/allocations | **CONFIRMED** | `int_inputs` 4 ms -> 0.03 ms per vertex shader; 33,390 texts identical; a full Ocarina stage 3 stays 473 s (translator + SPIRV-Cross are 86-89 % of it) |
| pipeline I/O: 3 cache walks, 5 ms memory meter, `statvfs` per wait, full log reads, per-line stage rewrite and log allocations | **CONFIRMED** | 1 walk instead of 2-3, 1,410 samples instead of 28,200 per build, 20 us instead of 31-66 ms for a log tail, 142 stage writes instead of 282 |
| `build-app.sh` spawns; FFI static library rebuilt every time | **CONFIRMED** | crate fingerprints 4.17 -> 0.49 s (identical hashes); the iOS library is reused on an exact cache-key match (`FFI_CACHE=0` disables) |

## 3. The lever lists

| Lever | Verdict | Why |
|---|---|---|
| 11 finish porting the finishing (809 -> 80-150 s) | **REJECTED** | the numbers are the original project's Python profile; here stage 3 is all Rust already |
| 12 two-level translation cache | **PARTIAL** | implemented (`tool_memo`, tested) but OFF: the translation cache is deleted after each build by design (thousands of small files slowed the app's launch), so it would not survive anyway |
| 13 SQLite + three image writes | **REJECTED** | does not exist in the Rust chain |
| 2 archive "0 hits" (sweep deletes it) | **REJECTED as stated, real cause found** | the `0` counter is the Metal 4 archive, unused on iOS; the real archive (paches/0009) saved nothing at SIGTERM, dropped 451/472 pipelines when hot, died on one error, never kept mesh: engine patch 0414 |
| 5 `pso_user.tsv` stays at 2 lines | **REJECTED as stated** | written as designed (only what the pack list lacks) and read after it: 0413 records every requested line and reads it first |
| 1 tail on one utility worker, killed with the process | **CONFIRMED** | 0415: the tail resumes where it stopped and waits while hot |
| 14 compaction | **REJECTED** | exists only for the Metal 4 archive |
| 3 write-protection faults / mprotect | **REJECTED** | the 36,661 is cumulative; mprotect is 0.73 s over the 44 min session |
| 4 `DM_QOS_MAP=1` frees the P-cores | **REJECTED** | the busy guest threads are priority 44, which stays user-interactive |
| the 161 s frames | **CONFIRMED, cause in this repo** | the app forced `DM_PSO_NOWAIT=0`, which disables the engine's 50 ms per-frame budget: for a seed list over 10,000 lines (Ocarina) the host's policy is now kept (`profile.rs` `set_pso_nowait`) |
| 8, 9 RT hysteresis, residency | **NOT TAKEN** | keeping more render targets raises the jetsam risk at 4.2 GB; residency follows the RT churn |

Engine patches added on this occasion: 0413-0415 (above), 0450-0462 from nxnative upstream (0459, Ocarina's 8 GiB alias, kept in `engine-patches/optional/` for a decision), 0500-0505 adapted from
the MIT port kid-with-sword-remake (texture memory saver and pool skip off by default, stall report on). See `ENGINE-UPDATE.md`.

## r5 (2026-11-22): `iBuilderRust-problemes-2026-11-22.md`

Third-party review r5, problems and opportunities only. Each claim re-read (and reproduced where cheap) before a change; a change gets a test that fails without it. Rewriter and shader outputs
are unchanged (oracle tests below). App-side items (C# in `app/patches/`) are handled in app patch 0093 by another agent; CI by the repository agent. Nothing ran on an iPhone; no IPA was built.

| Id | Claim | Verdict | Action and evidence |
|---|---|---|---|
| P1 | `markers::note_boot_result` reads a 16 KiB tail of the `O_APPEND` guest log: a refusal further back is missed, the "starts again" loop returns | **CONFIRMED, FIXED** (and a worse bug found) | the last boot's section is read from its end (`util::rfind_line`) back to its `==== guest output, pid` banner (`markers::last_guest_run_refused`): seen however far from the end, an earlier boot's refusal no longer counts. Found on the way: in the Rust build the `[gameapp] dlopen FAILED: …sandbox blocked mmap()` line went to the chain log ONLY (the C shim's `logln` also wrote stderr = the guest log), so the record was never written; `sys/boot.rs` now writes it on fd 2 too (`redirect::write_fd2`). Tests: `pipeline_spec::the_sandbox_refusal_is_found_anywhere_in_the_last_boot_and_only_there` (the refusal about 41 KB from the end of a 117 KB log; an older boot's refusal ignored), `redirect` test (the line reaches the log). Device check still due |
| P2 | `spvc_cli.c --output`: a failed open or write returns 0 | **CONFIRMED, FIXED** | `write_output`: rc 1 on open failure, on a short `fwrite`, `fflush`/`ferror`/`fclose` failure (message: upstream's + the reason). Upstream exits 0 there: a deliberate divergence, only on failure paths; the bytes written are the same (`fwrite` of the text). Test `app/c/test/spvc_cli_test.c` case 6: whole write, a folder as path, a short write under `RLIMIT_FSIZE` 1 KiB |
| P3 | 0090 `RemoteReadStream` caches a short block | app patch 0093 (other agent) | |
| P4 | `lib-spawn.sh` `nm -u` reads only the host slice of a universal file | **CONFIRMED (reproduced), FIXED** | `nm -arch all -u`, slice headers skipped; thin files unchanged. `scripts/test-lib-spawn.sh` (clang + lipo: clean arm64 + `_popen` arm64e) fails with the old line and passes with the new one; the shipped bundle still gives the same result (591 imports, 3 allowed) |
| P5 | `list_macho` `find -type f` skips symbolic links | **CONFIRMED (reproduced), FIXED** | `find -L` (loops reported by find and not walked twice, dangling links skipped). Test: a bundle with 2 Mach-O files, a link to a third outside it, a loop and a dangling link: 3 listed, the linked file's `_popen` seen |
| P6 | a `$`-suffixed Darwin variant (`_popen$DARWIN_EXTSN`) is neither forbidden nor allowed | **CONFIRMED (made with an asm label), FIXED** | the name is compared without what follows its first `$` (`SPAWN_ERE` too); Objective-C `_OBJC_CLASS_$_X` names reduce to prefixes never in the list. Test in `scripts/test-lib-spawn.sh` |
| P7 | free-space gates use the optimistic "important usage" value | C#: app patch 0093 (other agent). Rust gate (`shaders.rs`): **DECIDED, kept** | owner/main-agent decision: important usage is Apple's API for user-initiated work and the device showed statvfs 24 GB vs Settings > 100 GB. Verified: `sys::free_bytes` = max(important, strict), falls back to strict when important usage cannot be read (never 0), -1 = gate skipped; both values logged. Comment added at the gate; no behaviour change |
| P8 | 0087 `previous.Join()` without timeout | app patch 0093 (other agent) | |
| P9 | 0088 abandoning a paused rebuild deletes the framework | app patch 0093 (other agent) | |
| P10 | no automatic gate on Capstone's text | **CONFIRMED, FIXED** | `crates/ibuilder-a64/tests/cs_golden.rs`: 951 distinct words of the synthetic fixtures + 4,000 random / bit-flipped words (1,716 undecodable) compared line for line with dumps written by the PYTHON `capstone` 5.0.7 (`tools/cs_dump.py`): byte-identical, so the gate is real parity, not only a regression guard. The dump format moved to `Decoder::probe_line` (shared with `examples/cs_probe.rs`) |
| P11 | the shipped IPA (20261121-235154) predates 0090/0091 | **OUTDATED** | `dist/GameApp-rust-20261122-004929-rust7a5f3e13.ipa` carries 0090-0092 (STATE); the current verifier passes it (226 checks). A rebuild with 0093 waits for the owner |
| P12 | `verify_ipa.sh` rewritten after the build: a published PASS cannot be replayed | **CONFIRMED, FIXED** | `build-app.sh` copies `app/verify_ipa.sh` + `scripts/lib-spawn.sh` to `dist/<ipa>.verify/` (same layout), runs THAT copy, keeps its log and a `replay.sh` with the build's environment; the MANIFEST records both full sha256 and the check count. Exercised on a copy of the last IPA (pass and FAILED rename paths); not yet by a real build |
| P13 | CI does not run `verify_ipa.sh`, `check-no-spawn.sh`, `app/c/test/run.sh` | repo/CI agent | `scripts/test-lib-spawn.sh` (new, needs only clang/lipo/nm) is a candidate for CI too |
| P14 | the 0084-0089 markers are string presences | **CONFIRMED, DEFERRED** (device) | by design: the behaviour is checked on the device |
| P15 | `ins.rs` `full()` cache unbounded | **CONFIRMED, FIXED** | capped at 256 (as `finish::rx::cached`), cleared when full; answers unchanged (test `the_pattern_cache_is_bounded_and_changes_no_match`; oracle tests below) |
| P16 | `meter.rs` 100 ms sampling misses short steps | **CONFIRMED, ACCEPTED** | documented at `SAMPLE_EVERY`: a step's peak below the run's earlier maximum can be under-reported for that step, never for the run |
| P17 | `cache_sizes` finds the chain folder by path equality | **CONFIRMED (theoretical), FIXED** | both paths come from the same `cas` today; compared by `file_name()` now; test with `x/../shadercache` spellings (log line only) |
| P18 | 0083 `playStopped` | app patch 0093 (other agent) | |
| P19 | 0080 comment, 0086 `QueuePlan.For(Paused)`, 0078 wording | app patch 0093 (other agent) | |
| P20 | `app/inputs.tsv` `mtime_when_pinned` differs from the disk | **CONFIRMED, NO CHANGE** | only size and sha256 are read (`build-app.sh pin_state`, columns 3-4); the column is informative |
| P21 | `STATE-2026-11-20.md` counts drift | **CONFIRMED, FIXED** | counts updated to what was measured on 2026-11-22 (see its last entry) |
| A1 | `presolve` keeps one `Solved` per `may_solve` range without a cap | **LEFT** (documented) | output-neutral; it is in `scan.rs` (`may_solve`), not `jump_solver.rs`; cap it if a game ever makes it matter |
| A2 | the compressed-block path of `nsz_decompress.c` never ran | **CONFIRMED, FIXED** | `app/c/test/nsz_decompress_test.c` cases 8-9: NCZBLOCK with a real zstd Compressed_Block (214 -> 4,096 bytes) + a stored block, and a single zstd stream: rc 0, the rebuilt NCA body byte-exact, largest allocation 16 KiB; a damaged frame is refused with no output |
| A3 | the artifact verifier in the build loop | covered by P12 (build side) and P13 (CI side) | |
| A4 | owner decisions pending (0459, disk space) | **OPEN** (owner) | disk-space gate decided as P7 above; 0459 still waits |
| A5 | `verify_ipa.sh` count convention | **FIXED** | a summary line before the verdict: `N checks: P passed, F failed (one check = one PASS or FAIL line above; this line and the next are not checks)`; the managed-half PASS/FAIL lines printed by Python are counted too. 226 on the last IPA |
