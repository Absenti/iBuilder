//! `DM_PSO_PREPARE_PERCENT` (engine patch 0412): the share of the host's pipeline prewarm built BEFORE the first frame. The app sets, by itself, the share that makes about 500 lines of the seed
//! list (`pso_head_percent`: Ocarina of Time 25,163 lines -> 2, Mario Kart 8 Deluxe 1,516 -> 33) and leaves a list of 500 lines or fewer alone, unless an env.txt line / the environment or the config says otherwise.
//! These tests run on hand-made seed lists (every machine), plus checks on the engine's own `pso_manifest.tsv` files that skip, loudly, when the engine checkout is not there (`ibuilder_testkit::skip`, docs/CONVENTIONS.md).

mod common;

use common::*;
use ibuilder_pipeline::boot::prepare_boot;
use ibuilder_pipeline::env::Env;
use ibuilder_pipeline::profile::*;
use ibuilder_pipeline::settings::apply_env_files;
use std::fs;
use std::path::Path;

const TID: &str = "0400486024DEC000";
const VAR: &str = "DM_PSO_PREPARE_PERCENT";

/// A seed list with `data` data lines (S / R / C rows of the engine's shape) behind a comment header.
fn seed(data: usize) -> String {
    let mut s = String::from("# Prewarm seed list (a hand-made fixture)\n");
    for i in 0..data {
        match i % 3 {
            0 => s.push_str(&format!("S\tk{i:08x}\t{}\n", i % 5)),
            1 => s.push_str(&format!("R\t{i:064x}\n")),
            _ => s.push_str(&format!("C\tc{i:08x}\n")),
        }
    }
    s
}

/// A game on the phone as `prepare_boot` leaves it when it reaches `set_game_env`: the profile names the title, the shader map is the pack's (`DM_NVN_SHMAP`), and the pack holds a seed list of
/// `seed_lines` data lines (`None`: no `pso_manifest.tsv` at all).
fn game(seed_lines: Option<usize>) -> Fx {
    let mut fx = Fx::new();
    fx.profile(&format!("title_id\t{TID}\n"));
    let pack = fx.p(&format!("shaders/{TID}"));
    fx.env.set(
        "DM_NVN_SHMAP",
        &pack.join("shader_map.tsv").to_string_lossy(),
    );
    if let Some(n) = seed_lines {
        write(pack.join("pso_manifest.tsv"), seed(n));
    }
    fx
}

fn pack_manifest(fx: &Fx) -> std::path::PathBuf {
    fx.p(&format!("shaders/{TID}/pso_manifest.tsv"))
}

// ---- the count -----------------------------------------------------------------------------------------------------------------------------

#[test]
fn the_count_is_the_hosts_own_data_lines() {
    // nvn_mtl.m warm_parse: `l.length && ![l hasPrefix:@"#"]` over the text split at "\n"
    let fx = Fx::new();
    let p = fx.p("list.tsv");
    let cases: &[(&[u8], u64)] = &[
        (b"", 0),
        (b"\n", 0),
        (b"\n\n\n", 0),
        (b"a", 1),
        (b"a\n", 1),
        (b"a\nb", 2),
        (b"#only a comment\n", 0),
        (b"# h\n\nS\tk\t1\n\n#c\nR\tabc\nC\tdef", 3),
        (b"a\r\nb\r\n", 2),
        (b"S\t\xff\xfe\t1\n#\xff\n\xc0\n", 2),
        (b"a\n#b\n#c\nd\n\n\ne\n", 3),
    ];
    for (bytes, want) in cases {
        write(&p, bytes);
        assert_eq!(
            seed_list_lines(&p).unwrap(),
            *want,
            "{:?}",
            String::from_utf8_lossy(bytes)
        );
    }
}

