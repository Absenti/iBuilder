//! `GameAppMain.ConfigText`: the config the chain reads (`Documents/gameapp.config`), rebuilt by the app at every run so a stale one can never be handed over. Ported so a host
//! (the C# app, the CLI, the C test) can make it through one function and the keys stay one list.

use std::path::PathBuf;

/// An exported game (`GameAppExport.cs`) boots from its bundle: its framework, rewritten modules, game files and shader library are in the bundle (read-only); settings, saves and the
/// profile are in Documents. It boots, it never builds.
#[derive(Clone, Debug)]
pub struct PlayerPaths {
    /// `<bundle>/ExportedGame`
    pub bundle_game: PathBuf,
    pub bundle_framework: PathBuf,
    /// the dump the export was made from
    pub dump: String,
    pub romfs: Option<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct ConfigParams {
    pub documents: PathBuf,
    /// the .app bundle (`host-prebuilt.dylib` lives there)
    pub bundle: PathBuf,
    /// the dump being built (file name), empty for a run with no extraction
    pub game: Option<String>,
    pub player: Option<PlayerPaths>,
    /// the game is a Switch 2 `.nup` that exists in Documents: the chain reads it itself
    pub nup_path: Option<PathBuf>,
    /// the build script's id (`GameAppBuild.Id`)
    pub app_build: String,
    /// the translation chain the host was built with (`engine/dm/gfx/nvn/dmsh_chain.h`)
    pub shader_chain: String,
    /// the update and DLC this build takes (`GameAppUi.ContentSignature`)
    pub content: String,
    /// `GAMEAPP_BOOT_SECONDS`: 0 = no limit: a player's game runs until they press Quit game
    pub boot_seconds: Option<String>,
    /// `GAMEAPP_BOOT_AFTER_BUILD=1` boots at the end of the build (harness runs)
    pub boot_after_build: Option<String>,
    /// `GAMEAPP_KEEP_EXEFS=1`: the harness keeps extract/ExeFS for its chain-only runs
    pub keep_exefs: Option<String>,
    /// `GAMEAPP_BOOT_FLOOR_MIB`: the margin the chain demands before it boots the guest IN ITS OWN PROCESS (default 2500: the guest commits ~2 GB of its alias region before the first frames)
    pub boot_floor_mib: Option<String>,
}

pub fn config_text(p: &ConfigParams) -> String {
    let documents = &p.documents;
    let mut extract = documents.join("extract");
    let mut game = p.game.clone();
    let mut extra: Vec<String> = Vec::new();
    let player = p.player.is_some();
    if let Some(pl) = &p.player {
        extract = pl.bundle_game.join("extract");
        game = Some(pl.dump.clone());
        extra.push(format!(
            "shader_packs={}",
            pl.bundle_game.join("shaders").display()
        ));
        extra.push("shader_build=0".to_string());
    } else if let Some(nup) = &p.nup_path {
        // A Switch 2 package: the chain reads it itself (extract_nup) before the profile.
        extra = vec![format!("nup={}", nup.display())];
    }
    let framework = match &p.player {
        Some(pl) => pl.bundle_framework.clone(),
        None => documents.join("DmGameProbe.framework").join("DmGameProbe"),
    };
    let romfs = match &p.player {
        Some(pl) => pl
            .romfs
            .clone()
            .unwrap_or_else(|| extract.join("RomFS.img")),
        None => extract.join("RomFS.img"),
    };
    let d = |o: &Option<String>, default: &str| o.clone().unwrap_or_else(|| default.to_string());
    let lines = vec![
        format!("extract_out={}", extract.display()),
        format!("exe_dir={}", extract.join("ExeFS").display()),
        "modules=main subsdk0 sdk".to_string(),
        format!("patched_dir={}", extract.join("rewrite").display()),
        // the prebuilt host the assembler grafts the game onto
        format!(
            "host_dylib={}",
            p.bundle.join("host-prebuilt.dylib").display()
        ),
        format!("framework_out={}", framework.display()),
        format!("profile={}", documents.join("profile.tsv").display()),
        format!("romfs={}", romfs.display()),
        format!(
            "shader_manifest={}",
            documents.join("shader_manifest").display()
        ),
        format!("boot_seconds={}", d(&p.boot_seconds, "0")),
        format!("boot_after_build={}", d(&p.boot_after_build, "0")),
        format!("keep_exefs={}", d(&p.keep_exefs, "0")),
        format!("boot_floor_mib={}", d(&p.boot_floor_mib, "2500")),
        format!("app_build={}", p.app_build),
        format!("shader_chain={}", p.shader_chain),
        format!("game={}", game.clone().unwrap_or_default()),
        format!(
            "content={}",
            if game.is_none() || player {
                String::new()
            } else {
                p.content.clone()
            }
        ),
    ];
    lines
        .into_iter()
        .chain(extra)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn the_text_is_the_apps_keys_in_the_apps_order() {
        let t = config_text(&ConfigParams {
            documents: "/D".into(),
            bundle: "/B".into(),
            game: Some("Grizzy.nsp".into()),
            app_build: "20261117-173055-c842913".into(),
            shader_chain: "cc0f2129e1de".into(),
            content: "u:1".into(),
            ..Default::default()
        });
        let keys: Vec<&str> = t.lines().map(|l| l.split('=').next().unwrap()).collect();
        assert_eq!(
            keys,
            [
                "extract_out",
                "exe_dir",
                "modules",
                "patched_dir",
                "host_dylib",
                "framework_out",
                "profile",
                "romfs",
                "shader_manifest",
                "boot_seconds",
                "boot_after_build",
                "keep_exefs",
                "boot_floor_mib",
                "app_build",
                "shader_chain",
                "game",
                "content"
            ]
        );
        let c = Config::parse(&t);
        assert_eq!(
            c.get("framework_out"),
            Some("/D/DmGameProbe.framework/DmGameProbe")
        );
        assert_eq!(c.get("host_dylib"), Some("/B/host-prebuilt.dylib"));
        assert_eq!(
            c.get("boot_seconds"),
            Some("0"),
            "no limit by default: a player's game runs until Quit game"
        );
        assert_eq!(c.get("boot_floor_mib"), Some("2500"));
        assert_eq!(c.get("content"), Some("u:1"));
    }

    #[test]
    fn a_nup_and_an_exported_player() {
        let t = config_text(&ConfigParams {
            documents: "/D".into(),
            bundle: "/B".into(),
            game: Some("G.nup".into()),
            nup_path: Some("/D/G.nup".into()),
            ..Default::default()
        });
        assert!(t.ends_with("nup=/D/G.nup\n"));
        let t = config_text(&ConfigParams {
            documents: "/D".into(),
            bundle: "/B".into(),
            game: Some("ignored".into()),
            content: "ignored".into(),
            player: Some(PlayerPaths {
                bundle_game: "/B/ExportedGame".into(),
                bundle_framework: "/B/ExportedGame/fw".into(),
                dump: "Grizzy.nsp".into(),
                romfs: None,
            }),
            ..Default::default()
        });
        let c = Config::parse(&t);
        assert_eq!(c.get("extract_out"), Some("/B/ExportedGame/extract"));
        assert_eq!(c.get("framework_out"), Some("/B/ExportedGame/fw"));
        assert_eq!(c.get("shader_packs"), Some("/B/ExportedGame/shaders"));
        assert_eq!(c.get("shader_build"), Some("0"));
        assert_eq!(c.get("game"), Some("Grizzy.nsp"));
        assert_eq!(c.get("content"), Some(""));
        assert_eq!(c.get("romfs"), Some("/B/ExportedGame/extract/RomFS.img"));
    }
}
