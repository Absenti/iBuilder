//! `pack_nxgame.py`: assemble a `<title ID>.nxgame` from the halves iOSBuilder produces.
//!
//! The manifest is written exactly as the engine's `gen_manifest.py` does (same keys, order, spelling:
//! `indent=2, ensure_ascii=False` plus a trailing newline) and `package.json` exactly as the Mac app's
//! `PackageWriter` does (Swift `JSONEncoder` prettyPrinted + sortedKeys: `" : "`, alphabetical keys, no trailing
//! newline). Every value that cannot be derived is refused, never guessed.
use crate::json::{Dump, Json, dumps};
use crate::profile::{self, Profile};
use crate::pyutil::py_split_ws;
use crate::romfs_id::{romfs_identity_of_image, romfs_identity_short};
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub const SCHEMA: i128 = 1;
pub const IMAGE_MARKER: &str = "dmgame_image_marker";
pub const PROFILE_NAME: &str = "profile.tsv";
pub const SHADER_MAP: &str = "dm/shader_map.tsv";
pub const LAUNCHER: &str = "dm/launcher.json";
pub const FINDER_METADATA: [&str; 1] = [".DS_Store"];

/// A failure. `Die` is the Python's `die()` (`[RED] pack_nxgame: ...`, exit 1); `Io` is an uncaught OS error.
#[derive(Debug)]
pub enum PackError {
    Die(String),
    Io(String),
}

impl fmt::Display for PackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PackError::Die(m) => write!(f, "[RED] pack_nxgame: {m}"),
            PackError::Io(m) => write!(f, "pack_nxgame: {m}"),
        }
    }
}

impl std::error::Error for PackError {}

/// `d` stays inside the folder it is joined to: not empty, not absolute, no `..`
fn dest_is_inside(d: &str) -> bool {
    use std::path::Component;
    let p = Path::new(d);
    !d.is_empty()
        && !p.is_absolute()
        && p.components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

fn die<T>(m: impl Into<String>) -> Result<T, PackError> {
    Err(PackError::Die(m.into()))
}

trait IoCtx<T> {
    fn ctx(self, what: impl FnOnce() -> String) -> Result<T, PackError>;
}

impl<T> IoCtx<T> for io::Result<T> {
    fn ctx(self, what: impl FnOnce() -> String) -> Result<T, PackError> {
        self.map_err(|e| PackError::Io(format!("{}: {e}", what())))
    }
}

/// The command line of `pack_nxgame.py`.
#[derive(Clone, Debug)]
pub struct PackArgs {
    pub out: PathBuf,
    pub profile: Option<PathBuf>,
    pub framework: Option<PathBuf>,
    pub framework_name: Option<String>,
    pub dm_dir: Option<PathBuf>,
    pub shader_map: Option<PathBuf>,
    pub launcher: Option<PathBuf>,
    pub msl: Option<PathBuf>,
    pub icon: Option<PathBuf>,
    /// `SRC=DEST` entries.
    pub extra: Vec<String>,
    pub romfs_img: Option<PathBuf>,
    pub aoc: Option<PathBuf>,
    pub nro_table: Option<PathBuf>,
    pub game: Option<String>,
    pub name: Option<String>,
    pub display_version: Option<String>,
    pub package_version: Option<String>,
    pub modules: Option<String>,
    pub romfs_marker: Option<String>,
    pub romfs_identity: Option<String>,
    pub chain: Option<String>,
    pub min_os: String,
    pub loader: String,
    pub created_by: String,
    pub no_console: bool,
    pub manifest_only: bool,
    pub force: bool,
    /// APFS `clonefile` for copied files (falls back to a real copy).
    pub clone: bool,
}

impl PackArgs {
    pub fn new(out: impl Into<PathBuf>) -> Self {
        PackArgs {
            out: out.into(),
            profile: None,
            framework: None,
            framework_name: None,
            dm_dir: None,
            shader_map: None,
            launcher: None,
            msl: None,
            icon: None,
            extra: Vec::new(),
            romfs_img: None,
            aoc: None,
            nro_table: None,
            game: None,
            name: None,
            display_version: None,
            package_version: None,
            modules: None,
            romfs_marker: None,
            romfs_identity: None,
            chain: None,
            min_os: "26.0".into(),
            loader: "FH".into(),
            created_by: "iosbuilder pack_nxgame".into(),
            no_console: false,
            manifest_only: false,
            force: false,
            clone: false,
        }
    }
}

/// What a run produced (the Python prints these and, with `--json`, a summary object).
#[derive(Clone, Debug)]
pub struct PackOutcome {
    pub out: PathBuf,
    /// Manifest-only: the file written.
    pub manifest_path: Option<PathBuf>,
    pub manifest: Json,
    pub framework: String,
    pub framework_files: usize,
    pub framework_code_bytes: u64,
    pub romfs_size: u64,
    pub aoc_size: u64,
    pub package_json: Option<String>,
    warnings: Vec<String>,
}

impl PackOutcome {
    /// The lines the Python prints on stdout (without `--json`).
    pub fn stdout_lines(&self) -> Vec<String> {
        if let Some(t) = &self.manifest_path {
            return vec![format!("wrote {}", t.display())];
        }
        vec![
            format!("packed {}", self.out.display()),
            format!(
                "  framework: {} ({} files)",
                self.framework, self.framework_files
            ),
            format!("  manifest keys: {}", self.manifest.keys().join(", ")),
            format!(
                "  romfs_size={} aoc_size={}",
                self.romfs_size, self.aoc_size
            ),
        ]
    }

    /// The extra object `--json` prints (`json.dumps(..., indent=2)`, ensure_ascii on).
    pub fn json_summary(&self) -> String {
        let mut o = Json::obj();
        if let Some(t) = &self.manifest_path {
            o.set("manifest", Json::str(t.display().to_string()));
            o.set("fields", self.manifest.clone());
        } else {
            o.set("out", Json::str(self.out.display().to_string()));
            o.set("framework", Json::str(self.framework.clone()));
            o.set("manifest", self.manifest.clone());
            o.set("framework_files", Json::Int(self.framework_files as i128));
            o.set(
                "framework_code_bytes",
                Json::Int(self.framework_code_bytes as i128),
            );
            o.set("romfs_size", Json::Int(self.romfs_size as i128));
            o.set("aoc_size", Json::Int(self.aoc_size as i128));
        }
        dumps(&o, &Dump::indented(2, true))
    }

    /// Lines the Python prints on stderr (`[warn] ...`).
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

// ---------------------------------------------------------------------------------------------------------------
// the manifest
// ---------------------------------------------------------------------------------------------------------------

fn is_title_id(s: &str) -> bool {
    s.len() == 16
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))
}

