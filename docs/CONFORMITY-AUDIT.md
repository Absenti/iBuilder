# Conformity audit of the Rust replacement against the latest iOSBuilder (2026-11-19)

Question asked: *go over the latest iOSBuilder again and make sure the Rust replacement is conformant and that everything is implemented.* The reference is the worktree
`$IOSBUILDER_WORKTREE` (read only; no write, no git, Python only with `PYTHONDONTWRITEBYTECODE=1 python3 -B -I`), the engine the app is built from
(`out-stage/out-engine/engine`, stamp `75c7336...` + 7 `paches/engine/*.patch`) and the worktree's newest Python .ipa (`out-game-eva/gameapp-out-device/GameApp.ipa`, 2026-11-19 10:32). **No .ipa/.app was built, no `dotnet publish`, no
`build-app.sh` run.** Python was used only to run the engine's own `settings.py` and `game.py` (stdlib only, from the read-only tree, `-B -I`) as oracles; no venv was created (the `capstone` module is not installed and nothing was downloaded).

Verdicts: **DONE** (equal behaviour, or a Rust/C#/C/bundle counterpart that does the same), **DIFFERENT** (differs; the note says whether it is intended), **MISSING** (not implemented), **NOT APPLICABLE** (belongs to
the interpreter that is gone). "fixed here" marks a row this audit closed. Counts are at the end of section 10.

## 0. Method, and what was and was not checked

