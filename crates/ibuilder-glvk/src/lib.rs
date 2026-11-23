//! `ibuilder-glvk`: stage 3 for the Switch games that ship no NVN shader binary but compile their shaders at run time through the host's GL or Vulkan layer.
//!
//! | module | engine original (`dm/gfx/tools/`) | input | output |
//! |---|---|---|---|
//! | [`vk`] | `vk_shaders.py` | SPIR-V files (RomFS, capture folders), bgfx shaders in `main` and in RenderDragon `.material.bin` files | `vk/vk_shaders.tsv` + MSL |
//! | [`gl`] (+ [`mvgl`]) | `gl_shaders.py` (+ `mvgl.py`) | GLSL: `.vert` / `.frag` / `.glsl` / libGDX `.vs` + `.fs` files, MVGL archives, `#version` strings in `main` | `gl/gl_shaders.tsv` + MSL |
//! | [`idtech4`] | `idtech4_programs.py` | `main` + the RomFS's `base/*.resources` (DOOM 3 BFG) | the `DM_GL_RENDERPROGS` job file |
//!
//! The tables are byte-identical to the Python's. The Python ends with `xcrun metal` / `metallib` (a Mac step); here the translated MSL text of every Metal function is
//! the output instead ([`Layer::msl`], one file per function, named as the Python's temporary `.metal` file), as the NVN chain's library holds MSL text the host compiles on
//! the phone. The host's GL and Vulkan layers load a `.metallib` today: they need a source mode (an engine patch: `<DM_MSL_DIR>/<api>/msl/<name>.metal` compiled at first use, as the NVN layer does).
//!
//! The native tools stay native and are reached through [`tools::LayerTools`]: SPIRV-Cross (in the app: `gameapp_spirv_cross`) and, for GL only, glslang's
//! `glslangValidator` (in the app: `gameapp_glslang`, app/c/glslang_cli.cpp). No process is spawned by the library (`host-tools` adds the Mac's child-process tools).

#![forbid(unsafe_code)]

pub mod game;
pub mod gl;
pub mod idtech4;
pub mod mvgl;
pub mod py;
pub mod spv;
pub mod tools;
pub mod vk;
pub mod walk;

pub use tools::{LayerTools, ToolRun};

use std::path::Path;

/// Which host layer a library is for: the folder name under the pack (`vk/`, `gl/`) and the table's file name (`vk_shaders.tsv`, `gl_shaders.tsv`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Api {
    Vk,
    Gl,
}

impl Api {
    pub fn dir(self) -> &'static str {
        match self {
            Api::Vk => "vk",
            Api::Gl => "gl",
        }
    }
    pub fn table_name(self) -> &'static str {
        match self {
            Api::Vk => "vk_shaders.tsv",
            Api::Gl => "gl_shaders.tsv",
        }
    }
}

/// What a run of [`vk::build`] or [`gl::build`] made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layer {
    pub api: Api,
    /// the table, as the Python writes it (`'\n'.join(tsv) + '\n'`)
    pub table: String,
    /// `(file name, MSL text)` of every Metal function the Python's library holds, in the order the Python passes them to the Metal compiler: `<key>.metal`,
    /// `<key>_d.metal`, `<key>_vp<n>.metal` (Vulkan), `<key>_v.metal` / `<key>_f.metal` (GL). The function in `<name>.metal` is `vk_<name>` / `gl_<name>`.
    pub msl: Vec<(String, String)>,
    /// the lines the Python prints on stdout (`[RED] ... does not translate`, the skipped stages, the summary)
    pub log: Vec<String>,
    /// shaders in the table
    pub shaders: usize,
}

impl Layer {
    /// `<out>/<api>/<api>_shaders.tsv` and `<out>/<api>/msl/<name>.metal`. Files are written whole; an existing `msl/` folder is replaced.
    pub fn write(&self, out: &Path) -> std::io::Result<()> {
        let dir = out.join(self.api.dir());
        let msl = dir.join("msl");
        if msl.is_dir() {
            std::fs::remove_dir_all(&msl)?;
        }
        std::fs::create_dir_all(&msl)?;
        for (name, text) in &self.msl {
            std::fs::write(msl.join(name), text.as_bytes())?;
        }
        std::fs::write(dir.join(self.api.table_name()), self.table.as_bytes())
    }
}

/// The version of what this crate writes, for a pack's `layer` line: a digest of the sources that decide the bytes (a change of any of them makes every layer
/// pack built before it out of date, as `split_version` does for the NVN split)
pub fn layer_version() -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for s in [
        include_str!("vk.rs"),
        include_str!("gl.rs"),
        include_str!("mvgl.rs"),
        include_str!("walk.rs"),
        include_str!("py.rs"),
        include_str!("spv.rs"),
        include_str!("tools.rs"),
        include_str!("game.rs"),
    ] {
        h.update(s.as_bytes());
        h.update([0u8]);
    }
    h.finalize()
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A failure that stops the whole run, as the Python's `red()` (`sys.exit('[RED] ...')`) or an exception it does not catch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fatal(pub String);

impl std::fmt::Display for Fatal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Fatal {}

/// `red(msg)`: the text `sys.exit` prints
pub fn red<T>(msg: impl AsRef<str>) -> Result<T, Fatal> {
    Err(Fatal(format!("[RED] {}", msg.as_ref())))
}