fn is_chain(s: &str) -> bool {
    s.strip_prefix("b-").is_some_and(|r| {
        !r.is_empty()
            && r.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    })
}

fn is_framework_name(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b[0].is_ascii_alphabetic()
        && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
}

fn nonempty(a: &Option<String>) -> Option<&str> {
    a.as_deref().filter(|s| !s.is_empty())
}

/// `build_manifest`: the engine `gen_manifest.py` schema, key for key, in its order.
pub fn build_manifest(
    args: &PackArgs,
    prof: &Profile,
    romfs_identity: &str,
) -> Result<Json, PackError> {
    let title_id = nonempty(&args.game)
        .or(prof.get("title_id"))
        .unwrap_or("")
        .to_uppercase();
    if !is_title_id(&title_id) {
        return die(format!(
            "--game/title_id {} is not a 16-digit hex title ID",
            py_repr(&title_id)
        ));
    }
    let name = nonempty(&args.name)
        .or(prof.get("name"))
        .unwrap_or("")
        .to_string();
    if name.is_empty() {
        return die("no game name: pass --name or a profile with a `name` key");
    }
    let modules_s = nonempty(&args.modules)
        .or(prof.get("modules"))
        .unwrap_or("");
    let modules: Vec<&str> = py_split_ws(modules_s);
    if modules.is_empty()
        || modules
            .iter()
            .any(|m| m.is_empty() || !m.bytes().all(|b| b.is_ascii_alphanumeric()))
    {
        return die(format!(
            "modules [{}] names no module or a module with an underscore (the loader's embedded section names cannot hold one)",
            modules
                .iter()
                .map(|m| py_repr(m))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let framework_name = nonempty(&args.framework_name)
        .map(str::to_string)
        .unwrap_or_else(|| format!("DmGame{title_id}"));
    if !is_framework_name(&framework_name) {
        return die(format!(
            "--framework-name {} is not a bundle executable name",
            py_repr(&framework_name)
        ));
    }
    let chain = nonempty(&args.chain).unwrap_or("");
    if !is_chain(chain) {
        return die(format!(
            "chain {} is not written b-<version> (dm/gfx/nvn/dmsh_chain.h)",
            py_repr(chain)
        ));
    }
    let display_version = nonempty(&args.display_version)
        .map(str::to_string)
        .unwrap_or_else(|| {
            prof.display_versions
                .last()
                .map(|d| d.1.clone())
                .unwrap_or_default()
        });
    let package_version = nonempty(&args.package_version).unwrap_or("");
    if package_version.is_empty() {
        return die("no package version: pass --package-version (the profile's `package` key)");
    }
    let romfs_marker = match &args.romfs_marker {
        Some(m) => m.clone(),
        None => prof.get("romfs_marker").unwrap_or("").to_string(),
    };
    let nro_count = match &args.nro_table {
        Some(t) => profile::nro_count(t).ctx(|| format!("reading {}", t.display()))?,
        None => 0,
    };
    let mut d = Json::obj();
    d.set("schema", Json::Int(SCHEMA));
    d.set("title_id", Json::str(title_id.clone()));
    d.set("name", Json::str(name));
    d.set("display_version", Json::str(display_version));
    d.set("package_version", Json::str(package_version));
    d.set(
        "modules",
        Json::Arr(modules.iter().map(|m| Json::str(*m)).collect()),
    );
    d.set("nro_count", Json::Int(nro_count as i128));
    d.set("chain", Json::str(chain));
    d.set("loader", Json::str(args.loader.clone()));
    d.set(
        "framework",
        Json::str(format!("{framework_name}.framework")),
    );
    d.set("framework_executable", Json::str(framework_name));
    d.set("image_marker", Json::str(IMAGE_MARKER));
    d.set("profile", Json::str(PROFILE_NAME));
    d.set("shader_map", Json::str(SHADER_MAP));
    d.set("launcher", Json::str(LAUNCHER));
    d.set("romfs_marker", Json::str(romfs_marker));
    d.set("romfs_identity", Json::str(romfs_identity));
    d.set("min_os", Json::str(args.min_os.clone()));
    if !args.no_console
        && let Some(c) = args.profile.as_deref().and_then(profile::console_of)
    {
        d.set("console", Json::str(c));
    }
    Ok(d)
}

/// Python's `repr()` of a str, for the messages.
fn py_repr(s: &str) -> String {
    let q = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut o = String::from(q);
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if c == q => {
                o.push('\\');
                o.push(c)
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                o.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => o.push(c),
        }
    }
    o.push(q);
    o
}

/// `json.dumps(data, indent=2, ensure_ascii=False) + '\n'`
pub fn manifest_text(manifest: &Json) -> String {
    dumps(manifest, &Dump::indented(2, false)) + "\n"
}

fn write_manifest(path: &Path, manifest: &Json) -> Result<(), PackError> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).ctx(|| format!("mkdir {}", p.display()))?;
    }
    fs::write(path, manifest_text(manifest)).ctx(|| format!("writing {}", path.display()))
}