#[test]
fn the_count_streams_across_buffer_edges_and_very_long_lines() {
    // the reader keeps a 32 KiB buffer: lines that end exactly on its edge, straddle it, and are longer than it must count as the naive split does
    let fx = Fx::new();
    let p = fx.p("big.tsv");
    let mut text = String::new();
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut next = || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 33) as usize
    };
    for _ in 0..4000 {
        match next() % 10 {
            0 => {}                                                        // an empty line
            1 => text.push_str(&format!("#{}", "c".repeat(next() % 200))), // a comment
            _ => text.push_str(&format!("S{}", "x".repeat(next() % 120))), // a data line
        }
        text.push('\n');
    }
    text.push_str(&format!("S{}\n", "y".repeat(100_000))); // longer than the buffer
    text.push_str(&format!("#{}\n", "z".repeat(70_000))); // a comment longer than the buffer
    text.push_str(&"\n".repeat(40_000)); // blank lines across several buffers
    text.push_str("S-the last line has no newline");
    write(&p, &text);
    assert!(text.len() > 5 * 32 * 1024);
    let naive = text
        .split('\n')
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .count() as u64;
    assert_eq!(seed_list_lines(&p).unwrap(), naive);
}

#[test]
fn a_path_that_is_not_a_readable_file_is_an_error_never_a_panic() {
    let fx = Fx::new();
    assert_eq!(
        seed_list_lines(&fx.p("no/such/list.tsv"))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::NotFound
    );
    fs::create_dir_all(fx.p("a-folder")).unwrap();
    assert!(seed_list_lines(&fx.p("a-folder")).is_err());
}

// ---- the choice -------------------------------------------------------------------------------------------------------------------------------

#[test]
fn a_list_of_500_lines_or_fewer_is_built_whole_before_the_first_frame() {
    for n in [0usize, 1, 499, 500] {
        let fx = game(Some(n));
        set_game_env(&fx.ctx);
        assert_eq!(
            fx.env.get(VAR),
            None,
            "{n} lines: nothing set, the host's default (100) stays"
        );
        assert_eq!(fx.env.get("DM_PSO_PREPARE_TAIL"), None);
        assert!(
            fx.log.any(|l| l.contains(VAR)
                && l.contains("left alone")
                && l.contains(&format!("{n} lines"))
                && l.contains("not more than 500")),
            "{:?}",
            fx.log.lines()
        );
        assert_eq!(
            fx.env.get("DM_PSO_NOWAIT").as_deref(),
            Some("0"),
            "the neighbouring default is untouched"
        );
    }
}

#[test]
fn the_head_is_about_500_lines_whatever_the_length_of_the_list() {
    // the share: ceil(500 * 100 / lines), within 1..=99
    assert_eq!(pso_head_percent(500), None);
    assert_eq!(pso_head_percent(501), Some(99));
    assert_eq!(pso_head_percent(1000), Some(50));
    assert_eq!(pso_head_percent(1516), Some(33)); // Mario Kart 8 Deluxe's list: about 500 lines
    assert_eq!(pso_head_percent(5405), Some(10)); // Nintendo Switch Sports Resort
    assert_eq!(pso_head_percent(14_753), Some(4)); // Ocarina of Time, the engine's list
    assert_eq!(pso_head_percent(25_163), Some(2)); // Ocarina of Time, the pack built on the phone (device log 2026-11-20)
    assert_eq!(pso_head_percent(50_000), Some(1));
    assert_eq!(
        pso_head_percent(10_000_000),
        Some(1),
        "never under 1%: the host's knob cannot say less"
    );
    // the head the host takes is about 500 lines (it rounds per task; here one line = one task)
    for lines in [501u64, 777, 1516, 5405, 14_753, 25_163, 49_999] {
        let p = pso_head_percent(lines).unwrap() as u64;
        let head = (lines * p).div_ceil(100);
        // one percent step is lines / 100 lines: the head is within one step of 500
        let step = lines.div_ceil(100);
        assert!(
            (500 - step..=500 + step).contains(&head),
            "{lines} lines at {p}%: head {head}"
        );
    }
    // a list of any size above 500 lines gets it, with the background pass left on
    for (n, want) in [
        (1516usize, "33"),
        (5405, "10"),
        (14_753, "4"),
        (25_163, "2"),
    ] {
        let fx = game(Some(n));
        set_game_env(&fx.ctx);
        assert_eq!(fx.env.get(VAR).as_deref(), Some(want), "{n} lines");
        assert_eq!(
            fx.env.get("DM_PSO_PREPARE_TAIL"),
            None,
            "the background pass stays the host's default"
        );
    }
    // the config can still ask for another share
    let mut fx = game(Some(5_405));
    fx.set_cfg("pso_prepare_percent", "50");
    set_game_env(&fx.ctx);
    assert_eq!(fx.env.get(VAR).as_deref(), Some("50"));
}

