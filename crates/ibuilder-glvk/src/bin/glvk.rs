//! `glvk`: the Mac command line of the crate, as the engine's tools are called (tools as child processes: `$SPIRV_CROSS` or `spirv-cross`, `$GLSLANG` or `glslangValidator`).
//!
//! ```text
//! glvk vk <SPIR-V dir | RomFS image | NSO>... <out dir>                 -> <out>/vk/vk_shaders.tsv, <out>/vk/msl/*.metal
//! glvk gl <input path>... <out dir> [--fixed-function]                 -> <out>/gl/gl_shaders.tsv, <out>/gl/msl/*.metal
//! glvk idtech4 <main NSO> <RomFS> <job file>                            -> the DM_GL_RENDERPROGS job file (exit 1: no such renderer)
//! ```
use ibuilder_glvk::tools::ProcessLayerTools;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut a: Vec<String> = std::env::args().skip(1).collect();
    if a.is_empty() {
        eprintln!("usage: glvk vk|gl <inputs>... <out dir> | glvk idtech4 <main> <romfs> <job>");
        return ExitCode::from(2);
    }
    let kind = a.remove(0);
    let tools = ProcessLayerTools::from_env();
    let jobs = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    if kind == "idtech4" {
        if a.len() != 3 {
            eprintln!("usage: glvk idtech4 <main NSO> <RomFS> <job file>");
            return ExitCode::from(2);
        }
        return match ibuilder_glvk::idtech4::job(&PathBuf::from(&a[0]), &PathBuf::from(&a[1])) {
            Ok(Some(text)) => {
                if let Err(e) = std::fs::write(&a[2], text.as_bytes()) {
                    eprintln!("{}: {e}", a[2]);
                    return ExitCode::from(1);
                }
                println!("{}", ibuilder_glvk::idtech4::summary(&text));
                ExitCode::SUCCESS
            }
            Ok(None) => {
                println!("no id Tech 4 render program manager in main");
                ExitCode::from(1)
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::from(1)
            }
        };
    }
    let fixed = a.iter().any(|s| s == "--fixed-function");
    a.retain(|s| s != "--fixed-function");
    if a.len() < 2 || (kind != "vk" && kind != "gl") {
        eprintln!("usage: glvk vk|gl <inputs>... <out dir>");
        return ExitCode::from(2);
    }
    let out = PathBuf::from(a.pop().unwrap());
    let roots: Vec<PathBuf> = a.iter().map(PathBuf::from).collect();
    let work = out.join(".glvk-work");
    let _ = std::fs::remove_dir_all(&work);
    if let Err(e) = std::fs::create_dir_all(&work) {
        eprintln!("{}: {e}", work.display());
        return ExitCode::from(1);
    }
    let r = if kind == "vk" {
        ibuilder_glvk::vk::build(&roots, &tools, &work, jobs)
    } else {
        ibuilder_glvk::gl::build(&roots, &tools, &work, jobs, fixed)
    };
    let _ = std::fs::remove_dir_all(&work);
    match r {
        Ok(layer) => {
            for l in &layer.log {
                println!("{l}");
            }
            if let Err(e) = layer.write(&out) {
                eprintln!("{}: {e}", out.display());
                return ExitCode::from(1);
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}