// ---------------------------------------------------------------------------------------------------------------
// files
// ---------------------------------------------------------------------------------------------------------------

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut f = File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn is_finder_metadata(name: &str) -> bool {
    FINDER_METADATA.contains(&name) || name.starts_with("._")
}

/// Every regular file under `root`, relative path components -> (size, sha256), the Finder's own files left out,
/// sorted by path **component-wise** (Python's `sorted(Path.rglob('*'))` compares `str(p).split('/')`, so
/// `dm/a` sorts before `dm.txt`). A symbolic link is a refusal.
pub fn framework_files(root: &Path) -> Result<Vec<(String, u64, String)>, PackError> {
    let mut found: Vec<(Vec<String>, PathBuf)> = Vec::new();
    fn walk(
        dir: &Path,
        comps: &mut Vec<String>,
        found: &mut Vec<(Vec<String>, PathBuf)>,
    ) -> Result<(), PackError> {
        let mut entries: Vec<_> = fs::read_dir(dir)
            .ctx(|| format!("reading {}", dir.display()))?
            .collect::<Result<_, _>>()
            .ctx(|| format!("reading {}", dir.display()))?;
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            let ft = e.file_type().ctx(|| format!("stat {}", p.display()))?;
            if ft.is_symlink() {
                return die(format!(
                    "{} is a symbolic link; a package holds none",
                    p.display()
                ));
            }
            comps.push(name);
            if ft.is_dir() {
                walk(&p, comps, found)?;
            } else if ft.is_file() {
                let n = comps.last().unwrap();
                if !is_finder_metadata(n) {
                    found.push((comps.clone(), p));
                }
            }
            comps.pop();
        }
        Ok(())
    }
    walk(root, &mut Vec::new(), &mut found)?;
    found.sort_by(|a, b| a.0.cmp(&b.0));
    let hashed: Vec<Result<(String, u64, String), PackError>> = found
        .par_iter()
        .map(|(comps, p)| {
            let size = fs::metadata(p)
                .ctx(|| format!("stat {}", p.display()))?
                .len();
            let sha = sha256_file(p).ctx(|| format!("hashing {}", p.display()))?;
            Ok((comps.join("/"), size, sha))
        })
        .collect();
    hashed.into_iter().collect()
}