#[test]
fn ocarina_of_time_gets_about_500_lines_and_a_second_call_leaves_it() {
    let fx = game(Some(25_163));
    set_game_env(&fx.ctx);
    assert_eq!(fx.env.get(VAR).as_deref(), Some("2"));
    assert!(
        fx.log.contains("game env: DM_PSO_PREPARE_PERCENT=2 (the seed list holds 25163 lines: about the first 500 (2%) are built before the first frame, the rest in the background while the game runs)"),
        "{:?}",
        fx.log.lines()
    );
    assert_eq!(
        set_pso_prepare_percent(&fx.ctx),
        None,
        "a second call finds it set and leaves it"
    );
    assert!(fx.log.contains("it is already 2"), "{:?}", fx.log.lines());
}

#[test]
fn the_threshold_is_more_than_500_data_lines_and_comments_and_blank_lines_are_not_counted() {
    for (data, want) in [
        (500usize, None),
        (501, Some("99")),
        (499, None),
        (1000, Some("50")),
    ] {
        // the data lines are interleaved with comments and empty lines, and the file has no final newline
        let fx = game(None);
        let mut text = String::from("# header\n# header 2\n\n");
        for i in 0..data {
            text.push_str(&format!("S\tk{i:08x}\t0\n"));
            if i % 50 == 0 {
                text.push_str("# a comment\n\n\n");
            }
        }
        text.push_str("# trailing comment\n\n");
        text.truncate(text.trim_end_matches('\n').len());
        write(pack_manifest(&fx), &text);
        set_game_env(&fx.ctx);
        assert_eq!(
            fx.env.get(VAR).as_deref(),
            want,
            "{data} data lines: {:?}",
            fx.log.lines()
        );
        let n = format!("holds {data} lines");
        assert!(
            fx.log.contains(&n),
            "the log says the data-line count ({n}): {:?}",
            fx.log.lines()
        );
    }
    // 500 data lines + 9 comments + 6 blank lines is 515 lines of text and still not over the head
    let fx = game(None);
    write(
        pack_manifest(&fx),
        format!(
            "{}{}",
            seed(500),
            "# a\n# b\n# c\n# d\n# e\n# f\n# g\n# h\n# i\n\n\n\n\n\n\n"
        ),
    );
    assert_eq!(seed_list_lines(&pack_manifest(&fx)).unwrap(), 500);
    set_game_env(&fx.ctx);
    assert_eq!(fx.env.get(VAR), None);
}

#[test]
fn a_value_already_in_the_environment_wins() {
    for have in ["100", "80", "50", "1"] {
        let fx = game(Some(14_753));
        fx.env.set(VAR, have);
        set_game_env(&fx.ctx);
        assert_eq!(
            fx.env.get(VAR).as_deref(),
            Some(have),
            "a harness / settings value is not replaced"
        );
        assert!(
            fx.log.any(|l| l.contains(VAR)
                && l.contains("left alone")
                && l.contains(&format!("already {have}"))),
            "{:?}",
            fx.log.lines()
        );
    }
    // an empty value is no choice (`if os.environ.get(...)`, as DM_PSO_NOWAIT and DM_FONT_DIR)
    let fx = game(Some(14_753));
    fx.env.set(VAR, "");
    set_game_env(&fx.ctx);
    assert_eq!(fx.env.get(VAR).as_deref(), Some("4"));
}

#[test]
fn an_env_txt_line_wins_and_100_restores_the_old_behaviour() {
    // the real path: the boot reads env.txt (apply_env_files) before set_game_env
    let fx = game(Some(14_753));
    write(
        fx.p(&format!("games/{TID}/env.txt")),
        "# restore the old behaviour\nDM_PSO_PREPARE_PERCENT=100\n",
    );
    assert_eq!(
        apply_env_files(&fx.ctx).get(VAR).map(String::as_str),
        Some("100")
    );
    set_game_env(&fx.ctx);
    assert_eq!(
        fx.env.get(VAR).as_deref(),
        Some("100"),
        "the host reads 100 = all before the first frame"
    );
    assert!(fx.log.contains("left alone"));
    // and an env.txt line with another share is taken as is
    let fx = game(Some(1516));
    write(fx.p("env.txt"), "DM_PSO_PREPARE_PERCENT=25\n");
    apply_env_files(&fx.ctx);
    set_game_env(&fx.ctx);
    assert_eq!(fx.env.get(VAR).as_deref(), Some("25"));
}

