//! `profile_guess.guess(F)`: the profile keys a game's own files decide, each with the reason written above it in
//! `game.tsv`. Every rule is the Python's; keys left out keep `game.py`'s defaults.
use crate::error::{Error, Result};
use crate::facts::{Facts, ModFacts};
use crate::pyutil::{floordiv, hex};
use crate::syms::HookSet;

pub const APP_POOL: i128 = 0xCD50_0000; // the application memory pool of a 4 GB Switch (3285 MiB)
pub const OUNCE_POOL: i128 = 0x2_4000_0000; // a Switch 2 game's (9 GB of 12 GB, not measured)
pub const MIB: i128 = 1 << 20;
pub const HEAP_API: &str = "_ZN2nn2os17SetMemoryHeapSizeEm";
pub const GPU_DRIVERS: [&str; 2] = ["NintendoSDK_OpenGL", "NintendoSDK_Vulkan"];

/// `{key: (value, reason)}` in profile order (a Python dict: insertion-ordered, `update` replaces in place).
#[derive(Clone, Debug, Default)]
pub struct Guess(pub Vec<(String, (String, String))>);

impl Guess {
    pub fn get(&self, key: &str) -> Option<&(String, String)> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub fn set(&mut self, key: &str, v: (String, String)) {
        if let Some(s) = self.0.iter_mut().find(|(k, _)| k == key) {
            s.1 = v;
        } else {
            self.0.push((key.to_string(), v));
        }
    }
    pub fn iter(&self) -> impl Iterator<Item = &(String, (String, String))> {
        self.0.iter()
    }
}

/// `load_order(m)`: main, the subsdks by number, sdk.
pub fn load_order(m: &str) -> (bool, bool, u64) {
    let n = m
        .strip_prefix("subsdk")
        .filter(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit()))
        .map_or(0, |r| r.parse::<u64>().unwrap_or(u64::MAX));
    (m != "main", m == "sdk", n)
}

/// `engine(F)`: keys that follow from the engine rather than from a general signal.
fn engine(f: &Facts) -> Vec<(&'static str, (String, String))> {
    if f.romfs.root.contains("hagi.config") {
        // NERD's Hagi Wii layer: the pointer is the touch screen in handheld mode
        return vec![
            (
                "op_mode",
                (
                    "handheld".into(),
                    "Hagi (hagi.config): the Wii pointer is the touch screen in handheld mode"
                        .into(),
                ),
            ),
            (
                "pad_layout",
                (
                    "full".into(),
                    "Hagi: a 3D game pad (D-pad, right stick)".into(),
                ),
            ),
        ];
    }
    vec![]
}

fn module<'a>(f: &'a Facts, name: &str) -> &'a ModFacts {
    f.module(name).expect("module present")
}