fn aoc_bytes(dir: &Path) -> io::Result<u64> {
    let mut total = 0;
    for e in fs::read_dir(dir)? {
        let e = e?;
        let p = e.path();
        let md = fs::metadata(&p)?;
        if md.is_dir() {
            total += aoc_bytes(&p)?;
        } else if md.is_file() && !is_finder_metadata(&e.file_name().to_string_lossy()) {
            total += md.len();
        }
    }
    Ok(total)
}

struct Copier {
    clone: bool,
    warnings: Vec<String>,
}

impl Copier {
    /// `copy_file`: with `clone`, APFS `clonefile`, falling back to a real copy; else `shutil.copy2`.
    fn file(&mut self, src: &Path, dst: &Path) -> Result<(), PackError> {
        if let Some(p) = dst.parent() {
            fs::create_dir_all(p).ctx(|| format!("mkdir {}", p.display()))?;
        }
        if self.clone {
            // clonefile refuses an existing destination (cp -c overwrites): remove it first
            let _ = fs::remove_file(dst);
            match crate::clone::clone_file(src, dst) {
                Ok(()) => return Ok(()),
                Err(e) => self.warnings.push(format!(
                    "[warn] clonefile {} failed ({e}); copying",
                    src.display()
                )),
            }
        }
        fs::copy(src, dst)
            .map(|_| ())
            .ctx(|| format!("copy {} -> {}", src.display(), dst.display()))
    }

    /// `shutil.copytree(src, dst, dirs_exist_ok=True)` (symlinks are followed).
    fn tree(&mut self, src: &Path, dst: &Path) -> Result<(), PackError> {
        fs::create_dir_all(dst).ctx(|| format!("mkdir {}", dst.display()))?;
        for e in fs::read_dir(src).ctx(|| format!("reading {}", src.display()))? {
            let e = e.ctx(|| format!("reading {}", src.display()))?;
            let (p, q) = (e.path(), dst.join(e.file_name()));
            if fs::metadata(&p)
                .ctx(|| format!("stat {}", p.display()))?
                .is_dir()
            {
                self.tree(&p, &q)?;
            } else {
                fs::copy(&p, &q).ctx(|| format!("copy {} -> {}", p.display(), q.display()))?;
            }
        }
        Ok(())
    }
}

fn min_info_plist(framework_name: &str, title_id: &str, min_os: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n<dict>\n\
\t<key>CFBundleExecutable</key><string>{framework_name}</string>\n\
\t<key>CFBundleIdentifier</key><string>org.nxnative.dmgame.{}</string>\n\
\t<key>CFBundlePackageType</key><string>FMWK</string>\n\
\t<key>CFBundleShortVersionString</key><string>1.0</string>\n\
\t<key>CFBundleVersion</key><string>1</string>\n\
\t<key>MinimumOSVersion</key><string>{min_os}</string>\n\
</dict>\n</plist>\n",
        title_id.to_lowercase()
    )
}