#[test]
fn the_config_key_100_leaves_it_alone_and_another_share_is_set() {
    // 100: the host's default, whatever the list
    let mut fx = game(Some(14_753));
    fx.set_cfg("pso_prepare_percent", "100");
    set_game_env(&fx.ctx);
    assert_eq!(fx.env.get(VAR), None);
    assert!(
        fx.log.any(|l| l.contains(VAR)
            && l.contains("left alone")
            && l.contains("pso_prepare_percent=100")),
        "{:?}",
        fx.log.lines()
    );
    // 30: set, with a large list, a short one and none at all (the config is an explicit choice: no count is needed)
    for seed_lines in [Some(14_753), Some(1516), None] {
        let mut fx = game(seed_lines);
        fx.set_cfg("pso_prepare_percent", "30");
        set_game_env(&fx.ctx);
        assert_eq!(fx.env.get(VAR).as_deref(), Some("30"), "{seed_lines:?}");
        assert!(fx.log.contains("game env: DM_PSO_PREPARE_PERCENT=30 (pso_prepare_percent=30 in the config: only the first 30% of the prewarm lists is built before the first frame, the rest in the background)"), "{:?}", fx.log.lines());
    }
    // the config key and 1..=99 bounds
    for n in ["1", "99", " 75 "] {
        let mut fx = game(Some(100));
        fx.set_cfg("pso_prepare_percent", n);
        set_game_env(&fx.ctx);
        assert_eq!(fx.env.get(VAR).as_deref(), Some(n.trim()));
    }
    // an env.txt / harness value still beats the config key
    let mut fx = game(Some(14_753));
    fx.set_cfg("pso_prepare_percent", "30");
    fx.env.set(VAR, "60");
    set_game_env(&fx.ctx);
    assert_eq!(fx.env.get(VAR).as_deref(), Some("60"));
}

#[test]
fn a_config_value_that_is_not_a_share_is_said_and_ignored() {
    for bad in ["abc", "0", "101", "-5", "50.5", "50%", "1e2"] {
        let mut fx = game(Some(14_753));
        fx.set_cfg("pso_prepare_percent", bad);
        set_game_env(&fx.ctx);
        assert_eq!(
            fx.env.get(VAR).as_deref(),
            Some("4"),
            "`{bad}`: the seed list decides"
        );
        assert!(
            fx.log
                .any(|l| l.contains("pso_prepare_percent=") && l.contains("ignored")),
            "{bad}: {:?}",
            fx.log.lines()
        );
        let mut short = game(Some(400));
        short.set_cfg("pso_prepare_percent", bad);
        set_game_env(&short.ctx);
        assert_eq!(short.env.get(VAR), None, "`{bad}` with a short list");
    }
}

#[test]
fn a_missing_or_unreadable_seed_list_sets_nothing_and_says_why() {
    // no pso_manifest.tsv beside the shader map
    let fx = game(None);
    set_game_env(&fx.ctx);
    assert_eq!(fx.env.get(VAR), None);
    assert!(
        fx.log.any(|l| l.contains(VAR)
            && l.contains("left alone")
            && l.contains("pso_manifest.tsv")
            && l.contains("does not exist")),
        "{:?}",
        fx.log.lines()
    );
    assert!(fx.env.truthy("DM_SAVE"), "the rest of set_game_env ran");
    // a folder where the list should be
    let fx = game(None);
    fs::create_dir_all(pack_manifest(&fx)).unwrap();
    set_game_env(&fx.ctx);
    assert_eq!(fx.env.get(VAR), None);
    assert!(
        fx.log.any(|l| l.contains(VAR)
            && l.contains("could not be read")
            && l.contains("not a regular file")),
        "{:?}",
        fx.log.lines()
    );
    assert!(fx.env.truthy("DM_SAVE"));
    // an empty file is a short list, not an error
    let fx = game(None);
    write(pack_manifest(&fx), "");
    set_game_env(&fx.ctx);
    assert_eq!(fx.env.get(VAR), None);
    assert!(fx.log.contains("holds 0 lines"));
}

