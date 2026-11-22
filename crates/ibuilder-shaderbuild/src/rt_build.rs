//! The universal NVN2 ray-tracing library's PLAN: `dmsh_rt.py` (`parse_module`, `make_link_plan`) and `dmsh_rt_build.py` (`make_job`) of the shipped engine.
//!
//! The Mac build gives every RT module of a game (`raytracing` rows of the shader map, packages `rt_<group>.bin`) to ONE library, `rt_library`: this module reads each module's control section
//! and code (the RTCore serializer's records), checks them, assigns logical IDs to the continuation regions, concatenates the code at 256-byte boundaries and describes the result as a
//! manifest and as the translator's `rt` job. Nothing here runs a tool or touches a file; `rt_compile.rs` takes the plan from there (translate, SPIRV-Cross, the MSL rewrites, reflection).
//!
//! The reading order, the alignment (of the absolute position in the metadata, as `_Reader.align` does), every `_require` and its message are the Python's: the answer for a module, and the
//! text of the `InvalidModule` it raises, are compared with the engine's own on 93 synthetic cases (`tests/rt_build.rs`, `tools/rt_cases.py`). The metadata reader duplicates
//! `ibuilder-shadersplit/src/rt.rs`, which only validates (the split needs the verdict and the text); the test also checks that the two agree on every module of the cases.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};

pub type R<T> = Result<T, String>;

pub const MANIFEST_VERSION: u64 = 1;
pub const CODE_ALIGNMENT: u64 = 256;
const RESOURCE_SIZES: [(&str, u64); 2] = [("coreParams", 0x140), ("descriptorBaseArray", 0x1430)];

fn require(c: bool, m: &str) -> R<()> {
    if c { Ok(()) } else { Err(m.to_string()) }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn sha(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

fn span<'a>(d: &'a [u8], offset: u64, size: u64, label: &str) -> R<&'a [u8]> {
    let n = d.len() as u64;
    require(
        offset <= n && size <= n - offset,
        &format!("{label} outside input"),
    )?;
    Ok(&d[offset as usize..(offset + size) as usize])
}

// ---- the metadata reader --------------------------------------------------------------------------------------------------------------------

struct Rd<'a> {
    d: &'a [u8],
    pos: usize,
    end: usize,
}

