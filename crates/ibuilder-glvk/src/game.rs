//! A whole game: which host layer it draws with (from `main`'s imports, as `dm/games/profile_guess.py shader_sources` decides `graphics`), and its layer library
//! built from the roots the engine's NSP script passes (`build_ipa_from_nsp.py`: the RomFS, `ExeFS/main`, the DLC RomFS images).
//!
//! The decision is the part of `shader_sources` that `main` alone answers:
//! * **vulkan**: `main` imports a `vk*` function other than `vkGetInstanceProcAddr`, or imports only that loader with no `eglCreateWindowSurface` and the RomFS
//!   holds SPIR-V; and `main` does not import `nvnBootstrapLoader`;
//! * **gl**: not vulkan, `main` imports `eglCreateWindowSurface` and not `nvnBootstrapLoader`.
//!
//! Not decided here (the Python needs inputs the phone's chain does not gather): Vulkan looked up through `nn::ro` (`vk_named`), the NROs' imports (Portal: an NVN
//! game whose `main` imports EGL; this module calls it gl, the GL build then finds no GLSL and the game stays "not supported", as before), the GL fixed-function
//! case (`gl_fixed`) and the run-time captures (`vk_capture`, `gl_capture`: the Mac boots the game).

use crate::walk::{self, Root};
use crate::{Api, Fatal, Layer, LayerTools, gl, vk};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const SPIRV_MAGIC: [u8; 4] = 0x07230203u32.to_le_bytes();

/// The undefined dynamic symbols of an NSO (`nso_syms.dynsyms(...)` with `defined` false)
pub fn imports(main: &Path) -> Result<BTreeSet<String>, Fatal> {
    let data = std::fs::read(main).map_err(|e| Fatal(format!("[RED] {}: {e}", main.display())))?;
    let m = ibuilder_nso::load(&data, &main.display().to_string())
        .map_err(|e| Fatal(format!("[RED] {}", e.0)))?;
    let syms = ibuilder_profile::syms::dynsyms(&m.image)
        .map_err(|e| Fatal(format!("[RED] {}: {e}", main.display())))?;
    Ok(syms
        .into_iter()
        .filter(|s| !s.defined)
        .map(|s| s.name)
        .collect())
}

/// Whether any file of the root starts with the SPIR-V magic
pub fn has_spirv(root: &Path) -> Result<bool, Fatal> {
    for d in walk::walk(root)? {
        for f in &d.files {
            if f.len() >= 4 && f.head(4).map(|h| h == SPIRV_MAGIC).unwrap_or(false) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// `shader_sources(...)['graphics']` for what `main` imports (see the module docs): `None` is nvn
pub fn graphics(imports: &BTreeSet<String>, spirv: bool) -> Option<Api> {
    let nvn = imports.contains("nvnBootstrapLoader");
    let egl = imports.contains("eglCreateWindowSurface");
    let vk = imports
        .iter()
        .filter(|s| {
            let b = s.as_bytes();
            b.len() > 2
                && b.starts_with(b"vk")
                && b[2].is_ascii_uppercase()
                && *s != "vkGetInstanceProcAddr"
        })
        .count();
    let vk_loader = imports.contains("vkGetInstanceProcAddr") && spirv && !egl;
    if (vk > 0 || vk_loader) && !nvn {
        return Some(Api::Vk);
    }
    if egl && !nvn {
        return Some(Api::Gl);
    }
    None
}

/// The layer library of a game whose NVN split found nothing: `Ok(None)` when `main` says it draws with NVN, else the library its layer reads (`Err`: the
/// Python's build would have stopped, e.g. `no GLSL shaders in ...`).
pub fn build_for_game(
    romfs: &Path,
    main: &Path,
    dlc: &[PathBuf],
    tools: &dyn LayerTools,
    work: &Path,
    jobs: usize,
) -> Result<Option<Layer>, Fatal> {
    let imp = imports(main)?;
    let spirv = matches!(walk::root(romfs)?, Root::Tree(_)) && has_spirv(romfs)?;
    let Some(api) = graphics(&imp, spirv) else {
        return Ok(None);
    };
    let mut roots = vec![romfs.to_path_buf(), main.to_path_buf()];
    roots.extend(dlc.iter().cloned());
    std::fs::create_dir_all(work).map_err(|e| Fatal(format!("[RED] {}: {e}", work.display())))?;
    Ok(Some(match api {
        Api::Vk => vk::build(&roots, tools, work, jobs)?,
        Api::Gl => gl::build(&roots, tools, work, jobs, false)?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_api_from_the_imports() {
        assert_eq!(
            graphics(&set(&["vkCreateInstance", "vkQueueSubmit"]), false),
            Some(Api::Vk)
        );
        assert_eq!(
            graphics(&set(&["vkCreateInstance", "nvnBootstrapLoader"]), false),
            None
        );
        // Re:ZERO: only the loader and SPIR-V; Balatro: the same loader through SDL, with EGL
        assert_eq!(
            graphics(&set(&["vkGetInstanceProcAddr"]), true),
            Some(Api::Vk)
        );
        assert_eq!(graphics(&set(&["vkGetInstanceProcAddr"]), false), None);
        assert_eq!(
            graphics(
                &set(&["vkGetInstanceProcAddr", "eglCreateWindowSurface"]),
                true
            ),
            Some(Api::Gl)
        );
        assert_eq!(
            graphics(
                &set(&["eglCreateWindowSurface", "nvnBootstrapLoader"]),
                false
            ),
            None
        );
        assert_eq!(graphics(&set(&["nvnBootstrapLoader"]), false), None);
        assert_eq!(graphics(&set(&["vkfoo"]), false), None);
    }
}