#[test]
fn no_shader_map_and_no_title_is_said_and_not_fatal() {
    let fx = Fx::new();
    set_game_env(&fx.ctx);
    assert_eq!(fx.env.get(VAR), None);
    assert!(
        fx.log.any(|l| l.contains(VAR) && l.contains("no title id")),
        "{:?}",
        fx.log.lines()
    );
}

#[test]
fn the_list_counted_is_the_one_the_host_reads_beside_its_shader_map() {
    // nvn_mtl.m warm_paths: pso_manifest.tsv beside g_shmap_path (DM_NVN_SHMAP). Without a pack the map is the derived one: that folder's list is the host's
    let fx = game(Some(14_753));
    write(
        fx.p(&format!("profile-derived/{TID}/pso_manifest.tsv")),
        seed(100),
    );
    fx.env.set(
        "DM_NVN_SHMAP",
        &fx.p(&format!("profile-derived/{TID}/shader_map.tsv"))
            .to_string_lossy(),
    );
    set_game_env(&fx.ctx);
    assert_eq!(
        fx.env.get(VAR),
        None,
        "the derived folder's short list is the one the host reads, not the pack's large one"
    );
    assert!(fx.log.contains("holds 100 lines"), "{:?}", fx.log.lines());
    // no DM_NVN_SHMAP yet (set_game_env called on its own): the game's pack folder
    let fx = game(Some(14_753));
    fx.env.remove("DM_NVN_SHMAP");
    set_game_env(&fx.ctx);
    assert_eq!(fx.env.get(VAR).as_deref(), Some("4"));
}

#[test]
fn this_devices_own_list_is_never_counted() {
    // pso_user.tsv grows by play: the share is taken from the seed list alone (the host applies it to both lists)
    let fx = game(Some(400));
    write(fx.p(&format!("games/{TID}/pso_user.tsv")), seed(20_000));
    fx.env.set(
        "DM_PSO_USER",
        &fx.p(&format!("games/{TID}/pso_user.tsv")).to_string_lossy(),
    );
    set_game_env(&fx.ctx);
    assert_eq!(fx.env.get(VAR), None);
    // and a long seed list gets its share whatever this device's list holds
    let fx = game(Some(14_753));
    set_game_env(&fx.ctx);
    assert_eq!(fx.env.get(VAR).as_deref(), Some("4"));
}

#[test]
fn dm_pso_manifest_names_the_list_the_host_reads() {
    // DM_PSO_MANIFEST=<file>: the host reads that file alone; =off: no list at all
    let fx = game(Some(14_753));
    fx.env.set("DM_PSO_MANIFEST", "off");
    set_game_env(&fx.ctx);
    assert_eq!(fx.env.get(VAR), None);
    assert!(
        fx.log
            .any(|l| l.contains(VAR) && l.contains("DM_PSO_MANIFEST=off")),
        "{:?}",
        fx.log.lines()
    );
    let fx = game(Some(400));
    write(fx.p("harness-list.tsv"), seed(12_000));
    fx.env.set(
        "DM_PSO_MANIFEST",
        &fx.p("harness-list.tsv").to_string_lossy(),
    );
    set_game_env(&fx.ctx);
    assert_eq!(
        fx.env.get(VAR).as_deref(),
        Some("5"),
        "the harness's list is long (12,000 lines: 5%), the pack's short one is not read"
    );
    let fx = game(Some(14_753));
    write(fx.p("harness-list.tsv"), seed(10));
    fx.env.set(
        "DM_PSO_MANIFEST",
        &fx.p("harness-list.tsv").to_string_lossy(),
    );
    set_game_env(&fx.ctx);
    assert_eq!(fx.env.get(VAR), None);
}