impl<'a> Rd<'a> {
    fn new(d: &'a [u8], start: u64, size: u64) -> R<Rd<'a>> {
        span(d, start, size, "inner module")?;
        Ok(Rd {
            d,
            pos: start as usize,
            end: (start + size) as usize,
        })
    }
    fn take(&mut self, size: u64) -> R<&'a [u8]> {
        require(
            size <= (self.end - self.pos) as u64,
            "truncated inner record",
        )?;
        let r = &self.d[self.pos..self.pos + size as usize];
        self.pos += size as usize;
        Ok(r)
    }
    fn align(&mut self, a: usize) -> R<()> {
        self.take(((a - self.pos % a) % a) as u64).map(|_| ())
    }
    fn u8(&mut self) -> R<u8> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> R<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> R<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn count(&mut self, minimum_size: u64) -> R<u64> {
        self.align(8)?;
        let c = self.u64()?;
        require(
            c <= (self.end - self.pos) as u64 / minimum_size,
            "inner count exceeds remaining input",
        )?;
        Ok(c)
    }
    fn vector(&mut self, stride: u64) -> R<&'a [u8]> {
        let c = self.count(stride)?;
        self.take(c * stride)
    }
    fn name(&mut self) -> R<String> {
        let raw = self.vector(1)?;
        require(
            !raw.is_empty() && !raw.contains(&0),
            "empty or embedded-NUL symbol name",
        )?;
        std::str::from_utf8(raw)
            .map(|s| s.to_string())
            .map_err(|_| "invalid UTF-8 symbol name".to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Region {
    pub offset: u64,
    pub size: u64,
    pub name: String,
    pub selector: i64,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub fields: [u32; 9],
    /// the names of its serialized regions
    pub regions: Vec<String>,
}

impl Entry {
    pub fn kind(&self) -> u32 {
        self.fields[8]
    }
    /// the public owned-module query's entry+0x1c value
    pub fn native_stack_bytes(&self) -> u32 {
        self.fields[1]
    }
    /// the public owned-module query's entry+0x20 value
    pub fn continuation_frame_bytes(&self) -> u32 {
        self.fields[2]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resource {
    pub name: String,
    pub kind: u32,
    pub size: u64,
    pub alignment: u64,
    pub offset: u64,
    pub data: Vec<u8>,
    pub fields: (u32, u64, u32),
}

#[derive(Clone, Debug)]
pub struct Relocation {
    pub patch_offset: u64,
    pub addend: u64,
    pub template: u64,
    pub kind: i32,
    pub region_base: u32,
    pub target: u32,
    pub tail: u32,
}

impl Relocation {
    pub fn instruction_offset(&self) -> u64 {
        self.region_base as u64 + self.patch_offset
    }
}

#[derive(Clone, Debug)]
pub struct Module {
    pub header: [u32; 14],
    pub fingerprint: [u8; 16],
    pub regions: Vec<Region>,
    pub entry_region_counts: Vec<u32>,
    pub entries: Vec<Entry>,
    pub resources: Vec<Resource>,
    pub relocations: Vec<Relocation>,
    pub native_module160_bytes: u32,
    pub module_fields: (u32, u32, u32),
    pub metadata: Vec<u8>,
    pub code: Vec<u8>,
}

/// `_serialized_region`: the region's name
fn serialized_region(r: &mut Rd<'_>) -> R<String> {
    let name = r.name()?;
    r.align(4)?;
    r.take(17 * 4)?;
    r.align(8)?;
    for _ in 0..8 {
        r.u64()?;
    }
    for _ in 0..r.count(72)? {
        r.name()?;
        r.align(4)?;
        r.take(64)?;
    }
    for _ in 0..r.count(16)? {
        r.align(4)?;
        r.u32()?;
        r.align(8)?;
        r.u64()?;
    }
    r.u8()?;
    r.align(4)?;
    r.u32()?;
    r.vector(8)?;
    r.take(2)?;
    Ok(name)
}

fn entry(r: &mut Rd<'_>) -> R<Entry> {
    let name = r.name()?;
    r.align(4)?;
    let mut fields = [0u32; 9];
    for f in fields.iter_mut() {
        *f = r.u32()?;
    }
    r.take(9)?; // flags
    r.align(4)?;
    r.u32()?;
    let mut regions = Vec::new();
    for _ in 0..r.count(80)? {
        regions.push(serialized_region(r)?);
    }
    require(
        fields[0] as u64 == regions.len() as u64,
        "entry region count disagrees with inner records",
    )?;
    r.align(4)?;
    r.u32()?;
    r.vector(12)?;
    r.align(4)?;
    r.take(6)?;
    r.align(4)?;
    r.u32()?;
    Ok(Entry {
        name,
        fields,
        regions,
    })
}

fn resource(r: &mut Rd<'_>) -> R<Resource> {
    let name = r.name()?;
    r.align(4)?;
    let kind = r.u32()?;
    r.align(8)?;
    let (size, alignment, offset) = (r.u64()?, r.u64()?, r.u64()?);
    require(
        alignment > 0 && alignment & (alignment - 1) == 0,
        "resource alignment is not a power of two",
    )?;
    require(offset % alignment == 0, "misaligned resource offset")?;
    require(offset <= u64::MAX - size, "resource extent overflows u64")?;
    let data = r.vector(1)?.to_vec();
    r.align(4)?;
    let f0 = r.u32()?;
    r.align(8)?;
    let f1 = r.u64()?;
    let f2 = r.u32()?;
    Ok(Resource {
        name,
        kind,
        size,
        alignment,
        offset,
        data,
        fields: (f0, f1, f2),
    })
}

type Inner = ([u8; 16], Vec<Entry>, Vec<Resource>, u32, (u32, u32, u32));

fn inner(metadata: &[u8], start: u64, size: u64) -> R<Inner> {
    let mut r = Rd::new(metadata, start, size)?;
    let fingerprint: [u8; 16] = r.take(16)?.try_into().unwrap();
    r.vector(1)?;
    r.align(8)?;
    r.take(0xc5)?; // fixed compiler/options record
    r.vector(1)?; // opaque module+0x108 string
    r.take(3)?;
    r.align(4)?;
    r.u32()?;
    require(
        r.count(1)? == 0,
        "unsupported nonempty module parameter list",
    )?;
    r.align(4)?;
    r.u32()?;
    r.name()?; // module+0x148 name
    r.align(8)?;
    let fixed = r.take(0x24)?;
    let module160 = u32::from_le_bytes(fixed[8..12].try_into().unwrap());
    r.align(4)?;
    r.u32()?;
    for _ in 0..r.count(48)? {
        r.align(4)?;
        r.take(8)?;
        r.vector(8)?;
        r.vector(4)?;
        r.align(4)?;
        r.take(9)?;
        r.vector(24)?;
        r.vector(4)?;
    }
    let mut entries = Vec::new();
    for _ in 0..r.count(80)? {
        entries.push(entry(&mut r)?);
    }
    for _ in 0..r.count(20)? {
        r.name()?;
        r.align(4)?;
        r.take(12)?;
    }
    r.u8()?;
    for _ in 0..2 {
        for _ in 0..r.count(16)? {
            r.name()?;
            r.align(8)?;
            r.u64()?;
        }
    }
    for _ in 0..r.count(16)? {
        r.name()?;
        for _ in 0..r.count(8)? {
            r.name()?;
        }
    }
    r.align(4)?;
    let (module250, module254) = (r.u32()?, r.u32()?);
    let mut resources = Vec::new();
    for _ in 0..r.count(56)? {
        resources.push(resource(&mut r)?);
    }
    r.u8()?;
    r.align(4)?;
    let module278 = r.u32()?;
    r.vector(1)?;
    require(r.pos == r.end, "unconsumed inner metadata variant")?;
    Ok((
        fingerprint,
        entries,
        resources,
        module160,
        (module250, module254, module278),
    ))
}

/// `parse_module(metadata, code, expected_fingerprint=None)`: validate and inspect a module; unknown numeric fields are kept unchanged.
pub fn parse_module(
    metadata: &[u8],
    code: &[u8],
    expected_fingerprint: Option<&[u8]>,
) -> R<Module> {
    require(
        !code.is_empty() && code.len().is_multiple_of(16),
        "code is empty or not 16-byte aligned",
    )?;
    let hb = span(metadata, 0, 56, "outer header")?;
    let mut h = [0u32; 14];
    for (i, c) in hb.chunks(4).enumerate() {
        h[i] = u32::from_le_bytes(c.try_into().unwrap());
    }
    require(h[3] >= 56 && h[3] % 8 == 0, "misaligned inner metadata")?;
    require(h[4] > 0, "invalid outer region count")?;
    require(0 < h[11] && h[11] <= h[4], "invalid outer entry count")?;
    let words = |offset: u32, count: u32, label: &str| -> R<Vec<u32>> {
        require(offset.is_multiple_of(4), &format!("misaligned {label}"))?;
        let n = metadata.len() as u64;
        require(
            count as u64 <= (n - (offset as u64).min(n)) / 4,
            &format!("{label} count exceeds input"),
        )?;
        let b = span(metadata, offset as u64, count as u64 * 4, label)?;
        Ok(b.chunks(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect())
    };
    let starts = words(h[5], h[4], "region starts")?;
    let sizes = words(h[6], h[4], "region sizes")?;
    let names = words(h[7], h[4], "region names")?;
    let selectors = words(h[8], h[4], "region selectors")?;
    let entry_counts = words(h[12], h[11], "entry region counts")?;
    require(
        entry_counts.iter().all(|&c| c != 0)
            && entry_counts.iter().map(|&c| c as u64).sum::<u64>() == h[4] as u64,
        "entry regions do not cover region table",
    )?;
    let mut regions: Vec<Region> = Vec::new();
    for i in 0..h[4] as usize {
        let (offset, size, name_offset, selector) = (starts[i], sizes[i], names[i], selectors[i]);
        require(
            offset % 16 == 0 && size % 16 == 0 && size > 0,
            "unaligned or empty region",
        )?;
        span(code, offset as u64, size as u64, "region code")?;
        require(
            (name_offset as usize) < metadata.len(),
            "region name outside metadata",
        )?;
        let end = metadata[name_offset as usize..]
            .iter()
            .position(|&b| b == 0)
            .map(|p| p + name_offset as usize);
        require(
            matches!(end, Some(e) if e > name_offset as usize),
            "empty or unterminated region name",
        )?;
        let name = std::str::from_utf8(&metadata[name_offset as usize..end.unwrap()])
            .map_err(|_| "invalid UTF-8 region name".to_string())?;
        let selector = if selector < (1u32 << 31) {
            selector as i64
        } else {
            selector as i64 - (1i64 << 32)
        };
        regions.push(Region {
            offset: offset as u64,
            size: size as u64,
            name: name.to_string(),
            selector,
        });
    }
    let (fingerprint, entries, resources, module160, module_fields) =
        inner(metadata, h[3] as u64, h[2] as u64)?;
    if let Some(e) = expected_fingerprint {
        require(e.len() == 16, "expected fingerprint must contain 16 bytes")?;
        require(fingerprint[..] == *e, "inner fingerprint mismatch")?;
    }
    require(
        entries.len() as u64 == h[11] as u64,
        "inner/outer entry count mismatch",
    )?;
    require(
        entries
            .iter()
            .map(|e| e.regions.len() as u64)
            .eq(entry_counts.iter().map(|&c| c as u64)),
        "inner/outer region count mismatch",
    )?;
    require(
        entries
            .iter()
            .flat_map(|e| e.regions.iter())
            .eq(regions.iter().map(|r| &r.name)),
        "inner/outer region names disagree",
    )?;
    let raw = span(metadata, h[10] as u64, h[9] as u64 * 40, "relocation table")?;
    require(h[10] % 8 == 0, "misaligned relocation table")?;
    let mut relocations = Vec::new();
    for i in 0..h[9] as usize {
        let b = &raw[40 * i..40 * i + 40];
        let rel = Relocation {
            patch_offset: u64::from_le_bytes(b[0..8].try_into().unwrap()),
            addend: u64::from_le_bytes(b[8..16].try_into().unwrap()),
            template: u64::from_le_bytes(b[16..24].try_into().unwrap()),
            kind: i32::from_le_bytes(b[24..28].try_into().unwrap()),
            region_base: u32::from_le_bytes(b[28..32].try_into().unwrap()),
            target: u32::from_le_bytes(b[32..36].try_into().unwrap()),
            tail: u32::from_le_bytes(b[36..40].try_into().unwrap()),
        };
        let sources: Vec<&Region> = regions
            .iter()
            .filter(|r| r.offset == rel.region_base as u64)
            .collect();
        require(!sources.is_empty(), "relocation base has no region")?;
        require(
            rel.patch_offset.is_multiple_of(16)
                && sources.iter().any(|r| rel.patch_offset + 16 <= r.size),
            "relocation instruction outside region",
        )?;
        span(code, rel.instruction_offset(), 16, "relocation instruction")?;
        if rel.kind < 0 {
            require(
                (rel.target as usize) < regions.len(),
                "logical relocation target outside module",
            )?;
        } else {
            require(
                (rel.target as usize) < code.len(),
                "code relocation target outside module",
            )?;
        }
        relocations.push(rel);
    }
    Ok(Module {
        header: h,
        fingerprint,
        regions,
        entry_region_counts: entry_counts,
        entries,
        resources,
        relocations,
        native_module160_bytes: module160,
        module_fields,
        metadata: metadata.to_vec(),
        code: code.to_vec(),
    })
}

// ---- the link plan --------------------------------------------------------------------------------------------------------------------------

/// `RelocationKind.LOGICAL_ID_MOV`
const LOGICAL_ID_MOV: i32 = -56;

#[derive(Clone, Debug)]
pub struct LinkedRegion {
    pub logical_id: u32,
    pub entry_index: usize,
    pub region_index: usize,
    pub region_offset: u64,
}

#[derive(Clone, Debug)]
pub struct PlannedRelocation {
    pub instruction_offset: u64,
    pub template: u64,
    pub target_logical_id: u32,
}

#[derive(Clone, Debug)]
pub struct LinkPlan {
    pub regions: Vec<LinkedRegion>,
    pub relocations: Vec<PlannedRelocation>,
}

/// `make_link_plan(module, logical_ids, expected_fingerprint=...)`: an immutable namespace of IDs over the module's regions (no code is patched).
pub fn make_link_plan(
    module: &Module,
    ids: &[u32],
    expected_fingerprint: Option<&[u8]>,
) -> R<LinkPlan> {
    require(
        ids.len() == module.regions.len(),
        "logical ID count differs from region count",
    )?;
    require(
        ids.iter().all(|&i| 0 < i && i < 0x8000),
        "logical ID outside positive signed16 domain",
    )?;
    require(
        ids.iter().collect::<HashSet<_>>().len() == ids.len(),
        "duplicate logical IDs",
    )?;
    if let Some(e) = expected_fingerprint {
        require(
            e.len() == 16 && e == module.fingerprint,
            "link fingerprint mismatch",
        )?;
    }
    require(
        module.header[13] == 0 || module.header[13] == 1,
        "unsupported outer metadata flag",
    )?;
    require(
        module.module_fields == (0, 3, 7),
        "unsupported executable module fields",
    )?;
    require(
        module.regions.iter().all(|r| r.selector == -1),
        "unsupported internal region selector",
    )?;
    require(
        module.entries.iter().all(|e| matches!(e.kind(), 0 | 7 | 8)),
        "unsupported executable entry kind",
    )?;
    require(
        module.resources.iter().all(|r| r.kind == 3),
        "unsupported executable resource kind",
    )?;
    require(
        module
            .resources
            .iter()
            .all(|r| r.data.len() as u64 <= r.size),
        "resource initializer exceeds extent",
    )?;
    require(
        module
            .resources
            .iter()
            .map(|r| &r.name)
            .collect::<HashSet<_>>()
            .len()
            == module.resources.len(),
        "duplicate resource symbols",
    )?;
    require(
        module
            .regions
            .iter()
            .map(|r| &r.name)
            .collect::<HashSet<_>>()
            .len()
            == module.regions.len(),
        "duplicate region symbols",
    )?;
    let mut ordered: Vec<&Region> = module.regions.iter().collect();
    ordered.sort_by_key(|r| r.offset);
    require(
        ordered
            .windows(2)
            .all(|w| w[0].offset + w[0].size <= w[1].offset),
        "unsupported overlapping executable regions",
    )?;
    let mut linked = Vec::new();
    let mut cursor = 0usize;
    for (ei, &count) in module.entry_region_counts.iter().enumerate() {
        let part = &ids[cursor..cursor + count as usize];
        require(
            part.iter()
                .enumerate()
                .all(|(k, &v)| v as u64 == part[0] as u64 + k as u64),
            "continuation IDs are not contiguous",
        )?;
        for ri in cursor..cursor + count as usize {
            linked.push(LinkedRegion {
                logical_id: ids[ri],
                entry_index: ei,
                region_index: ri,
                region_offset: module.regions[ri].offset,
            });
        }
        cursor += count as usize;
    }
    let mut plans = Vec::new();
    let mut sites = HashSet::new();
    for r in &module.relocations {
        require(
            r.kind == LOGICAL_ID_MOV,
            &format!("unsupported executable relocation kind {}", r.kind),
        )?;
        require(
            r.addend == 0 && r.tail == 0,
            "unsupported logical MOV addend or trailing field",
        )?;
        require(
            r.template < (1u64 << 32) && matches!(r.template & 0xfff, 0x802 | 0x882),
            "unsupported logical MOV template",
        )?;
        let at = r.instruction_offset() as usize;
        let source = u64::from_le_bytes(module.code[at..at + 8].try_into().unwrap());
        require(
            source == r.template,
            "logical MOV template disagrees with original instruction",
        )?;
        require(
            sites.insert(r.instruction_offset()),
            "duplicate relocation instruction",
        )?;
        plans.push(PlannedRelocation {
            instruction_offset: r.instruction_offset(),
            template: r.template,
            target_logical_id: ids[r.target as usize],
        });
    }
    Ok(LinkPlan {
        regions: linked,
        relocations: plans,
    })
}

// ---- make_job -------------------------------------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceLayout {
    pub name: String,
    pub kind: u32,
    pub size: u64,
    pub alignment: u64,
    pub offset: u64,
    pub data: Vec<u8>,
    pub fields: (u32, u64, u32),
}

#[derive(Clone, Debug)]
pub struct PlannedEntry {
    pub address: u64,
    pub id: u32,
    pub owner: u32,
    pub kind: u32,
    pub frame: u32,
    pub entry_index: usize,
    pub region_index: usize,
}

impl PlannedEntry {
    /// `job_record()`: what the translator's job carries
    pub fn job_record(&self) -> Value {
        json!({"address": self.address, "id": self.id, "owner": self.owner, "kind": self.kind, "frame": self.frame})
    }
    fn full_record(&self) -> Value {
        json!({"address": self.address, "id": self.id, "owner": self.owner, "kind": self.kind, "frame": self.frame, "entry_index": self.entry_index, "region_index": self.region_index})
    }
}

#[derive(Clone, Debug)]
pub struct PlannedModule {
    pub key: u64,
    pub address: u64,
    pub length: u64,
    pub control_sha256: String,
    pub metadata_sha256: String,
    pub code_sha256: String,
    pub entries: Vec<PlannedEntry>,
}

#[derive(Clone, Debug)]
pub struct PlannedMove {
    pub address: u64,
    pub template: u64,
    pub id: u32,
}

#[derive(Clone, Debug)]
pub struct BuiltJob {
    pub code: Vec<u8>,
    pub modules: Vec<PlannedModule>,
    pub moves: Vec<PlannedMove>,
    pub layout: Vec<ResourceLayout>,
    pub fingerprint: [u8; 16],
    pub local_memory_bytes: u32,
}

impl BuiltJob {
    fn offset_of(&self, name: &str) -> u64 {
        self.layout
            .iter()
            .find(|r| r.name == name)
            .map(|r| r.offset)
            .unwrap_or(0)
    }

    /// `core_offset` of the translator's job
    pub fn core_offset(&self) -> u64 {
        self.offset_of("coreParams")
    }

    /// `descriptor_offset` of the translator's job
    pub fn descriptor_offset(&self) -> u64 {
        self.offset_of("descriptorBaseArray")
    }

    /// `rt`: the translator job (a fresh value each call)
    pub fn rt(&self) -> Value {
        json!({
            "core_offset": self.core_offset(),
            "descriptor_offset": self.descriptor_offset(),
            "modules": self.modules.iter().map(|m| json!({"address": m.address, "length": m.length})).collect::<Vec<_>>(),
            "entries": self.modules.iter().flat_map(|m| m.entries.iter()).map(|e| e.job_record()).collect::<Vec<_>>(),
            "moves": self.moves.iter().map(|m| json!({"address": m.address, "template": m.template, "id": m.id})).collect::<Vec<_>>(),
        })
    }

    /// `manifest`: our versioned manifest, without interpreting RTCore's fingerprint
    pub fn manifest(&self) -> Value {
        json!({
            "version": MANIFEST_VERSION,
            "fingerprint": hex(&self.fingerprint),
            "code_alignment": CODE_ALIGNMENT,
            "code_bytes": self.code.len(),
            "code_sha256": sha(&self.code),
            "local_memory_bytes": self.local_memory_bytes,
            "bank_bytes": self.layout.iter().map(|r| r.offset + r.size).max().unwrap_or(0),
            "resources": self.layout.iter().map(|r| json!({"name": r.name, "kind": r.kind, "size": r.size, "alignment": r.alignment, "offset": r.offset, "data": hex(&r.data), "fields": [r.fields.0, r.fields.1, r.fields.2]})).collect::<Vec<_>>(),
            "modules": self.modules.iter().map(|m| json!({
                "key": format!("{:016x}", m.key), "address": m.address, "length": m.length,
                "control_sha256": m.control_sha256, "metadata_sha256": m.metadata_sha256, "code_sha256": m.code_sha256,
                "entries": m.entries.iter().map(|e| e.full_record()).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "rt": self.rt(),
        })
    }
}

/// `_module(control, code)`: one record's control section and code, checked, as a parsed module
fn module_of(control: &[u8], code: &[u8]) -> R<Module> {
    require(control.len() >= 0x68, "truncated RT control header")?;
    let u32_at = |o: usize| u32::from_le_bytes(control[o..o + 4].try_into().unwrap());
    require(
        u32_at(0) == 0x98761234 && u32_at(4) == 0xc9,
        "unsupported RT control schema",
    )?;
    require(control[0x56] == 8, "RT control is not stage 8")?;
    let (size, offset, code_size) = (
        u32_at(0x5c) as u64,
        u32_at(0x60) as u64,
        u32_at(0x64) as u64,
    );
    require(
        size > 0
            && offset >= 0x68
            && offset % 8 == 0
            && offset <= control.len() as u64
            && size <= control.len() as u64 - offset,
        "RT metadata extent outside control",
    )?;
    require(
        code_size == code.len() as u64 && u32_at(0x44) as u64 == code.len() as u64,
        "RT control code size disagrees with code bytes",
    )?;
    let module = parse_module(
        &control[offset as usize..(offset + size) as usize],
        code,
        None,
    )?;
    require(
        module.native_module160_bytes == 0
            && module.entries.iter().all(|e| e.native_stack_bytes() == 0),
        "unsupported native RT stack requirement",
    )?;
    require(
        module
            .entries
            .iter()
            .all(|e| e.kind() == 0 || e.regions.len() == 1),
        "unsupported non-raygen continuation regions",
    )?;
    require(
        module
            .entries
            .iter()
            .all(|e| e.continuation_frame_bytes() <= 0x10000),
        "RT continuation frame exceeds 64 KiB",
    )?;
    Ok(module)
}

/// `_layout(module)`: the named resources, by offset then name
fn layout_of(module: &Module) -> R<Vec<ResourceLayout>> {
    let mut resources: Vec<ResourceLayout> = module
        .resources
        .iter()
        .map(|r| ResourceLayout {
            name: r.name.clone(),
            kind: r.kind,
            size: r.size,
            alignment: r.alignment,
            offset: r.offset,
            data: r.data.clone(),
            fields: r.fields,
        })
        .collect();
    resources.sort_by(|a, b| (a.offset, &a.name).cmp(&(b.offset, &b.name)));
    let names: HashSet<&str> = resources.iter().map(|r| r.name.as_str()).collect();
    let wanted: HashSet<&str> = RESOURCE_SIZES.iter().map(|r| r.0).collect();
    require(
        names == wanted && resources.len() == RESOURCE_SIZES.len(),
        "unsupported or missing RT resource symbols",
    )?;
    require(
        resources.iter().all(|r| {
            r.kind == 3
                && RESOURCE_SIZES
                    .iter()
                    .any(|(n, s)| *n == r.name && *s == r.size)
        }),
        "unsupported named RT resource extent or kind",
    )?;
    require(
        resources
            .windows(2)
            .all(|w| w[0].offset + w[0].size <= w[1].offset),
        "RT resource extents overlap",
    )?;
    Ok(resources)
}

/// One input of `make_job`: `{key, control, code}`
#[derive(Clone, Debug)]
pub struct RtRecord {
    pub key: u64,
    pub control: Vec<u8>,
    pub code: Vec<u8>,
}

/// `make_job(records)`: deterministic code, translator input, IDs and a named resource map. Identical repeated keys collapse, conflicting copies fail; the inputs are sorted by u64 key and
/// concatenated at 256-byte boundaries; IDs start at one and stay positive signed16 values; compatibility needs equal opaque fingerprints and equal named resource layouts.
pub fn make_job(records: impl IntoIterator<Item = RtRecord>) -> R<BuiltJob> {
    let mut unique: BTreeMap<u64, (Vec<u8>, Vec<u8>)> = BTreeMap::new();
    for rec in records {
        let payload = (rec.control, rec.code);
        require(
            !unique.contains_key(&rec.key) || unique[&rec.key] == payload,
            &format!("conflicting RT records for key {:016x}", rec.key),
        )?;
        unique.insert(rec.key, payload);
    }
    require(!unique.is_empty(), "RT library has no modules")?;
    let mut image: Vec<u8> = Vec::new();
    let mut modules = Vec::new();
    let mut moves: Vec<PlannedMove> = Vec::new();
    let mut fingerprint: Option<[u8; 16]> = None;
    let mut layout: Vec<ResourceLayout> = Vec::new();
    let mut next_id: u32 = 1;
    let (mut raygen_frame, mut callback_frame): (i64, i64) = (-1, 0);
    for (key, (control, code)) in &unique {
        let module = module_of(control, code)?;
        require(
            next_id as usize + module.regions.len() <= 0x8000,
            "RT library exceeds positive signed16 logical IDs",
        )?;
        let fp = *fingerprint.get_or_insert(module.fingerprint);
        let ids: Vec<u32> = (next_id..next_id + module.regions.len() as u32).collect();
        let plan = make_link_plan(&module, &ids, Some(&fp))?;
        let current = layout_of(&module)?;
        if layout.is_empty() {
            layout = current.clone();
        }
        require(
            layout == current,
            "RT modules disagree on named resource layout",
        )?;
        let address = (image.len() as u64 + CODE_ALIGNMENT - 1) & !(CODE_ALIGNMENT - 1);
        require(
            address <= u64::MAX - code.len() as u64,
            "RT linked code extent overflows u64",
        )?;
        image.resize(address as usize, 0);
        image.extend_from_slice(code);
        let mut owner_ids: BTreeMap<usize, u32> = BTreeMap::new();
        let mut entries = Vec::new();
        for linked in &plan.regions {
            let entry = &module.entries[linked.entry_index];
            let owner = *owner_ids
                .entry(linked.entry_index)
                .or_insert(linked.logical_id);
            entries.push(PlannedEntry {
                address: address + linked.region_offset,
                id: linked.logical_id,
                owner,
                kind: entry.kind(),
                frame: entry.continuation_frame_bytes(),
                entry_index: linked.entry_index,
                region_index: linked.region_index,
            });
        }
        for entry in &module.entries {
            if entry.kind() == 0 {
                raygen_frame = raygen_frame.max(entry.continuation_frame_bytes() as i64);
            } else {
                callback_frame = callback_frame.max(entry.continuation_frame_bytes() as i64);
            }
        }
        modules.push(PlannedModule {
            key: *key,
            address,
            length: code.len() as u64,
            control_sha256: sha(control),
            metadata_sha256: sha(&module.metadata),
            code_sha256: sha(code),
            entries,
        });
        moves.extend(plan.relocations.iter().map(|m| PlannedMove {
            address: address + m.instruction_offset,
            template: m.template,
            id: m.target_logical_id,
        }));
        next_id += module.regions.len() as u32;
    }
    require(raygen_frame >= 0, "RT library has no ray-generation entry")?;
    let arena = ((32i64.max(raygen_frame + callback_frame)) + 31) & !31;
    require(arena <= 0x10000, "RT continuation arena exceeds 64 KiB")?;
    moves.sort_by_key(|m| m.address);
    Ok(BuiltJob {
        code: image,
        modules,
        moves,
        layout,
        fingerprint: fingerprint.unwrap(),
        local_memory_bytes: arena as u32,
    })
}

/// `module_entries[key][i].id` and friends, as the manifest prints them
pub fn module_entries_json(job: &BuiltJob) -> Value {
    let mut o = serde_json::Map::new();
    for m in &job.modules {
        o.insert(
            format!("{:016x}", m.key),
            Value::Array(m.entries.iter().map(|e| e.full_record()).collect()),
        );
    }
    Value::Object(o)
}
