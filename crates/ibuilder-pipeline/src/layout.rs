//! Where everything lives under the app's `Documents` folder. The names are the contract between this crate, the C# app
//! (GameAppMain.cs, GameAppResume.cs, GameAppUi.cs read and write several of them), the shim and `verify_ipa.sh`:
//! do not rename.

use std::path::{Path, PathBuf};

pub const STAGE_FILE: &str = "gameapp.stage";
pub const RUNNING: &str = "gameapp.running";
pub const ARGS: &str = "gameapp.args";
pub const BOOT_ONLY: &str = "gameapp.boot-only";
pub const FRAMEWORK_STAMP: &str = "gameapp.framework";
pub const CONFIG: &str = "gameapp.config";
pub const KILLED: &str = "gameapp.killed";
pub const CONTINUE: &str = "gameapp.continue";
pub const LOAD_BLOCKED: &str = "gameapp.load-blocked";
pub const THERMAL: &str = "gameapp.thermal";
pub const TIMING_PARTIAL: &str = "gameapp.timing.partial";
pub const GUEST_LOG: &str = "gameapp.guest.log";
pub const CHAIN_LOG: &str = "gameapp.chain.log";
pub const STAGES_LOG: &str = "pipeline.stages.log";
pub const UI_LOG: &str = "gameapp.ui.log";
pub const PIPELINE_BOOT: &str = "pipeline.boot";
pub const BUILD_TIMES: &str = "build_times.tsv";
pub const APP_SETTINGS: &str = "gameapp.settings.tsv";
pub const LOGMARKS: &str = "gameapp.logmarks";

#[derive(Clone, Debug)]
pub struct Docs {
    root: PathBuf,
}

impl Docs {
    pub fn new(root: impl Into<PathBuf>) -> Docs {
        Docs { root: root.into() }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn join(&self, name: impl AsRef<Path>) -> PathBuf {
        self.root.join(name)
    }

    pub fn stage_file(&self) -> PathBuf {
        self.join(STAGE_FILE)
    }
    pub fn running(&self) -> PathBuf {
        self.join(RUNNING)
    }
    pub fn args(&self) -> PathBuf {
        self.join(ARGS)
    }
    pub fn args_done(&self) -> PathBuf {
        self.join("gameapp.args.done")
    }
    pub fn boot_only(&self) -> PathBuf {
        self.join(BOOT_ONLY)
    }
    pub fn framework_stamp(&self) -> PathBuf {
        self.join(FRAMEWORK_STAMP)
    }
    pub fn config(&self) -> PathBuf {
        self.join(CONFIG)
    }
    pub fn killed(&self) -> PathBuf {
        self.join(KILLED)
    }
    pub fn continue_file(&self) -> PathBuf {
        self.join(CONTINUE)
    }
    pub fn continue_run(&self) -> PathBuf {
        self.join("gameapp.continue.run")
    }
    pub fn load_blocked(&self) -> PathBuf {
        self.join(LOAD_BLOCKED)
    }
    pub fn thermal(&self) -> PathBuf {
        self.join(THERMAL)
    }
    pub fn timing_partial(&self) -> PathBuf {
        self.join(TIMING_PARTIAL)
    }
    pub fn guest_log(&self) -> PathBuf {
        self.join(GUEST_LOG)
    }
    pub fn chain_log(&self) -> PathBuf {
        self.join(CHAIN_LOG)
    }
    pub fn stages_log(&self) -> PathBuf {
        self.join(STAGES_LOG)
    }
    pub fn pipeline_boot(&self) -> PathBuf {
        self.join(PIPELINE_BOOT)
    }
    pub fn build_times(&self) -> PathBuf {
        self.join(BUILD_TIMES)
    }
    pub fn app_settings(&self) -> PathBuf {
        self.join(APP_SETTINGS)
    }
    pub fn logmarks(&self) -> PathBuf {
        self.join(LOGMARKS)
    }

    /// `Documents/extract`: the last game extracted (ExeFS, RomFS.img, rewrite/, flat/, control.nacp, icon.jpg)
    pub fn extract(&self) -> PathBuf {
        self.join("extract")
    }
    /// `Documents/.nsz`: NSZ conversions (out of the game list)
    pub fn nsz_dir(&self) -> PathBuf {
        self.join(".nsz")
    }
    /// `Documents/profile-derived`: the profiles this app derived, one folder per title id
    pub fn profile_derived(&self) -> PathBuf {
        self.join("profile-derived")
    }
    /// `Documents/profile-derived/<id>`
    pub fn derived_dir(&self, tid: &str) -> PathBuf {
        self.profile_derived().join(tid)
    }
    /// `Documents/games/<id>`
    pub fn game_dir(&self, tid: &str) -> PathBuf {
        self.join("games").join(tid)
    }
    /// `Documents/shaders`: installed shader packs, one folder per title id (`SHADER_PACKS` unless an exported game moves it)
    pub fn shaders(&self) -> PathBuf {
        self.join("shaders")
    }
    /// `Documents/shadercache`: the translation cache every game's stage 3 shares
    pub fn shader_cache(&self) -> PathBuf {
        self.join("shadercache")
    }
    /// `Documents/shader_map.tsv`
    pub fn shader_map(&self) -> PathBuf {
        self.join("shader_map.tsv")
    }
    /// `Documents/env.txt`: free-form engine switches for every game
    pub fn env_txt(&self) -> PathBuf {
        self.join("env.txt")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_the_apps_contract() {
        let d = Docs::new("/D");
        assert_eq!(d.boot_only(), Path::new("/D/gameapp.boot-only"));
        assert_eq!(d.continue_run(), Path::new("/D/gameapp.continue.run"));
        assert_eq!(d.stages_log(), Path::new("/D/pipeline.stages.log"));
        assert_eq!(d.derived_dir("0100"), Path::new("/D/profile-derived/0100"));
        assert_eq!(d.game_dir("0100"), Path::new("/D/games/0100"));
    }
}