/// The packager (`main`).
/// `<path><suffix>` next to `path` (a sibling: the same folder, so a rename never crosses a volume)
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    match path.file_name() {
        // `out/` and `out` are the same folder: the sibling is `<parent>/out.new`, never a folder INSIDE `out`
        Some(n) => {
            let mut s = n.to_owned();
            s.push(suffix);
            path.with_file_name(s)
        }
        None => {
            let mut s = path.as_os_str().to_owned();
            s.push(suffix);
            PathBuf::from(s)
        }
    }
}

/// The staging folder of a package being built: removed again unless the package was put in place.
struct Staging {
    path: PathBuf,
    keep: bool,
}

impl Drop for Staging {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// `new` becomes `dst`: an existing `dst` is moved aside first and put back if the move fails, so the caller has the old package or the new one, never neither.
/// The aside name carries the process id and nothing already there is removed: a folder the user keeps as `<out>.old` is not ours to delete (if the name is taken the move fails and `dst` stays).
fn swap_into_place(new: &Path, dst: &Path) -> Result<(), PackError> {
    if !dst.exists() {
        return fs::rename(new, dst)
            .ctx(|| format!("rename {} -> {}", new.display(), dst.display()));
    }
    let old = sibling(dst, &format!(".old-{}", std::process::id()));
    fs::rename(dst, &old).ctx(|| format!("rename {} -> {}", dst.display(), old.display()))?;
    if let Err(e) = fs::rename(new, dst) {
        let _ = fs::rename(&old, dst);
        return Err(PackError::Io(format!(
            "rename {} -> {}: {e}",
            new.display(),
            dst.display()
        )));
    }
    let _ = fs::remove_dir_all(&old);
    Ok(())
}

pub fn pack(args: &PackArgs) -> Result<PackOutcome, PackError> {
    let out = &args.out;
    let prof = match &args.profile {
        Some(p) => profile::read_profile(p).ctx(|| format!("reading {}", p.display()))?,
        None => Profile::default(),
    };
    // The chain comes from the MSL library's own name when it is not given (msl/b-<chain>.img).
    let mut args = args.clone();
    if nonempty(&args.chain).is_none()
        && let Some(msl) = &args.msl
    {
        let stem = msl
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if stem.ends_with(".img") && stem.starts_with("b-") {
            args.chain = Some(stem[..stem.len() - 4].to_string());
        }
    }
    let args = &args;
    let mut romfs_identity = args.romfs_identity.clone().unwrap_or_default();
    if romfs_identity.is_empty()
        && let Some(img) = &args.romfs_img
    {
        let title = nonempty(&args.game)
            .or(prof.get("title_id"))
            .unwrap_or("")
            .to_uppercase();
        let version = nonempty(&args.display_version)
            .map(str::to_string)
            .unwrap_or_else(|| {
                prof.display_versions
                    .last()
                    .map(|d| d.1.clone())
                    .unwrap_or_default()
            });
        let build = nonempty(&args.package_version).unwrap_or("");
        match romfs_identity_of_image(img, &title, &version, build)
            .ctx(|| format!("reading {}", img.display()))?
        {
            Some(s) => romfs_identity = s,
            None => return die(format!("{} is not a complete RomFS image", img.display())),
        }
    }
    let manifest = build_manifest(args, &prof, &romfs_identity)?;
    let framework = manifest
        .get("framework")
        .and_then(Json::as_str)
        .unwrap()
        .to_string();
    let framework_name = manifest
        .get("framework_executable")
        .and_then(Json::as_str)
        .unwrap()
        .to_string();
    let title_id = manifest
        .get("title_id")
        .and_then(Json::as_str)
        .unwrap()
        .to_string();

    if args.manifest_only {
        let target = if out.extension().is_some_and(|e| e == "json") {
            out.clone()
        } else {
            out.join(&framework).join("game.manifest.json")
        };
        write_manifest(&target, &manifest)?;
        return Ok(PackOutcome {
            out: out.clone(),
            manifest_path: Some(target),
            manifest,
            framework,
            framework_files: 0,
            framework_code_bytes: 0,
            romfs_size: 0,
            aoc_size: 0,
            package_json: None,
            warnings: vec![],
        });
    }

    let Some(fw_src) = &args.framework else {
        return die("--framework is required unless --manifest-only");
    };
    // `--extra SRC=DEST`: DEST is a place INSIDE the framework folder. An absolute DEST, or one with `..`, would write outside the output folder (`fw_dir.join("/x")` is `/x`); refused here,
    // before the output folder is touched (review finding 4.8).
    for spec in &args.extra {
        if let Some((_, d)) = spec.split_once('=')
            && !dest_is_inside(d)
        {
            return die(format!(
                "--extra {}: DEST must be a relative path inside the framework folder (no leading /, no .. component)",
                py_repr(spec)
            ));
        }
    }
    // The package is built in `<out>.new` and put in place at the very end. It used to `rmtree(out)` first and only then find that `--framework` did not exist, an `--extra` source was missing, the
    // disk was full...: a failed run left the caller with neither the old package nor a new one (review 4.7). Nothing of `out` is touched until the new package is complete.
    let final_out = out;
    if final_out.exists() && !args.force {
        return die(format!(
            "{} exists (pass --force to replace it)",
            final_out.display()
        ));
    }
    if final_out.exists() && !final_out.is_dir() {
        // `--force` replaces a package folder, not a file the user keeps under that name: the `rmtree` it replaced failed here, with this message
        return Err(PackError::Io(format!(
            "rmtree {}: {}",
            final_out.display(),
            io::Error::from_raw_os_error(libc::ENOTDIR)
        )));
    }
    let staging_path = sibling(final_out, ".new");
    if staging_path.exists() {
        fs::remove_dir_all(&staging_path).ctx(|| format!("rmtree {}", staging_path.display()))?; // what an earlier failed run left
    }
    let mut staging = Staging {
        path: staging_path,
        keep: false,
    };
    let out = &staging.path.clone();
    let fw_dir = out.join(&framework);
    fs::create_dir_all(&fw_dir).ctx(|| format!("mkdir {}", fw_dir.display()))?;

    let mut cp = Copier {
        clone: args.clone,
        warnings: Vec::new(),
    };
    // the framework's executable (a file) or the whole framework (a directory)
    if !fw_src.exists() {
        return die(format!("--framework {} does not exist", fw_src.display()));
    }
    if fw_src.is_dir() {
        fn rglob(dir: &Path, root: &Path, fw_dir: &Path, cp: &mut Copier) -> Result<(), PackError> {
            for e in fs::read_dir(dir).ctx(|| format!("reading {}", dir.display()))? {
                let p = e.ctx(|| format!("reading {}", dir.display()))?.path();
                let rel = p.strip_prefix(root).unwrap();
                if p.is_dir() {
                    fs::create_dir_all(fw_dir.join(rel))
                        .ctx(|| format!("mkdir {}", fw_dir.join(rel).display()))?;
                    rglob(&p, root, fw_dir, cp)?;
                } else {
                    let dst = fw_dir.join(rel);
                    if let Some(parent) = dst.parent() {
                        fs::create_dir_all(parent).ctx(|| format!("mkdir {}", parent.display()))?;
                    }
                    fs::copy(&p, &dst)
                        .ctx(|| format!("copy {} -> {}", p.display(), dst.display()))?;
                }
            }
            let _ = cp;
            Ok(())
        }
        rglob(fw_src, fw_src, &fw_dir, &mut cp)?;
        if !fw_dir.join(&framework_name).exists() {
            return die(format!(
                "{} holds no executable named {framework_name}",
                fw_src.display()
            ));
        }
    } else {
        let dst = fw_dir.join(&framework_name);
        cp.file(fw_src, &dst)?;
        set_executable(&dst)?;
    }

    // the runtime profile, the dm/ tables, the MSL library, the icon, anything else
    if let Some(p) = &args.profile {
        cp.file(p, &fw_dir.join(PROFILE_NAME))?;
    }
    let dm_dir = fw_dir.join("dm");
    if let Some(d) = &args.dm_dir {
        cp.tree(d, &dm_dir)?;
    }
    if let Some(s) = &args.shader_map {
        fs::create_dir_all(&dm_dir).ctx(|| format!("mkdir {}", dm_dir.display()))?;
        cp.file(s, &dm_dir.join("shader_map.tsv"))?;
    }
    if let Some(s) = &args.launcher {
        fs::create_dir_all(&dm_dir).ctx(|| format!("mkdir {}", dm_dir.display()))?;
        cp.file(s, &dm_dir.join("launcher.json"))?;
    }
    if let Some(m) = &args.msl {
        let d = fw_dir.join("msl");
        fs::create_dir_all(&d).ctx(|| format!("mkdir {}", d.display()))?;
        cp.file(m, &d.join(m.file_name().unwrap_or_default()))?;
    }
    if let Some(i) = &args.icon {
        cp.file(i, &fw_dir.join("icon.png"))?;
    }
    for spec in &args.extra {
        let Some((s, d)) = spec.split_once('=') else {
            return die(format!("--extra {} is not SRC=DEST", py_repr(spec)));
        };
        let dst = fw_dir.join(d);
        if let Some(p) = dst.parent() {
            fs::create_dir_all(p).ctx(|| format!("mkdir {}", p.display()))?;
        }
        if Path::new(s).is_dir() {
            cp.tree(Path::new(s), &dst)?;
        } else {
            cp.file(Path::new(s), &dst)?;
        }
    }

    // a framework with no Info.plist gets the minimal one the verifier reads CFBundleExecutable from
    let plist = fw_dir.join("Info.plist");
    if !plist.exists() {
        let min_os = manifest.get("min_os").and_then(Json::as_str).unwrap_or("");
        fs::write(&plist, min_info_plist(&framework_name, &title_id, min_os))
            .ctx(|| format!("writing {}", plist.display()))?;
    }

    // the game data
    let mut romfs_size = 0;
    if let Some(img) = &args.romfs_img {
        let dst = out.join("romfs.img");
        cp.file(img, &dst)?;
        romfs_size = fs::metadata(&dst)
            .ctx(|| format!("stat {}", dst.display()))?
            .len();
    }
    let mut aoc_size = 0;
    if let Some(aoc) = &args.aoc {
        let dst = out.join("aoc");
        cp.tree(aoc, &dst)?;
        aoc_size = aoc_bytes(&dst).ctx(|| format!("reading {}", dst.display()))?;
        if aoc_size == 0 {
            fs::remove_dir_all(&dst).ctx(|| format!("rmtree {}", dst.display()))?;
        }
    }

    write_manifest(&fw_dir.join("game.manifest.json"), &manifest)?;

    // package.json: the Mac app's own record
    let entries = framework_files(&fw_dir)?;
    let files = Json::Arr(
        entries
            .iter()
            .map(|(p, z, s)| {
                let mut o = Json::obj();
                o.set("path", Json::str(p.clone()));
                o.set("sha256", Json::str(s.clone()));
                o.set("size", Json::Int(*z as i128));
                o
            })
            .collect(),
    );
    let m = |k: &str| manifest.get(k).cloned().unwrap();
    let mut package = Json::obj();
    package.set("schema", Json::Int(SCHEMA));
    package.set("title_id", m("title_id"));
    package.set("name", m("name"));
    package.set("version", m("display_version"));
    package.set("package_version", m("package_version"));
    package.set("framework", m("framework"));
    package.set("chain", m("chain"));
    package.set("min_os", m("min_os"));
    package.set("romfs_id", Json::str(romfs_identity_short(&romfs_identity)));
    package.set("romfs_size", Json::Int(romfs_size as i128));
    package.set("aoc_size", Json::Int(aoc_size as i128));
    package.set("files", files);
    package.set("created_by", Json::str(args.created_by.clone()));
    let package_json = dumps(&package, &Dump::swift_pretty());
    fs::write(out.join("package.json"), &package_json)
        .ctx(|| "writing package.json".to_string())?;
    swap_into_place(out, final_out)?;
    staging.keep = true; // it is the package now

    Ok(PackOutcome {
        out: final_out.clone(),
        manifest_path: None,
        manifest,
        framework,
        framework_files: entries.len(),
        framework_code_bytes: entries.iter().map(|e| e.1).sum(),
        romfs_size,
        aoc_size,
        package_json: Some(package_json),
        warnings: cp.warnings,
    })
}

#[cfg(unix)]
fn set_executable(p: &Path) -> Result<(), PackError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(p, fs::Permissions::from_mode(0o755))
        .ctx(|| format!("chmod {}", p.display()))
}

#[cfg(not(unix))]
fn set_executable(_p: &Path) -> Result<(), PackError> {
    Ok(())
}