pub fn guess(f: &Facts, hooks: &HookSet) -> Result<Guess> {
    let main = f
        .module("main")
        .ok_or_else(|| Error::Format("KeyError: 'main' (no main module in the ExeFS)".into()))?;
    let imp = &main.imports;
    let preset = engine(f);
    let mut g = Guess::default();

    // Code modules: a subsdk main takes nothing from (nothing the host does not supply itself) is not loaded
    let mut names: Vec<&str> = f
        .mods
        .iter()
        .map(|(n, _)| n.as_str())
        .filter(|n| *n != "rtld")
        .collect();
    names.sort_by_key(|n| load_order(n)); // stable
    let (mut keep, mut drop): (Vec<&str>, Vec<String>) = (vec![], vec![]);
    for name in names {
        let m = module(f, name);
        let takes = m
            .exports
            .iter()
            .any(|s| imp.contains(s) && !hooks.contains(s));
        if name == "main" || name == "sdk" || (takes && !GPU_DRIVERS.contains(&m.kind.as_str())) {
            keep.push(name);
        } else {
            drop.push(format!("{name} ({})", m.kind));
        }
    }
    g.set(
        "modules",
        (
            keep.join(" "),
            if drop.is_empty() {
                "every module main imports from".to_string()
            } else {
                format!(
                    "not loaded, main takes nothing from them the host does not supply: {}",
                    drop.join(", ")
                )
            },
        ),
    );

    // Memory: the guest block is 256 MB in front + heap region + stack region + alias region (hle_svc.c mem_init)
    let giant: i128 = f
        .mods
        .iter()
        .filter(|(n, m)| keep.contains(&n.as_str()) && m.far)
        .map(|(_, m)| m.bss as i128)
        .sum();
    let heap_api = imp.contains(HEAP_API);
    let pool = if f.ounce { OUNCE_POOL } else { APP_POOL };
    let sysres = f.sysres as i128;
    let mem: (i128, i128, i128, String) = if sysres != 0 {
        // virtual address memory: Engage's 5 GB block. With a heap, the heap check moves the alias / heap split once
        // it sees the heap the game asks for; without one the game maps everything in the alias region itself
        (
            0x1_4000_0000,
            0x1_1800_0000,
            pool,
            format!(
                "the NPDM declares {} MB of system resource (virtual address memory) and main ",
                sysres / MIB
            ) + if heap_api {
                "sets a heap: a 5 GB block with a large alias region; the heap check sizes the heap region from the heap the game asks for"
            } else {
                "never sets a heap: a 5 GB block with a 4.375 GB alias region, where the game maps its memory itself"
            },
        )
    } else {
        let heap: i128 = if heap_api {
            0x1000_0000.max(-floordiv(-(pool - 64 * MIB - giant), 0x1000_0000) * 0x1000_0000)
        } else {
            0x1000_0000
        };
        let why = if heap_api {
            format!(
                "main sets a heap of at most the pool \u{2212} 64 MB{}: a {} MB heap region",
                if giant != 0 {
                    format!(
                        " \u{2212} {} MB of bss (counted as used memory)",
                        giant / MIB
                    )
                } else {
                    String::new()
                },
                heap / MIB
            )
        } else {
            "main never sets a heap (its memory is its own bss): a 256 MB heap region".to_string()
        };
        (0x3000_0000 + heap, 0x1000_0000, pool, why)
    };
    g.set("mem_reserve", (hex(mem.0), mem.3.clone()));
    g.set("mem_alias", (hex(mem.1), String::new()));
    g.set("mem_stack", ("0x10000000".into(), String::new()));
    g.set(
        "app_pool",
        (
            format!("{:#X}", mem.2).replace("0X", "0x"),
            if f.ounce {
                "the sdk is built for the Switch 2 (spec: Ounce): its 9 GB pool".into()
            } else {
                String::new()
            },
        ),
    );
    g.set(
        "sys_resource",
        (
            hex(sysres),
            format!("the NPDM declares {} MB of system resource", sysres / MIB)
                + if sysres != 0 { "" } else { ": IsVirtualAddressMemoryEnabled is false, as on Horizon (Big Brain Academy: Brain vs. Brain sets a heap only then)" },
        ),
    );

    if f.main_markers.contains("Unreal")
        && f.romfs.exts.get(".pak") != 0
        && imp.contains("nvnBootstrapLoader")
    {
        g.set(
            "shader_groups",
            (
                "ShaderArchive-Global-=global ShaderArchive-=game".into(),
                "the decoded shader code archives: the engine's global shaders and the game's material shaders".into(),
            ),
        );
    }
    if f.main_markers.contains("RE Engine") && f.romfs.mags.get("RE Engine PAK") != 0 {
        g.set(
            "shader_groups",
            (
                "re_chunk=sdf".into(),
                "RE Engine: the shaders are SDF definitions inside the KPKA PAKs (shader_split.py kpka_jobs / sdf_shaders), as in Monster Hunter Rise's and PRAGMATA's profiles".into(),
            ),
        );
    }
    // Unreal takes the layout above with no rule of its own (see profile_guess.py for why)
    for (k, v) in preset {
        g.set(k, v);
    }
    Ok(g)
}

/// The text `profile_guess.py <ExeFS> <manifest>` prints: one line per module, the SDK line, then the guessed keys.
pub fn report(f: &Facts, g: &Guess) -> String {
    let mut s = String::new();
    for (n, m) in &f.mods {
        s.push_str(&format!(
            "{n}: {}, build {}, bss {:.1} MB{}, {} imports, {} exports\n",
            m.kind,
            m.bid,
            m.bss as f64 / MIB as f64,
            if m.far { " (far)" } else { "" },
            m.imports.len(),
            m.exports.len()
        ));
    }
    s.push_str(&format!(
        "SDK {}, system resource {} MB, GLSLC outputs in main {}, GLSL sources {}, SPIR-V files {}\n",
        f.sdk,
        f.sysres as i128 / MIB,
        f.glslc,
        f.glsl_src,
        f.romfs.mags.get("SPIR-V")
    ));
    for (k, (v, why)) in g.iter() {
        s.push_str(&format!("{k}\t{v}"));
        if !why.is_empty() {
            s.push_str(&format!("\t# {why}"));
        }
        s.push('\n');
    }
    s
}