| Check | Command / source | Result |
|---|---|---|
| Python read in full | `gameapp_pipeline.py` (2,910 lines, 155,544 B, mtime 02:24:57: the version the pipeline port's delta used), `gameapp_shim.c` (1,662 lines), `DECISIONS.md` 70-92, the C# `ConfigText`, `GameAppWarm.cs`, the settings sheet | rows below |
| Every Python def/class has a Rust place | `grep '^def\|^class'` (92 names) against `fn`/`struct` of the crates | 92 / 92 mapped (the names with no same-named Rust item are renamed, inlined or interpreter-only; each is a row of A) |
| Every Python string literal >= 10 chars has its text in Rust | `ast` walk, 578 literals, fragment search in `crates/**` (`audit/lits.py`) | 44 not found: all CPython-only (`_dmgameapp`, GIL, `shaderengine`, `PIPELINE_BUNDLE`, spawn audit, the live queue's `[RED] the queue failed`), none a line the app or the next launch reads |
| C# snapshot = worktree | `app/refresh-snapshot.sh --check` | "the worktree sources equal the snapshot (32 files)" |
| The 19 P/Invoke imports | `grep DllImport app/gameapp/*.cs` against `app/gameapp_shim_rust.c` | 19 / 19 exported (same line numbers as `APP-RUST.md` 1.1: no import added or removed) |
| `launcher.json` base document, every registered game | the engine's `settings.py` run on 187 games x 3 variants (nvn, nvn with `DYNAMIC_RESOLUTION` + `RENDER_SCALE 2`, vulkan) = 561 documents against `ibuilder_ffi::launcher::build` (`crates/ibuilder-ffi/tests/launcher_oracle_all.rs`) | before the fix 552 / 561 identical (9 refused), **after 561 / 561 identical, key order included** |
| `profile.tsv` render, every registered game | the engine's `game.py profile <id>` on 187 games against `render_profile` (`crates/ibuilder-profile/tests/engine_all_games.rs`) | before the fix 184 / 187 (3 refused: Ocarina of Time among them), **after 187 / 187** |
| IPA bundle contents | `unzip -Z1` of the worktree's Python .ipa (1,479 entries) and of `dist/GameApp-rust-20261119-150707-rustd75e1943.ipa` (706) | section 9 |
| New tests | `cargo test --release -p ibuilder-pipeline`, `-p ibuilder-profile --lib`, the two oracle tests above, then the whole workspace | section 10 |

**Not checked (no oracle left, say so):** the rewriter on a real game other than through its own earlier tests (the Python oracles `out-py/` and the dumps are gone; `capstone` is not installed and a download is not mine to
make); a real device (nothing here ran on a phone); the derivation of a provisional profile (`profile_guess.py`, `facts`) for games the engine does not register, against the *patched* engine (the earlier 346-variant check used the older
engine copy); the C# compiled (the new patch was only parsed, section 8); `scripts/build-app.sh` itself (never run).

## 1. What changed in the worktree, and what each change requires

The snapshot (`app/SNAPSHOT.tsv`) was taken at 12:09 and **still equals the worktree** (checked at ~16:30). Files whose mtime is after 02:30 today (`find -newermt '2026-11-19 02:30'`, build artefacts left out):

| file (mtime) | in the snapshot? | what it requires of the Rust app |
|---|---|---|
| `GameAppMain.cs` (09:59), `GameAppLibrary.cs` (10:14), `GameAppTheme.cs` (10:11) | yes | nothing: UI. The 19 imports and `ConfigText` keys are unchanged (section 5/6); no new Documents file name appears in the C# beyond those of section 5 |
| `GameAppUi.cs`, `GameAppCloud.cs`, `GameAppQueueUi.cs`, `GameAppModsUi.cs`, `GameAppSavesUi.cs` (02:50), `Info.plist.fragment` (02:51) | yes | nothing (UI / plist). `verify_ipa.sh` of this repo already reads the plist keys |
| `gameapp/build_gameapp.sh` (10:30), `gameapp/verify_ipa.sh` (10:26) | no (the Python build's scripts, not used here) | compared for what the bundle carries: section 9 |
| `load/front_edit.m` (02:34), `load/tests/pad_harness/build.sh` (02:35), `tools/build_pydeps_ios_ft.sh` (02:38) | no (host sources / Python build) | `front_edit.m` is inside the prebuilt host: `host-prebuilt-loadable-ios.dylib` is 02:39, after it. `app/inputs.tsv` pins the host |
| `tools/patched_engine.sh` (12:26) | no | the cloner of the patched engine. The app reads the already built `out-stage/out-engine`; the 7 `paches/engine/*.patch` sha256 equal the engine stamp (`6b0fee9e... 0301 b6b1530b...`, compared now) and none is newer than 02:24 |
| `docs/DECISIONS.md` (10:19), `docs/STATE-GAMEAPP.md` (10:33) | no | read: entries up to 92, section 7 |
| `gameapp_pipeline.py` (02:24:57), `mods.py` (02:24:27), `gameapp_shim.c` (02:23) | n/a | unchanged since the delta of `docs/chain/PIPELINE-RUST.md` section 8 (same size and mtime) |
| `paches/0012`, `0013` (02:24, 02:25), `0009` (02:12) | n/a | host patches: inside the prebuilt host (02:39). Section 7 |

## 2. Matrix A: every def/class of `gameapp_pipeline.py` and its Rust place

Rust places are `crates/ibuilder-pipeline/src/*.rs` unless named. Tests: `tests/pipeline_spec.rs` (52 + 8 new) and `tests/resume_spec.rs` carry the Python checks (`// py:` comments).

| ID | Python (line) | Counterpart | Verdict | Note |
|---|---|---|---|---|
| A01 | `_audit`, `_SPAWN_EVENTS` (29) | none: no process API in the crate; log line "process spawns: none" | NOT APPLICABLE | |
| A02 | `BUNDLE`, `DOCS` from the environment (35) | `ctx.bundle`, `ctx.docs`; `ibuilder_options.{bundle_dir,documents_dir}` | DONE | |
| A03 | `read_config` (44) | `config.rs` | DONE | |
| A04 | `mib`, `mem_line` (56, 60) | `memory.rs` (the `[mem:tag]` text is byte-equal) | DONE | |
| A05 | `STAGE_FILE`, `stage` (76, 1131) | `stage.rs::write_stage`, `parse_stage` | DONE | |
| A06 | `SHADER_MAP_HEADER` (86) | `shaderpack.rs` | DONE | |
| A07 | `_cfg_title_id`, `_profile_title_id` (91, 96) | `ctx.title_id`, `tsv::profile_title_id` | DONE | |
| A08 | `_derived_shader_maps` (108) | `shaderpack::derived_shader_maps` | DONE | |
| A09 | `set_shader_map` (123) | `shaderpack::set_shader_map` | DONE | |
| A10 | `install_shader_packs` (162): `.shaders.tar` import, one `<id>/` folder, `.part` then rename, tar removed, refusals | `shaderpack::install_shader_packs` (`tar` crate) | DONE | tests: imported pack, other chain, member outside the folder |
| A11 | `set_shader_pack` (219): DM_NVN_SHMAP, DM_MSL_DIR, chain check | `shaderpack::set_shader_pack` | DONE | **plus** the top-up of an existing pack (row P05, fixed here) |
| A12 | `game_table_dir` (249) | `tsv::game_table_dir` | DONE | |
| A13 | `profile_modules` (261) | `profile::profile_modules` | DONE | |
| A14 | `invalidate_previous_build` (280) | `profile::invalidate_previous_build` | DONE | |
| A15 | `free_bytes` (305) | `sys::free_bytes` | DONE | |
| A16 | `_last_guest_run` (316) | `tsv::last_guest_run` | DONE | |
| A17 | `note_boot_result` (330): sandbox refused dlopen -> `gameapp.load-blocked`, boot request withdrawn | `markers::note_boot_result` | DONE | the line the C# greps is byte-equal |
| A18 | `presize_heap` (370) | `profile::presize_heap` | DONE | |
| A19 | `apply_heap_check` (437) + the engine's `heap_signals`, `depth_signal` | `profile::{apply_heap_check, heap_signals, depth_signal}` | DONE | re-read against the PATCHED engine's `heap_signals` today (Ocarina issue #1361 branch, the Unity `stdalloc_cap` branch are in): same logic. Equality over all games' logs is not verified (no logs) |
| A20 | `GAME_SETTINGS`, `apply_game_settings` (504, 757) | `settings::{game_setting, apply_game_settings}` | DONE | |
| A21 | `game_gfxapi` (515) | `settings::game_gfxapi` | DONE | reads in 4 MiB blocks (the phone's memory is the budget) |
| A22 | `LAUNCHER_DROP_UNLESS_PULLING`, notes (533-558) | `settings.rs` constants | DONE | |
| A23 | `GAMEAPP_ROWS` (565), `LINKED_SWITCHES`, `METALFX_LEVELS` | `settings::{gameapp_rows, LINKED_SWITCHES, METALFX_LEVELS}` | DONE | 5 rows; the External display note's text differs from the Python only in how the grep sees the quotes (compared: equal) |
| A24 | `write_launcher_settings` (597) | `settings::write_launcher_settings` + `LauncherBase` (`ibuilder-ffi/src/launcher.rs`) | DONE | base document now proven on 187 games x 3 variants (561/561) |
| A25 | `launcher_catalogue` (674) | `settings::launcher_catalogue` | DIFFERENT | intended: sha256 of a canonical form instead of sha1 of Python reprs; every `launcher.json` is written once again (documented in PIPELINE-RUST 7) |
| A26 | `launcher_stale`, `launcher_envs` (691, 701) | `settings::{launcher_stale, launcher_envs}` | DONE | |
| A27 | `apply_env_files` (714) | `settings::apply_env_files` | DONE | |
| A28 | `apply_mods` (746) | `mods::apply_mods` -> `ibuilder-mods` (`apply_boot`) | DONE | |
| A29 | `profile_hex` (803) | `tsv::profile_hex` | DONE | |
| A30 | `set_alias_size`, `check_address_space`, `set_game_env` (819, 850, 882) | `profile.rs` | DONE | DM_AOC_DIR, DM_SAVE, DM_TEMP, DM_ALIAS_SIZE, the RED texts |
| A31 | `_map_rows` (917) | `tsv::map_rows` | DONE | |
| A32 | boot-only request, framework stamp, `boot_only_*`, `read/write_framework_stamp` (936-990) | `markers.rs` | DONE | |
| A33 | `boot_only` (992) | `boot::boot_only` | DONE | |
| A34 | `release_memory` (1079) | `memory::release_memory` | DIFFERENT | intended: no `gc.collect` line ("N object(s) collected" is gone); the trim and `[mem:tag]` lines stay |
| A35 | `release_mimalloc` (1106), DECISIONS 82 | `memory::release_mimalloc` over `MemSource::release_mimalloc() -> Option<i32>` | NOT APPLICABLE | the patched-CPython allocator call; Rust's allocator has nothing to hand back; the seam is documented and tested |
| A36 | `Log` (1143) | `log.rs` | DIFFERENT | intended: the run banner says `ibuilder-pipeline`, not the Python version; nothing in the C# greps the banner (checked: no `chain run` match in the snapshot) |
| A37 | `load_module`, `mods_module` (1157, 1168) | none (crates linked) | NOT APPLICABLE | |
| A38 | `rewrite_key` (1175) | `rewrite::rewrite_key` | DONE | test: equals sha256 of the Python's input, mods digest included |
| A39 | `_link_tree`, `rewrite_cache_dir` (1202, 1216) | `util::link_tree`, `rewrite::rewrite_cache_dir` | DONE | |
| A40 | `do_rewrite` (1224): 3 modules, thermal wait, cache, mods plan/apply/remove/mark, `DM_REEXEC` loop of 3 | `rewrite::do_rewrite`; the re-run loop is inside `ibuilder-rewrite` | DONE | a refused module is an `Err`, not `sys.exit` |
| A41 | `SHADER_TABLES` (1296) | `shaders::SHADER_TABLES` | DONE | |
| A42 | `shader_pulling_wanted` (1299) | `settings::shader_pulling_wanted` | DONE | **bug-compatible**: stage 3 asks for `work/profile/game.tsv` while the table is in `work/profile/<id>/`, so `pso_mtl4` is read only by `write_launcher_settings`; the pulling libraries are built only with `shader_pull=1` (as in Python; found and left alone, `PIPELINE-RUST` 9) |
| A43 | `split_version` (1318) | `ShaderBackend::split_version` (`ibuilder-shadersplit`) | DIFFERENT | intended: a library built on the phone by the Python app records another split hash, so it counts as stale and is built once more by the Rust app |
| A44 | `stale_split`, `aoc_images`, `dlc_signature`, `stale_dlc` (1329-1378) | `shaderpack.rs` | DONE | |
| A45 | `app_setting`, `shader_parallelism` (1384, 1397) | `shaders::{app_setting, shader_parallelism}` | DIFFERENT | intended: no GIL, so "on unless `parallel_shaders 0`" |
| A46 | `SHADER_CACHE`, `folder_bytes`, `_gib`, `shader_cache_folder` (1412-1464) | `shaderpack::shader_cache_folder` | DIFFERENT | intended: the cache is the folder `shadercache/cas-<chain>.dmsb`, not the engine's SQLite file; plus `shader_cache_keep` (a finished build removes its 22-43k small files, the Python kept them) |
| A47 | `rewrite_jobs` (1470) | `memory::rewrite_jobs` | DIFFERENT | the GIL test is gone (Rust threads always run). The memory fit still uses the Python's "the serial rewrite peaks near 3 GiB" (`left - 3200 MiB`): the Rust rewrite peaks near 0.5 GiB (RESULTS 2), so on a 3.3 GiB ceiling it runs `main` on 1 thread where 4 would fit. Conservative, not wrong; left (needs a phone measurement, section 11) |
| A48 | `shader_jobs_for_memory` (1504) | `memory::shader_jobs_for_memory` | DONE | 450 MiB per translation as sized for Python (conservative) |
| A49 | `read_thermal`, `ThermalGate`, `thermal_gate` (1525-1592) | `thermal.rs` | DONE | |
| A50 | `ShaderMemoryGate` (1595) | `gate.rs` | DONE | |
| A51 | `file_fingerprint` (1641) | `shaders::file_fingerprint` | DIFFERENT | intended: sha256 prefix instead of sha1; an interrupted Python-era stage 3 does not resume once |
| A52 | `shader_work_signature`, `ShaderResume` (1658, 1676) | `shaders.rs` | DONE | |
| A53 | `rewrite_is_cached`, `shaders_will_build` (1749, 1773) | `rewrite.rs`, `shaders.rs` | DONE | |
| A54 | `CONTINUE_*`, `KILLED_FILE`, `read_kv`, `note_killed_run`, `take_continue` (1793-1845) | `markers.rs`, `timings::read_kv` | DONE | |
| A55 | `restart_for_shaders` (1848), `SHADER_RESTART_MIB` 2400 | `markers::restart_for_shaders` | DONE | trap 75 conditions in the same order; the mimalloc branch is the A35 seam |
| A56 | `BUILD_TIMES*`, `device_model`, `Timings` (1896-2043) | `timings.rs` | DONE | 21 columns equal; the latent `merge_partial` bug of the Python is fixed (PIPELINE-RUST 7) |
| A57 | `build_shaders_on_device` (2046) | `shaders::build_shaders_on_device` | DONE | no-library, `.none` marker, stale split/DLC, chain cleanup, 8 GiB free check, work folder resume, 6 threads, `pack.txt`; **plus** the top-up (P05, fixed here) |
| A58 | `LEARNED`, `learned_*`, `learn_shaders`, `relearn_shaders` (2262-2370) | `shaders.rs` | DONE | same `learned_shaders/b-<chain>/` and `raw/` layout the C# warm-up reads |
| A59 | `LiveShaders` (2372) | `shaders::LiveShaders` + `sys::boot` trampoline + `live.rs` | DONE | `live shaders: [RED] the queue failed` has no form: the host reports a failure as a refused shader |
| A60 | `do_shaders` (2447): the `shader_manifest` post-rewrite path | `shaders::do_shaders` -> `ShaderBackend::post_rewrite` | DONE | |
| A61 | `save_game_icon` (2469), `do_profile` (2492), `INFO_PLIST` (2508), `drop_exefs` (2516) | `profile.rs`, `chain::ProfileStage`, `assemble.rs` | DONE | |
| A62 | `do_assemble` (2530): flat/ hard links, assemble, Info.plist, sign callback, stamp | `assemble::do_assemble`, `ibuilder-macho`, `ibuilder_sign_fn` | DONE | the sign mode strings and rc 10-13/20 are unchanged in the shim |
| A63 | `extract_nup` (2609), `nup_extracted` (2870) | `NupStage` + `ibuilder-nup`; `chain::nup_extracted` | DONE | in the build (`ibuilder-cli` default feature `nup`, `dist/MANIFEST.txt`) |
| A64 | `use_config_paths` (2651) | `shaderpack::use_config_paths` | DONE | |
| A65 | `main`, `_chain` (2661-2867): result codes 0/1/2/3/75, boot tail, `pipeline.boot` | `chain.rs`, `boot.rs` | DONE | a failed stage 3 or 4 still returns 0, as in Python |
| A66 | `stdlib_extensions_check`, `__main__` SystemExit, `set_chain_result` (2876, 2894) | none | NOT APPLICABLE | |

## 3. Matrix B: settings tables, rows, switches

| ID | Python | Counterpart | Verdict | Note |
|---|---|---|---|---|
| B01 | `GAME_SETTINGS` (mode, fps, picture, show_fps, `DM_LIVE_SHADERS`) | `settings::game_setting` | DONE | |
| B02 | `LAUNCHER_NOTES` (14 notes), `LAUNCHER_NOTES_REPLACE` (3: `DM_FRAMEGEN`, `DM_PRESENT_SNAP`, `DM_OPMODE`) | `settings.rs` | DONE | texts compared by the literal search |
| B03 | `GAMEAPP_ROWS`: Low latency (`DM_PRESENT_DRAIN`, +`DM_PRESENT_LATCH`), Missing graphics (`DM_LIVE_SHADERS`), Pipeline cache (`DM_PSO_BINARY_ARCHIVE`), MetalFX (`DM_METALFX` -> `DM_PRESENT_UPSCALE` + `DM_RENDER_SCALE` 0.75/0.5), External display (`DM_EXTERNAL_DISPLAY`) | `settings::gameapp_rows`, `apply_game_settings` | DONE | the engine's `DM_PRESENT_UPSCALE` row is dropped for nvn games (one MetalFX row) |
| B04 | rows of the engine's `settings.tsv` (`DM_FRAMEGEN`, `DM_TOUCH_HAPTICS/SIZE/STICK`, `DM_RUMBLE*`, `DM_PAD_PRESS_TO_JOIN`, `DM_FPS_*`, `DM_RENDER_SCALE`, `DM_OPMODE`, `DM_LANG`, `DM_SCREEN_ASPECT`, the Performance rows) | `LauncherBase` (`ibuilder-ffi/src/launcher.rs`); `settings.tsv` bundled | DONE (fixed here) | **561 / 561 documents identical to `settings.py`** on 187 games; before: 3 games (0400486024DEC000 Ocarina, 0400A3A01EE4C000 Pokopia, 01006F8002326000 Animal Crossing) stopped on `screen_aspect wide|both` (row P03) |
| B05 | capture (`load/front_capture.m`), controllers (`dm_pad_*`, `controller_map.json`), touch editor (`touch_layout.json`), cheats menu | host patches + C#; not `launcher.json` rows | NOT APPLICABLE | nothing the chain writes; see F |
| B06 | `DM_EXTERNAL_DISPLAY` read by `GameAppExternal.cs` from the environment | `Env::commit` before the boot | DONE | `commit` runs right before `dm_host_probe_boot` |
| B07 | `ENV_FILE_PREFIXES` (`DM_`, `MTL_`), `Documents/env.txt`, `games/<id>/env.txt` | `settings::apply_env_files` | DONE | the "already set" line keeps the Python's `repr` |
| B08 | `DM_FONT_DIR` (shared-font stand-ins) | `profile.rs` (bundle `dm/font/`) | DIFFERENT | NEW in Rust, not in Python (Mario Kart 8 black screen "cannot open the standard shared font"); intended; the bundle step is another package's (`build-app.sh`), and the 15:07 .ipa predates it |

## 4. Matrix C: decisions and policies the chain makes

| ID | Behaviour | Counterpart | Verdict | Note |
|---|---|---|---|---|
| C01 | `decide_launch` (interrupted -> nothing; args; continue; boot-only armed / attempted / withdrawn) | the C# `ViewDidLoad` keeps deciding; `state.rs::decide_launch` is a tested equal | DONE | the Rust one is in the archive, not called (zero C# changes) |
| C02 | boot-only: `attempted` after the start, consumed when the boot comes back, never retried by a launch | `markers.rs`, `boot.rs` | DONE | |
| C03 | continue / killed run / `restart_for_shaders` (rc 75) | `markers.rs`, `state.rs` | DONE | `gameapp.killed` is the C#'s, read and removed by `note_killed_run` |
| C04 | heap check, first-boot heap, alias region, address-space report | `profile.rs`, `sys/vm.rs` | DONE | |
| C05 | thermal gate: `gameapp.thermal` "state lowperf", Low performance = 2 jobs, serious = wait, critical stops the rewrite between modules | `thermal.rs`, `gate.rs`, `rewrite.rs` | DONE | |
| C06 | memory / jobs policy: `shader_jobs_for_memory`, `rewrite_jobs`, `SHADER_PER_JOB`, the 8 GiB free-space check | `memory.rs`, `shaders.rs` | DONE | constants as sized for Python (A47/A48: conservative) |
| C07 | rewrite cache: key = module bytes + `app_build` + the table's `.tsv` + the mods digest; hard-link tree | `rewrite.rs` | DONE | |
| C08 | learned shaders / live shaders: `DM_MSL_LEARNED`, `DM_NVN_SHMAP_LEARNED`, `GAMEAPP_LIVE_THREAD`, `relearn_shaders` before a boot | `shaders.rs`, `sys/boot.rs` | DONE | |
| C09 | mods hooks: `plan`, `apply_module`, `remove_patched`, `mark_applied`, `record_build_ids`, `apply_boot` (`DM_ROMFS_OVERLAY`, `DM_CHEATS`, `cheats-run/`) | `ibuilder-mods` through `ModsHost` | DONE | 0 differences on 200 scenarios (MODS-RUST) |
| C10 | nup / extraction flows: `.nup` read in stage 1 (`ibuilder-nup`), `nsz` conversion before the extractor (C, kept), merged-update extractor patch | `NupStage`, `app/c/nsz_decompress.c`, `app/patches/nx_extract_merged_update.py` | DONE | the merged-update patch is not tested on real data |
| C11 | boot refusals: 32-bit program, unknown build id | `preflight.rs` | DIFFERENT | NEW in Rust (Mario Kart 8 `KeyError: 2`); intended; changes no output of a game that passes |

## 5. Matrix D: files and keys the C# reads or writes (Documents)

| ID | File / key | Writer -> reader | Verdict |
|---|---|---|---|
| D01 | `gameapp.config` keys: `extract_out exe_dir modules patched_dir host_dylib framework_out profile romfs shader_manifest boot_seconds boot_after_build keep_exefs boot_floor_mib app_build shader_chain game content shader_packs shader_build nup` (C# `ConfigText`) | C# -> `config.rs` | DONE (all 31 keys the Python reads are read by Rust; `shader_cache_keep` is Rust-only and optional) |
| D02 | `gameapp.stage` | Rust `write_stage` -> C# `ParseStage` | DONE |
| D03 | `pipeline.stages.log`, `gameapp.chain.log`, `gameapp.guest.log` | Rust (and the shim's C `logln`) -> C# tailer | DONE |
| D04 | `build_times.tsv` (21 columns, last 50), `gameapp.timing.partial` | Rust `timings.rs` | DONE |
| D05 | `gameapp.running` (+ `game=`, `bootonly=`), `gameapp.args`, `gameapp.boot-only`, `gameapp.framework`, `gameapp.continue(.run)`, `gameapp.killed`, `gameapp.load-blocked` | C# and Rust, same formats | DONE |
| D06 | `gameapp.thermal` | C# writes, Rust reads | DONE |
| D07 | `gameapp.settings.tsv` (`parallel_shaders`, `low_performance`, `warm_shaders`) | C# writes; Rust reads `parallel_shaders` | DONE (`warm_shaders` is read by the C# only and is removed by patch 0020, section 8) |
| D08 | `gameapp.logmarks` | C# only (Rust's `logwindow.rs` is a tested equal, unused) | DONE |
| D09 | `games/<id>/{launcher.json, settings.tsv, env.txt, mods/, mods.tsv, mods.applied, build_ids.txt, cheats-run/, icon.jpg, learned_shaders/, rewrite-cache/, save/, temp/, aoc/}` | Rust writes `launcher.json`, `mods.applied`, `build_ids.txt`, `cheats-run/`, `icon.jpg`, `learned_shaders/`, `rewrite-cache/`; C# the rest | DONE |
| D10 | `games/<id>/{pso_user.tsv, used_shaders.tsv, warmed.tsv, touch_layout.json, controller_map.json, pipelines-*.mtlarchive}` | host / C# only | NOT APPLICABLE (not the chain's) |
| D11 | `shaders/<id>/` (pack folder + `.work`, `.none`, `.part`), `shadercache/`, `profile.tsv`, `profile-derived/<id>/`, `extract/{ExeFS,flat,rewrite,RomFS.img,control.nacp,icon.jpg}` | Rust (stage 3, profile, rewrite, assemble) | DONE |

## 6. Matrix E: the shim

`gameapp_shim.c` (1,662 lines, CPython bridge) against `app/gameapp_shim_rust.c` (572 lines) and `ibuilder.h`.

| ID | Symbol | Counterpart | Verdict |
|---|---|---|---|
| E01 | `gameapp_run_chain` | `ibuilder_create` + `ibuilder_run_chain` + `ibuilder_chain_status`, flag `BOOT_IN_PROCESS`, 16 MiB stack, `DM_PROFILE` preset | DONE |
| E02 | `gameapp_memline`, `gameapp_available`, `gameapp_extended_va`, `gameapp_increased_memory`, `gameapp_get_task_allow`, `gameapp_address_space` | `ibuilder_mem_line`, `ibuilder_available_bytes`, `ibuilder_address_space_info` | DONE |
| E03 | `gameapp_identity_status/_store_bytes/_where`, `device_identity_sign` | kept in C (Security.framework), copied | DONE |
| E04 | `gameapp_set_game_layer`, `gameapp_quit_game` | `ibuilder_set_game_layer`, `ibuilder_quit_game` | DONE |
| E05 | `gameapp_nsz` | kept in C against a standard zstd object | DONE |
| E06 | `gameapp_pad_create/_layout/_layout_tv`, `gameapp_tv_attached`, `gameapp_game_menu` | `ibuilder_host_symbol("dm_front_*")` | DONE |
| E07 | `gameapp_set_xlate` | stores the managed entry; trampoline drops `argv[0]` | DONE |
| E08 | the launch constructor `gameapp_as_reserve` + census | Mach-O initializer `ibuilder-ffi::early` -> `sys::boot::early_init` | DONE |
| E09 | `install_signal_logger`, `redirect_guest_output`, `term_after`, `consume_boot_request` | `sys/signals.rs`, `sys/redirect.rs`, `sys/boot.rs` | DONE |
| E10 | `boot_framework`: `PROBE_MODE`, `DM_PROFILE`, `DM_ROMFS` set AFTER the dlopen | `sys/boot.rs` sets them BEFORE the dlopen (and the shim also presets `DM_PROFILE`) | DIFFERENT: intended; the race of `APP-RUST` 5.4 is closed in the crate now (the "proper fix" it asked for is in the code), the shim's preset is kept as a second guard |
| E11 | `_dmgameapp.{boot, sign, xlate, spirv_cross, live_next, live_done, live_stop, mem, address_space, trim, self_identifier}` | `DlopenBoot`, `ibuilder_sign_fn`, `ibuilder_options.{xlate,spirv_cross}`, `LiveQueue`, `sys::{mem,vm,entitlements}` | DONE |
| E12 | `_dmgameapp.release_mimalloc`, `set_chain_result`, `zstd_decompress`, `zstd_stream_new/feed` | none / `ruzstd` in `ibuilder-shadersplit`, `ibuilder-nup` | NOT APPLICABLE |
| E13 | `choose_gil`, `PYTHONHOME`, `Py_BytesMain`, `g_chain_result`, inittab | none | NOT APPLICABLE |
| E14 | new exports `gameapp_cancel_chain`, `gameapp_stdzstd_*` | present, not imported by the C# | DONE (extra) |

## 7. Matrix F: DECISIONS 70-92 and the patches

The prebuilt host (`host-prebuilt-loadable-ios.dylib`, 2026-11-19 02:39) carries every `paches/00xx` host patch and `load/*.m`; the engine patches `paches/engine/*` are in the patched engine the C# translator and the tables come from.

| ID | Entry | What it needs | Where it is | Verdict |
|---|---|---|---|---|
| F70 | 70 name, icon, Settings tab | C# + assets | `app/gameapp/` (snapshot) | DONE |
| F71 | 71 stage 3 under the ceiling: `shader_jobs_for_memory`, `ShaderMemoryGate`, compacting .NET GC every 40 translations, 15 s `[mem:]` lines, work folder -> 1 thread | pipeline + C# `XlateEntry` | `memory.rs`, `gate.rs`, `shaders.rs`; the GC in `GameAppMain.cs` | DONE |
| F72 | 72 build screen, exports | C# | snapshot | DONE |
| F73 | 73 Light/Full IPA | C# (`GameAppExport.cs`) | snapshot (+ `0010` patch by another package) | DONE |
| F74 | 74 resume of a killed build: `ShaderResume`, `JobMemo`, `gameapp.killed`, `note_killed_run` | pipeline + backend | `shaders.rs`, `markers.rs`, `ibuilder-shadersplit` memo | DONE |
| F75 | 75 restart between rewrite and shaders (rc 75, `gameapp.continue`) | pipeline + C# | `markers::restart_for_shaders` | DONE |
| F76 | 76 timings, thermal gate | pipeline + C# writes `gameapp.thermal` | `timings.rs`, `thermal.rs` | DONE |
| F77 | 77 exports per game, logs zip, `logmarks` | C# | snapshot | DONE |
| F78 | 78 startup, build queue, Low performance, heat check | C# + `gameapp.thermal` | snapshot, `thermal.rs` | DONE |
| F79 | 79 Preparing the shaders | C# `GameAppWarm.cs` + the pack's `b-<chain>.img` (DMROMFS1) | Rust writes the same image bytes; C# unchanged | DONE, behaviour **changed to manual** by patch 0020 (section 8) |
| F80 | 80 `DM_REWRITE_JOBS` (`paches/engine/0301`) | rewrite threads | `RewriteRequest.jobs` + rayon pool in `ibuilder-rewrite`; output equal on 1, 2, 8 threads | DONE |
| F81 | 81 BGContinuedProcessingTask | C# + plist | snapshot | DONE |
| F82 | 82 mimalloc release (`paches/python/0001`) | patched CPython | none | NOT APPLICABLE |
| F83 | 83 pipeline archive (`paches/0009`), row `DM_PSO_BINARY_ARCHIVE` | host + row | host; `settings::gameapp_rows` | DONE |
| F84 | 84 MetalFX = one row `DM_METALFX` | row + expansion | `settings.rs` | DONE |
| F85 | 85 used shaders warmed first (`used_shaders.tsv`, `pso_user.tsv`) | C# + host | snapshot / host | DONE (manual only after 0020) |
| F86 | 86 touch controls editor (`paches/0012`, `touch_layout.json`) | host + C# | host | DONE |
| F87 | 87 several controllers (`paches/0013`, `controller_map.json`) | host + C# | host | DONE |
| F88 | 88 capture (`front_capture.m`) | host + plist usage string | host, `Info.plist.fragment` | DONE |
| F89 | 89 mods (`mods.py`, three hooks) | pipeline hooks + `mods.py` | `ibuilder-mods`, `mods.rs`, `rewrite.rs` | DONE |
| F90 | 90 cheats (`write_cheats_folder`, `DM_CHEATS`) | mods apply_boot | `ibuilder-mods` | DONE |
| F91 | 91 TV as display (`DM_EXTERNAL_DISPLAY`, `dm_front_tv_attached`, `dm_game_menu`) | row + 3 host entries | `settings.rs`, shim `host_sym` | DONE |
| F92 | 92 iPad layout | C# + plist | snapshot | DONE |
| FP1 | `paches/0001` (Main public), `0003` (simulator layer guard), `0006` (`DM_AS_AT`) | build step / host | `build-app.sh` sed; host; `sys/vm.rs` sets `DM_AS_AT` | DONE |
| FP2 | `paches/0002`, `0004`, `0005`, `paches/engine/0301` (Python rewriter: fork, memory) | CPython rewriter | replaced by `ibuilder-rewrite` | NOT APPLICABLE |
| FP3 | `paches/0007` (MSL source at run time), `0008` (live shaders) | host + `.metal` library + `LiveShaders` | `ibuilder-shaderbuild` writes `.metal` sources; `shaders.rs` | DONE |
| FP4 | `paches/engine/0103 0202 0203 0205 0206` (host, `settings.tsv`) | host + bundled `settings.tsv` | host dylib; `settings.tsv` is the patched one (the 561-document oracle ran on it) | DONE |
| FP5 | `paches/engine/0104` (Snowdrop bare zlib cache files in `shader_split.py`: `snowdrop_shaders`, marker `#SDFZLIB@`; South Park: The Fractured But Whole 623 -> 4,614 shaders) | the split | **absent from `ibuilder-shadersplit`**: `grep SDFZLIB crates/` finds nothing, the patched `shader_split.py` has it (line 545) | **DONE** 2026-11-19 (section 13) |
| FR1 | the engine's per-game code-valued rewriter patches (`dm/games/*/patches.py`: `FRAME_STEP`, `STUBS`, `MATCH_FRAME_STEP`, `MATCH_STUBS`) and `tables.rs` generated from the 26 older ones | `ibuilder-rewrite/src/{stubs,tables}.rs` | **DONE** 2026-11-19 (section 13): the 13 code-valued patch files ported, `tables.rs` regenerated from the shipped engine, coverage guard added |

## 8. The shader warm-up ("Preparing the shaders", decisions 79 / 85)

What it is: `GameAppWarm.cs` compiles each library of the pack's `b-<chain>.img` with `newLibraryWithSource:` and the host's own options so that Metal's compile cache (per bundle id) already holds it when the host asks; it
orders what the player used first (`used_shaders.tsv`), then learned shaders, then the pack in order; two Utility-QoS workers; one at "fair" heat, none at "serious", none under 700 MiB; the rest of the pack only while the phone is charging
or cool; rows in `Documents/games/<id>/warmed.tsv` under a header naming the app build and the chain.

**What the Python-era app did.** The same C#: this file has no Python in it, so the Rust app behaves exactly as the Python one did. Its triggers (read in the snapshot): (1) `GameAppMain.cs:2391`, the end of every build that did not fail:
`StartWarm(lastRunGame ?? selected)`; (2) `GameAppMain.cs:848`, every launch: `StartWarmAtLaunch()`, which for an exported player is `StartWarm(PlayerDump)`; (3) the debug launch argument `-GAMEAPP_WARM_TITLE`. The only brake was
`warm_shaders 0` in `gameapp.settings.tsv` (default on, "no Settings row yet").

**What was intended** (DECISIONS 79): after a build, and "at a player's **first** launch", because an exported app has its own bundle id and so an empty cache. The code does not tell the first launch from the others: every launch of
a player runs the whole routine, and a later launch, although `warmed.tsv` lets it skip the compiles, still reads and SHA-256-hashes every library of the image (the whole 1.7-9 GB file) before it can skip it. On a new export or a new
app build (the header changes) it compiles everything again. That is the behaviour the user saw.

**Fix (C#, as a patch the build already applies).** `scripts/build-app.sh` applies `app/patches/*.patch` with `patch -p1` to its copy of the C# snapshot (lines 209-214), after the engine's extractor patch and before the project is
written; `app/gameapp/` stays the exact snapshot (`app/refresh-snapshot.sh --check` still passes). **`app/patches/0020-warm-up-manual-only.patch`**:
* removes the automatic start at the end of a build and at a launch (`StartWarmAtLaunch` keeps only the debug argument);
* removes `GameWarm.Enabled` (the `warm_shaders` row that defaulted to on);
* adds `GameWarm.HasLibrary(titleId, packRoot)` (a cheap `b-<chain>.img` existence check) and, in the game settings sheet (library **and** an exported player's Settings), a button **"Prepare the shaders…"**, offered only when the game has a
  library of this host's chain; it asks first (minutes to an hour, battery, heat), starts one run, and while a run is going the same button reads **"Stop preparing the shaders"** and stops it. A run still stops by itself when a build or a boot
  starts (`StopWarm`), exactly as before; what it finished stays in `warmed.tsv`.
Checks: `patch -p1 --dry-run` against the snapshot alone, and for real after `0010-export-clean-bundle-and-player-cover.patch` (another package's, same glob, applied first): both apply. The two files were parsed with Roslyn (`csc` without the
iOS references): no syntax error (CS1xxx), and the same classes of semantic errors as the unpatched files (missing iOS types), +13 CS0518 / +1 CS0246 from the new lines. **Not compiled against the iOS SDK; not run.** The marker
`verify_ipa.sh` looks for (`Preparing the shaders`, the status text) is still in the C#.

## 9. The bundle: what the worktree's Python .ipa carries and ours does not

Python .ipa: 1,479 entries, 34.1 MB; Rust (`dist/...rustd75e1943.ipa`): 706 entries, 28.7 MB (`unzip -Z1`, sorted, `comm`).

| ID | Carried by the Python .ipa, absent from ours | Needed by the Rust app? | Verdict |
|---|---|---|---|
| H01 | `py/lib` (624), `Frameworks/<stdlib module>.framework` (144), `pyext/` (6), `Python.framework`, all `*.py` of the app (`gameapp_pipeline.py`, `profile_derive.py`, `assemble.py`, `shader_post.py`, `pyrewrite_driver.py`, `shader_device.py`, `mods.py`, `lzma_py.py`, `nup_zstd.py`, `scripts/pack_nup.py`) | no: replaced by crates | NOT APPLICABLE |
| H02 | `dm/tools/*.py` (54), `dm/games/{game,profile_guess,settings}.py` | no: `ibuilder-profile`, `LauncherBase` | NOT APPLICABLE |
| H03 | `dm/games/<id>/patches.py` (28) | the rewriter's data is compiled in (`tables.rs`), code-valued ones are ported or refused | DIFFERENT: the data in `tables.rs` is older than these 28 files (FR1) |
| H04 | `shaderengine/` (221 files) and `engine/` (59): `dmsh_*.py`, `shader_split.py`, `*.h`/`*.inc` of `dm/gfx/nvn`, `dmsh_pull.metal`, `dmsh_chain.lock`, `dmsh_xlate/Program.cs` | no: the finish texts are generated into `ibuilder-shader/src/finish/texts.rs`; the chain id comes from the config | DONE (any change of the engine's `dmsh_*` needs `tools/finish/gen_texts.py` again; pinned by the engine stamp) |
| H05 | per-game files in `dm/games/<id>/` | Python: `game.tsv` + `patches.py` only. Rust: `game.tsv` + `bf_want`, `dual_want`, `pso_manifest`, `bench` (build-app.sh section 5) | DIFFERENT: intended, a Rust-side addition |
| H06 | same-named files present in both | `dm/games/<id>/game.tsv` (187), `dm/games/settings.tsv`, `dm/host/*.c`, `dm/gfx/**`, `host-prebuilt.dylib`, the .NET runtime, `GameApp.r2r`, OpenSSL | DONE |

What ours carries that the Python .ipa does not: 378 files, all `dm/games/<id>/{bf_want 180, dual_want 9, pso_manifest 185, bench 4}.tsv`, and a larger executable (the Rust archive). The `dm/font/` stand-ins that `profile.rs` now hands to the host (B08) are not in this 15:07 .ipa: they are newer than it.

| ID | Gap found by this comparison | Verdict |
|---|---|---|
| D-TABLES | The Rust build READS two more small inputs from a registered game's folder that the bundle does not carry: `geo_mesh.tsv` (vertex/geometry pairs that get a mesh library; `ibuilder-shaderbuild::game::INPUT_TABLES`, `mesh_pairs`; 3 games) and `shader_facts_manual.tsv` (merged over the split's facts, `ibuilder-shadersplit::game`; 2 games). The Python .ipa did not carry them either (it carried only `game.tsv`), so this is not a regression; the engine's own Mac build uses them | MISSING (fix as a diff, not applied: `app/patches/build-app-input-tables.diff`; `PACK_INPUT_TABLES` in the pipeline already covers them) |

## 10. Fixes made, tests, and the rows they close

| Row | Gap | Fix | Test |
|---|---|---|---|
| **P05** (open item 1) | A shader pack that exists is not built again (A57 returns "already holds a library"), so a table added to the bundle later never reached it: Mario Kart's boot said `[RED] pipeline prewarm: seed list does not exist: Documents/shaders/<id>/pso_manifest.tsv` | `ibuilder-pipeline/src/shaderpack.rs::top_up_pack` + `PACK_INPUT_TABLES`; called from `set_shader_pack` (every boot, both boot paths), from stage 3's "already holds a library" branch, and after a fresh build (for a registered game whose derived folder holds only `game.tsv` after the heap check). It copies only the game's small input tables the pack LACKS (`bf_want, dual_want, geo_mesh, shader_facts_manual, pso_manifest, bench`) from `profile-derived/<id>/` else the bundle's `dm/games/<id>/`; **never overwrites a file the pack has (even an empty one), never reads or touches the library, the map or the split's tables, never writes into an exported game's read-only pack**, copies aside then renames (no half file), says nothing when there is nothing to add, caps a table at 32 MiB, one log line per failure. When a table the BUILD reads (a non-comment row of `bf_want`/`dual_want`/`geo_mesh`/`shader_facts_manual`, a `G` row of `pso_manifest`) is added to an existing library it says so ([ORANGE]): the library was built without it, only a new build uses it, delete `Documents/shaders/<id>` to rebuild. The seed list takes effect at the next boot | 7 new tests in `pipeline_spec.rs`: adds only what is missing and leaves library/map/split tables byte-equal; never overwrites; idempotent and silent; derived folder wins, bundle fills the rest; no pack -> nothing created; exported read-only pack untouched; the caveat appears for build inputs with rows and not for comment-only tables; stage 3's "already here" branch tops up. 61 / 61 `pipeline_spec`, 63 / 63 unit |
| **W** (open item 2) | The exported player runs the whole warm-up at every launch; also after every build | `app/patches/0020-warm-up-manual-only.patch` (section 8): manual button, no automatic start | patch applies alone and after 0010; Roslyn syntax check |
| **P03 / B04** | `ibuilder-profile::game::header_macros` knew only `screen_aspect off|on`; the patched engine's tables say `wide` (Ocarina of Time 0400486024DEC000, Pokopia 0400A3A01EE4C000) and `both` (Animal Crossing 01006F8002326000): **stage 0 of the chain failed with "'wide' is not in list" for those games with the app's bundled tables**, and the settings sheet could not be drawn | `ibuilder-profile/src/game.rs`: `wide` -> 2, `both` -> 3 (the engine's `("off","on","wide","both").index`) | unit test `screen_aspect_takes_the_four_values_of_the_patched_engine`; `engine_all_games.rs` 187 / 187 profiles identical to `game.py profile`; `launcher_oracle_all.rs` 561 / 561 |
| **D-TABLES** | two more input tables not bundled | `app/patches/build-app-input-tables.diff` (`patch -p1 < ...`, dry-run clean against the current script) | dry run only |

**Matrix counts** (rows of sections 2-7 and 9; section 10's fix rows are not counted): 146 rows = **118 DONE, 13 DIFFERENT, 3 MISSING, 12 NOT APPLICABLE**. DIFFERENT: A25 A34 A36 A43 A45 A46 A47 A51 B08 C11 E10 H03 H05 (all intended or conservative, none changes an output the app depends on; A47 and H03 are the ones to act on). MISSING: FR1 (rewriter per-game patches), FP5 (Snowdrop bare-zlib split), D-TABLES (two bundle tables, diff provided). Rows closed here: P05 (top-up, new), B04/A24 (`screen_aspect`), the warm-up (W, by patch).

**Workspace:** `cargo test --release --workspace --no-fail-fast` (own `CARGO_TARGET_DIR`, loaded Mac) after all changes: **96 suites, 424 passed, 0 failed, 5 ignored**, exit 0. The two all-games oracle tests are env-gated and were run separately with the oracles described in section 12 (187 / 187 profiles, 561 / 561 launcher documents).

## 11. Open items (MISSING or undecided) and what could not be decided

1. **FR1, the rewriter's per-game patches (MISSING, the largest).** `ibuilder-rewrite/src/tables.rs` is `@generated` by `tools/gen_patch_tables.py` from an engine copy older than the patched engine the app ships. Regenerating it from the current engine
   (`python3 -B crates/ibuilder-rewrite/tools/gen_patch_tables.py <engine>/dm > tables.rs`, run here into the scratch folder, **not installed**) adds exactly four lines: Ocarina of Time's `MATCH_CUTS` (the four "movie pictures a frame" cuts),
   `FRAME_STEP` for Ocarina's `main` build `e0f3ccfa...` and for Pokopia's `816c5358...`, and `MATCH_STUBS` for Kirby's Dream Buffet. **Why it was not installed:** the code-valued stubs are hand ports (`stubs.rs`); with the new
   registrations the rewriter would refuse Ocarina's only build ("FRAME_STEP of build ... is not ported") where it now rewrites it, so installing the data without the port turns a working-but-stale build into a stop. What the stale tables
   mean today: the Rust rewrite of Ocarina's `main` uses the function-matched frame step but **lacks `_camera_aspect` (the widescreen / Hor+ stubs behind `screen_aspect wide`, the `ANAMORPHIC_ASPECT` capability) and the four movie cuts**; for Pokopia and
   Kirby's Dream Buffet it silently rewrites without their newest stubs (no error: their tables are not known to it). Nine more games' code-valued builds are unported and refuse explicitly (Hyrule Warriors: Age of Calamity, Xenoblade X,
   Animal Crossing, ASTRAL CHAIN, BotW, Hades II, Kirby's Dream Buffet, TotK, Mario Kart World; only Paper Mario TTYD, Ori and Ocarina's function-matched rules are ported). **Plan:** port `frame_step_v131072` (it calls the already ported
   `frame_step` with fixed `ctx.others`, then `_camera_aspect`: five emitters, about 90 lines) with the existing `tools/stubtest.py` differential harness, then regenerate `tables.rs`. It needs no dump (the harness builds a synthetic NSO with the registered build id), but it runs the real `patch_nso.py`
   in the reference venv (capstone 5.0.7), which was deleted and cannot be recreated here without a download that is not mine to make.
2. **FP5, `paches/engine/0104` (Snowdrop bare zlib shaders), MISSING (verified by grep, not by a run):** the patched `shader_split.py` finds the shaders of a Snowdrop game's bare `.mcache` zlib streams (`#SDFZLIB@`, 4,614 against 623 in South Park: The Fractured But Whole);
   `ibuilder-shadersplit` has the older Snowdrop paths (`snowdrop_wrapped`, SDF slices) and no `SDFZLIB`, so such a game gets a much smaller library (it still builds). Port `snowdrop_shaders`'s bare-stream branch (about 40 Python lines: a 64 KB-block zlib run, wbits 10) with the patch's `test_snowdrop.py` cases.
3. **A47 / A48** `rewrite_jobs` and `shader_jobs_for_memory` constants are the Python's (conservative for Rust): a 3.3 GiB phone rewrites `main` on one thread. Needs a measurement on the phone to retune.
4. **Whether to rebuild automatically** a library built without `bf_want`/`dual_want`/`geo_mesh`/`pso_manifest` G rows (P05 only says so): a rebuild costs minutes to an hour on a phone; left to the player.
5. **Export before the first boot after an update**: `GameExport.Plan` copies `Documents/shaders/<id>` as it is, so an old pack exported before any boot lacks the seed list (the top-up runs at the next boot or build). Closing it needs a C# call into the pipeline; not done.
6. Not decided: whether the 3 `.nup` / merged-update paths (C10) behave on real data (no data here); whether the warm-up should also offer itself once after an export (the user asked for manual only: done as asked).

`df -h` before and after the cleaning is in the report; the scratch folders this audit made (`audit/`, `target-audit/`) were removed at the end.

## 12. Reproduce

```
app/refresh-snapshot.sh --check
cargo test --release -p ibuilder-pipeline                                        # 63 unit + 61 pipeline_spec + 16 resume_spec + ...
# all-games oracles (read-only engine; python3 -B -I):
for id in <each dm/games/<id>>; do GFXAPI=nvn SETTINGS_PLATFORM=ios python3 -B -I <engine>/dm/games/settings.py $id <flat> <no nacp> <out>/$id.nvn.json; done   # also fx, vk variants
LAUNCHER_ORACLE_ALL=<out> LAUNCHER_GAMES=<engine>/dm/games LAUNCHER_FIXUPS=<fixups> cargo test --release -p ibuilder-ffi --features real-stages --test launcher_oracle_all
for id in ...; do python3 -B -I <engine>/dm/games/game.py profile $id <out>/$id.tsv; done
PROFILE_ORACLE_ALL=<out> PROFILE_GAMES=<engine>/dm/games cargo test --release -p ibuilder-profile --test engine_all_games
patch -p1 --dry-run < app/patches/build-app-input-tables.diff                     # from the repository root
```

## 13. Update of 2026-11-19 (evening): FR1, FP5 and what they uncovered

* **FR1 closed.** All 13 code-valued per-game patch files of the shipped engine are ported (`crates/ibuilder-rewrite/src/stubs_games/`: TTYD, Ori, Ocarina of Time with `_camera_aspect` and its 4 movie cuts, Pokopia, Kirby's Dream Buffet, Xenoblade X, Mario Kart World, Hades II, TotK, Age of Calamity, Animal Crossing, BotW, ASTRAL CHAIN); `tables.rs` is regenerated from the shipped engine; nothing is refused any more (an unknown build names the game and says to update the app).
  Checked against the real shipped `patch_nso.py`: 84 synthetic cases (68 equal outputs and logs, 16 stopping with the same message), Ocarina's three real modules byte-identical. `scripts/check-patch-coverage.sh` / `tests/patch_coverage.rs` fail listing any registration the Rust cannot rewrite or any stale table (60 registrations, all covered). Engine patch **0409** (CSEL with an x18 source) is ported (`select.rs`, 300 synthetic modules + real data equal).
  The Grizzy oracle was re-recorded for 0409 (`tests/grizzy.rs`, SHA-256 of the 0409 Python's outputs; all three modules identical at 1, 4 and 8 threads).
* **FP5 closed.** `paches/engine/0104` and the other changes of the rebased `shader_split.py` are ported (`docs/shaders/SHADERSPLIT-RUST.md` section 10): Snowdrop bare zlib, NVN2 ray tracing (stage `rt`), CIVBSF 0x1118, bnd2 v2, Phyre headerless compute, KIT archives, `main` first-copy-wins. Differential on synthetic inputs only (no real game file read).
* **RT library (`rt_library`): the plan is ported, the library is not.** The shipped `dmsh_build.py` collects the map's `raytracing` rows (`RT_KEYS`), keeps them out of the per-shader path and builds ONE universal `rt_library` (`dmsh_rt_build.py`, `dmsh_rt_compile.py`; `dmsh_rt.py` is the metadata reader). Now: the build leaves the RT keys out of the per-shader path (as the Python does) and logs a `[RED] N ray-tracing module(s)` line (`BuildReport::rt_modules`); `ibuilder-shaderbuild/src/rt_build.rs` ports `parse_module`, `make_link_plan` and `make_job` (code at 256-byte boundaries, logical IDs, moves, resource layout, manifest, the translator's `rt` job) and is **identical to the engine's on 93 recorded cases** (`tests/rt_build.rs`, `tools/rt_cases.py`: the answers and the exact text of every refusal). NOT ported: `dmsh_rt_compile` (the translator call with the `rt` job, SPIRV-Cross, the MSL rewrites, `reflection` / `bindings`, the cache digest, the `rt_library_b.metal` / `.json` outputs), which needs the ABI's RT constants (`dmsh_abi.h` of the rebased engine; `finish/abi.rs` is generated from an older header) and `dmsh_rewrite.raw_noinline`. The engine's own tests of that part are mock-based and no real RT module is available here, so it could not be verified end to end. A game with ray-tracing shaders gets no RT library. No game this project targets today uses it (not MK8D, not Ocarina).
* **NEW, MISSING: `jump_solver.py`.** The shipped `patch_nso.py` proves jump tables with a 704-line `jump_solver.py` that `ibuilder-rewrite` does not port; 22 of 40 hand-built jump-table modules differ from the shipped Python (stop vs rewrite), before and after 0409; real data (Ocarina, Grizzy) and the fuzz agree. 5 of the 21 recorded fixtures in `tests/fixtures/synth` no longer reproduce with the shipped Python. Next rewriter item (`docs/crates/REWRITE-RUST.md`).
* **Engine regression, not ours.** The shipped `unzs` (no dictionary) runs the `zstd` command, which the phone's wrapper refuses: those `.zs` shaders are dropped by the Python split on the phone. The Rust split decodes in process and finds them.
* **Review follow-ups** (`docs/REVIEW-TRIAGE-2026-11-19.md`): D1-D4 applied.
* **Workspace:** `scripts/test-report.sh`: **494 passed, 0 failed, 5 ignored; 61 of the 494 skipped (fully or partly) for missing fixtures** (Python oracle, Grizzy, host dylibs). Matrix counts of section 10 are unchanged except FR1 and FP5, now DONE (the matrix of section 10 was not recounted); the two new MISSING items above are listed here, not in the matrix.