#[test]
fn the_boot_sequence_sets_it_after_env_txt_and_the_shader_pack() {
    // prepare_boot: env.txt, then the pack (DM_NVN_SHMAP), then set_game_env: the pack's list is found and an env.txt line still wins
    let build = |env_txt: Option<&str>| {
        let fx = game(None);
        let pack = fx.p(&format!("shaders/{TID}"));
        write(pack.join("shader_map.tsv"), rows(3));
        write(pack.join("b-19d183a2b462.img"), "library");
        write(pack.join("pso_manifest.tsv"), seed(14_753));
        fx.env.remove("DM_NVN_SHMAP"); // the boot sets it itself, from the pack
        if let Some(t) = env_txt {
            write(fx.p(&format!("games/{TID}/env.txt")), t);
        }
        prepare_boot(&fx.ctx);
        fx
    };
    let fx = build(None);
    assert_eq!(
        fx.env.get("DM_NVN_SHMAP"),
        Some(
            fx.p(&format!("shaders/{TID}/shader_map.tsv"))
                .to_string_lossy()
                .into_owned()
        )
    );
    assert_eq!(
        fx.env.get(VAR).as_deref(),
        Some("4"),
        "{:?}",
        fx.log.lines()
    );
    // the boot also applies the settings' Profile: Ocarina of Time starts at Memory saver, which turns the background pass off (settings::PRESET_MEMORY_VALUES)
    assert_eq!(fx.env.get("DM_PSO_PREPARE_TAIL").as_deref(), Some("0"));
    let fx = build(Some(
        "DM_PSO_PREPARE_PERCENT=100\nDM_PSO_PREPARE_THREADS=12\n",
    ));
    assert_eq!(
        fx.env.get(VAR).as_deref(),
        Some("100"),
        "{:?}",
        fx.log.lines()
    );
    assert_eq!(
        fx.env.get("DM_PSO_PREPARE_THREADS").as_deref(),
        Some("12"),
        "an env.txt thread count reaches the host untouched"
    );
}

// ---- the game's env.txt starting point (app/profiles) ------------------------------------------------------------------------------------------------

/// The Ocarina of Time env.txt documents the three switches as COMMENTED examples: loaded as the app loads it, it adds no active line, the app still sets its own share (about 500 lines), and the documented
/// `DM_PSO_PREPARE_PERCENT=100` line, uncommented, restores the old behaviour.
#[test]
fn the_ocarina_env_txt_documents_the_switches_without_activating_any() {
    let src =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/profiles/0400486024DEC000.env.txt");
    if !ibuilder_testkit::have(&src) {
        return;
    }
    let text = fs::read_to_string(&src).unwrap();
    for name in [
        "DM_PSO_PREPARE_PERCENT",
        "DM_PSO_PREPARE_TAIL",
        "DM_PSO_PREPARE_THREADS",
    ] {
        assert!(
            text.contains(&format!("# {name}=")),
            "{name} has a commented example in the file"
        );
    }
    let fx = game(Some(14_753));
    write(fx.p(&format!("games/{TID}/env.txt")), &text);
    let got = apply_env_files(&fx.ctx);
    assert_eq!(
        got.keys().map(String::as_str).collect::<Vec<_>>(),
        ["DM_FPS_TARGET", "DM_OPMODE", "DM_SHOW_FPS", "DM_UPSCALER"],
        "only the lines that were active before the block"
    );
    set_game_env(&fx.ctx);
    assert_eq!(
        fx.env.get(VAR).as_deref(),
        Some("4"),
        "the app's own default (about 500 lines), with the file as shipped"
    );
    assert_eq!(fx.env.get("DM_PSO_PREPARE_TAIL"), None);
    assert_eq!(fx.env.get("DM_PSO_PREPARE_THREADS"), None);
    // the line the file tells the player to uncomment
    let restored = text.replace(
        "\n# DM_PSO_PREPARE_PERCENT=100\n",
        "\nDM_PSO_PREPARE_PERCENT=100\n",
    );
    assert_ne!(restored, text, "the documented line is there to uncomment");
    let fx = game(Some(14_753));
    write(fx.p(&format!("games/{TID}/env.txt")), &restored);
    apply_env_files(&fx.ctx);
    set_game_env(&fx.ctx);
    assert_eq!(
        fx.env.get(VAR).as_deref(),
        Some("100"),
        "uncommented, it restores the old behaviour"
    );
}

