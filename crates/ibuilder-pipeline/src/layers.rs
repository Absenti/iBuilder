//! Stage 3 for games that ship no NVN shader binary and draw through the host's GL or Vulkan layer (`dm/gfx/gl/hle_gl.m`, `dm/gfx/vk/hle_vk.m`): when the NVN split finds
//! no shader, [`ShaderBackend::build_layer`] (ibuilder-glvk) translates the game's GLSL / SPIR-V instead, and the pack holds `vk/` or `gl/` (the table and the MSL of
//! each Metal function) in place of `b-<chain>.img`. `pack.txt` says `layer <api> <layer version>`; the host finds the folder through `DM_MSL_DIR` (set at boot by
//! `shaderpack::set_shader_pack`). The hooks here keep `shaders.rs` to a few calls.

use crate::adapters::{LayerRequest, ShaderBackend, ShaderTools};
use crate::ctx::Ctx;
use crate::shaders::install_pack;
use crate::util::{is_file, read_text, rmtree, utc_now_iso};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The layer folders and their tables, as the host opens them (`<DM_MSL_DIR>/vk/vk_shaders.tsv`, `<DM_MSL_DIR>/gl/gl_shaders.tsv`)
pub const LAYER_TABLES: [(&str, &str); 2] =
    [("vk", "vk/vk_shaders.tsv"), ("gl", "gl/gl_shaders.tsv")];

/// The layer whose table the pack holds
pub fn layer_of(pack: &Path) -> Option<&'static str> {
    LAYER_TABLES
        .iter()
        .find(|(_, t)| is_file(&pack.join(t)))
        .map(|(a, _)| *a)
}

fn pack_layer_line(pack: &Path) -> Option<String> {
    let t = read_text(&pack.join("pack.txt")).ok()?;
    t.lines()
        .find_map(|l| l.strip_prefix("layer\t").map(|s| s.to_string()))
}

/// A layer pack made by this backend's layer version (the split and DLC checks of the pack are `stale_split` / `stale_dlc`, run before)
pub fn current(pack: &Path, version: &str) -> Option<&'static str> {
    let api = layer_of(pack)?;
    (pack_layer_line(pack)? == format!("{api} {version}")).then_some(api)
}

/// The `.none` marker's line that says which layer version tried too (none from a backend without layers, `-`)
pub fn none_line(version: &str) -> String {
    if version == "-" {
        String::new()
    } else {
        format!("layers {version}\n")
    }
}

/// A `.none` marker written before this backend could try the GL / Vulkan layers (or by another layer version): the game is split again
pub fn none_marker_misses_layers(text: &str, version: &str) -> bool {
    version != "-" && !text.contains(&none_line(version))
}

/// The NVN split found nothing: try the layers. `Some(pack)` when the pack now holds a layer library; `None`: not a GL / Vulkan game, or its library could not be
/// made (logged), and the caller writes the `.none` marker as before.
#[allow(clippy::too_many_arguments)]
pub fn try_layers(
    ctx: &Ctx,
    backend: &Arc<dyn ShaderBackend>,
    tools: &ShaderTools,
    tid: &str,
    chain: &str,
    romfs: &Path,
    main_nso: &Path,
    dlc: &[PathBuf],
    dlc_sig: &str,
    split_ver: &str,
    record: &[(String, String)],
    work: &Path,
    jobs: usize,
    pack: &Path,
) -> Option<PathBuf> {
    let version = backend.layer_version();
    if version == "-" {
        return None;
    }
    let mut tmp = pack.as_os_str().to_owned();
    tmp.push(".part");
    let tmp = PathBuf::from(tmp);
    rmtree(&tmp);
    if let Err(e) = fs::create_dir_all(&tmp) {
        ctx.log(&format!("stage 3 shaders: [RED] {}: {e}", tmp.display()));
        return None;
    }
    let lwork = work.join("layer");
    let req = LayerRequest {
        romfs,
        main_nso,
        dlc,
        out_dir: &tmp,
        work: &lwork,
        jobs,
        tools,
    };
    let r = backend.build_layer(&req, &*ctx.log);
    rmtree(&lwork);
    let r = match r {
        Ok(Some(r)) => r,
        Ok(None) => {
            rmtree(&tmp);
            return None;
        }
        Err(e) => {
            ctx.log(&format!(
                "stage 3 shaders: [RED] the GL / Vulkan layer library could not be made: {e}"
            ));
            rmtree(&tmp);
            return None;
        }
    };
    for l in &r.lines {
        ctx.log(&format!("stage 3 shaders: {l}"));
    }
    let mut info = format!(
        "game\t{tid}\nchain\t{chain}\nmade\ton this device, {}\nshaders\t{}\nsplit\t{split_ver}\ndlc\t{dlc_sig}\nlayer\t{} {version}\n",
        utc_now_iso(),
        r.shaders,
        r.api
    );
    // the split's record (which readers the game needed), as a library keeps it: the targeted re-split (`shaders::split_verdict`) reads it
    for (k, v) in record {
        info.push_str(&format!("{k}\t{v}\n"));
    }
    let done = fs::write(tmp.join("pack.txt"), info).and_then(|_| install_pack(&tmp, pack));
    if let Err(e) = done {
        rmtree(&tmp);
        ctx.log(&format!(
            "stage 3 shaders: [RED] the layer pack could not be installed: {e}"
        ));
        return None;
    }
    ctx.log(&format!(
        "stage 3 shaders: Documents/shaders/{tid} holds the game's {} layer library ({} shaders, Metal source): the host's {} layer reads it",
        r.api,
        r.shaders,
        if r.api == "vk" { "Vulkan" } else { "GL" }
    ));
    Some(pack.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_none_marker_from_before_the_layers_is_tried_again() {
        assert!(none_marker_misses_layers(
            "the split found 0 shaders\n",
            "abc"
        ));
        assert!(!none_marker_misses_layers("x\nlayers abc\n", "abc"));
        assert!(none_marker_misses_layers("x\nlayers old\n", "abc"));
        assert!(!none_marker_misses_layers("x\n", "-"));
    }

    #[test]
    fn a_layer_pack_is_current_for_its_version_only() {
        let d = std::env::temp_dir().join(format!("pipeline-layers-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("vk")).unwrap();
        assert_eq!(current(&d, "v1"), None);
        fs::write(d.join("vk/vk_shaders.tsv"), "# t\n").unwrap();
        fs::write(d.join("pack.txt"), "game\tx\nlayer\tvk v1\n").unwrap();
        assert_eq!(layer_of(&d), Some("vk"));
        assert_eq!(current(&d, "v1"), Some("vk"));
        assert_eq!(current(&d, "v2"), None);
        let _ = fs::remove_dir_all(&d);
    }
}
