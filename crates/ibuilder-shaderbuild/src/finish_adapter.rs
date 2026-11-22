//! The real finisher: `ibuilder_shader::finish` (package F2, `docs/shaders/SHADER-FINISH-RUST.md` section 1) behind [`Finisher`]. Feature `f2-finish`.
use crate::finish::{Finished, Finisher};
use ibuilder_shader::finish::{Ctx, Options, Partners, finish_partners, finish_with_locs};
use ibuilder_shader::tools::ShaderTools;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

pub struct RustFinisher {
    ctx: Ctx,
    opts_fp: String,
}

impl RustFinisher {
    /// `profile_dir`: the folder holding `game.tsv` and the tables (the game's own folder, as `shader_device.build` copies it); `pulling`: `DMSH_IOS_PULL`
    pub fn load(
        profile_dir: &Path,
        game_id: &str,
        chain: &str,
        pulling: bool,
    ) -> Result<RustFinisher, String> {
        let opts = Options::ios_source_mode(pulling);
        let opts_fp = format!(
            "{}|{}|{}",
            opts.pulling, opts.chain_metal, opts.chain_compiler
        );
        let ctx = Ctx::load(profile_dir, game_id, chain, opts)?;
        Ok(RustFinisher { ctx, opts_fp })
    }
}

impl Finisher for RustFinisher {
    fn fingerprint(&self) -> String {
        // the code's own fingerprint is in the cache digest through build.rs; the options (they decide the reflection's compiler fields) are here
        format!("ibuilder_shader::finish / {}", self.opts_fp)
    }

    fn finish(
        &self,
        key: &str,
        work: &Path,
        job: &mut Value,
        c1: &[u8],
        tools: &dyn ShaderTools,
        paired_locs: &dyn Fn(&str) -> Option<BTreeSet<u32>>,
    ) -> Finished {
        let r = finish_with_locs(&self.ctx, key, work, job, c1, tools, paired_locs);
        Finished {
            status: r.status,
            detail: r.detail,
            refl: r.refl,
            outputs: r.outputs,
            ps_locs: r.ps_locs,
            log: r.log,
        }
    }

    fn finish_with(
        &self,
        key: &str,
        work: &Path,
        job: &mut Value,
        c1: &[u8],
        tools: &dyn ShaderTools,
        partners: &dyn Partners,
    ) -> Finished {
        let r = finish_partners(&self.ctx, key, work, job, c1, tools, partners);
        Finished {
            status: r.status,
            detail: r.detail,
            refl: r.refl,
            outputs: r.outputs,
            ps_locs: r.ps_locs,
            log: r.log,
        }
    }
}