// ---- the engine's own lists (read-only fixtures) -------------------------------------------------------------------------------------------------

fn engine_list(tid: &str) -> String {
    format!(
        "{}/engine/dm/games/{tid}/pso_manifest.tsv",
        ibuilder_testkit::IOSBUILDER
    )
}

/// The engine's lists: each is longer than 500 lines, so each gets the share that makes about 500 lines before the first frame. The files are read in place (DM_PSO_MANIFEST names them); nothing is copied.
#[test]
fn the_engines_own_lists_fall_on_the_right_sides_of_the_threshold() {
    let (ocarina, sports, mk8d) = (
        engine_list(TID),
        engine_list("04008E001F098000"),
        engine_list("0100152000022000"),
    );
    if !ibuilder_testkit::have(&ocarina)
        || !ibuilder_testkit::have(&sports)
        || !ibuilder_testkit::have(&mk8d)
    {
        return;
    }
    let count = |p: &str| seed_list_lines(Path::new(p)).unwrap();
    let (n_oot, n_sports, n_mk) = (count(&ocarina), count(&sports), count(&mk8d));
    eprintln!(
        "engine lists: Ocarina of Time {n_oot} data lines, Nintendo Switch Sports Resort {n_sports}, Mario Kart 8 Deluxe {n_mk}"
    );
    assert_eq!(
        (n_oot, n_sports, n_mk),
        (14_748, 5_405, 1_516),
        "the lists the engine ships at the pinned revision"
    );
    let fx = game(None);
    fx.env.set("DM_PSO_MANIFEST", &ocarina);
    set_game_env(&fx.ctx);
    assert_eq!(fx.env.get(VAR).as_deref(), Some("4"));
    assert!(fx.log.contains("the seed list holds 14748 lines"));
    for (list, what, want) in [
        (&sports, "Nintendo Switch Sports Resort", "10"),
        (&mk8d, "Mario Kart 8 Deluxe", "33"),
    ] {
        let fx = game(None);
        fx.env.set("DM_PSO_MANIFEST", list);
        set_game_env(&fx.ctx);
        assert_eq!(fx.env.get(VAR).as_deref(), Some(want), "{what}");
    }
}

#[test]
fn a_seed_list_over_10000_lines_leaves_the_pipeline_wait_to_the_host_unless_a_wait_was_chosen() {
    // Ocarina of Time: 25,163 seed lines, a 2% head, and draws that waited without a bound froze frames for up to 161 s
    let big = game(Some(25_163));
    set_game_env(&big.ctx);
    assert_eq!(
        big.env.get("DM_PSO_NOWAIT"),
        None,
        "the host's own policy: wait at most 50 ms a frame, then skip the draw"
    );
    assert!(
        big.log
            .any(|l| l.contains("DM_PSO_NOWAIT left to the host") && l.contains("25163 lines")),
        "{:?}",
        big.log.lines()
    );
    // at the limit and below it a draw still waits (Mario Kart 8 Deluxe: a race drew only its HUD while its libraries compiled)
    for n in [1_516usize, 10_000] {
        let fx = game(Some(n));
        set_game_env(&fx.ctx);
        assert_eq!(
            fx.env.get("DM_PSO_NOWAIT").as_deref(),
            Some("0"),
            "{n} lines: the draw waits"
        );
    }
    // an explicit wait is kept whatever the list
    let mut cfg = game(Some(25_163));
    cfg.ctx.cfg.set("pso_nowait", "0");
    set_game_env(&cfg.ctx);
    assert_eq!(
        cfg.env.get("DM_PSO_NOWAIT").as_deref(),
        Some("0"),
        "pso_nowait=0 in the config"
    );
    let env0 = game(Some(25_163));
    env0.env.set("DM_PSO_NOWAIT", "0");
    set_game_env(&env0.ctx);
    assert_eq!(
        env0.env.get("DM_PSO_NOWAIT").as_deref(),
        Some("0"),
        "an env.txt 0"
    );
    // and a 1 chosen by the player stays
    let env1 = game(Some(1_516));
    env1.env.set("DM_PSO_NOWAIT", "1");
    set_game_env(&env1.ctx);
    assert_eq!(env1.env.get("DM_PSO_NOWAIT").as_deref(), Some("1"));
}
