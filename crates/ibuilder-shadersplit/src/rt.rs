//! `dmsh_rt.parse_module`: the validation of an NVN2 ray-tracing module's metadata (the RTCore serializer's records), as `shader_split.check(.., 'rt', ..)` and `bnsh_shaders` call it.
//!
//! Only what the split needs: whether the module parses, and the text of the `InvalidModule` it raises (it goes into the `check` reason `invalid RT metadata: <text>`, and from there into a
//! `[RED]` line). Every field the Python keeps without looking at it (fingerprint, flags, selectors, resources' contents) is read for its length and dropped. The reading order, the
//! alignment (of the absolute position in the metadata, as `_Reader.align` does) and every `_require` message are the Python's.

type R<T> = Result<T, String>;

fn require(c: bool, m: &str) -> R<()> {
    if c { Ok(()) } else { Err(m.to_string()) }
}

/// `_span(data, offset, size, label)`
fn span<'a>(d: &'a [u8], offset: u64, size: u64, label: &str) -> R<&'a [u8]> {
    let n = d.len() as u64;
    require(
        offset <= n && size <= n - offset,
        &format!("{label} outside input"),
    )?;
    Ok(&d[offset as usize..(offset + size) as usize])
}

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

/// `_serialized_region`: the region's name
fn serialized_region(r: &mut Rd<'_>) -> R<String> {
    let name = r.name()?;
    r.align(4)?;
    r.take(17 * 4)?; // fields
    r.align(8)?;
    for _ in 0..8 {
        r.u64()?;
    }
    for _ in 0..r.count(72)? {
        // named resource-access records
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
    r.vector(8)?; // opaque key/value pairs
    r.take(2)?;
    Ok(name)
}

/// `_entry`: the names of its regions
fn entry(r: &mut Rd<'_>) -> R<Vec<String>> {
    r.name()?;
    r.align(4)?;
    let fields = r.take(9 * 4)?;
    let nregions = u32::from_le_bytes(fields[..4].try_into().unwrap()) as u64;
    r.take(9)?; // flags
    r.align(4)?;
    r.u32()?;
    let mut regions = Vec::new();
    for _ in 0..r.count(80)? {
        regions.push(serialized_region(r)?);
    }
    require(
        nregions == regions.len() as u64,
        "entry region count disagrees with inner records",
    )?;
    r.align(4)?;
    r.u32()?;
    r.vector(12)?;
    r.align(4)?;
    r.take(6)?;
    r.align(4)?;
    r.u32()?;
    Ok(regions)
}

fn resource(r: &mut Rd<'_>) -> R<()> {
    r.name()?;
    r.align(4)?;
    r.u32()?;
    r.align(8)?;
    let (size, alignment, offset) = (r.u64()?, r.u64()?, r.u64()?);
    require(
        alignment > 0 && alignment & (alignment - 1) == 0,
        "resource alignment is not a power of two",
    )?;
    require(offset % alignment == 0, "misaligned resource offset")?;
    require(offset <= u64::MAX - size, "resource extent overflows u64")?;
    r.vector(1)?;
    r.align(4)?;
    r.u32()?;
    r.align(8)?;
    r.u64()?;
    r.u32()?;
    Ok(())
}

/// `_inner`: the entries' region names
fn inner(metadata: &[u8], start: u64, size: u64) -> R<Vec<Vec<String>>> {
    let mut r = Rd::new(metadata, start, size)?;
    r.take(16)?; // fingerprint
    r.vector(1)?;
    r.align(8)?;
    r.take(0xc5)?; // fixed compiler/options record
    r.vector(1)?; // opaque module+0x108 string
    r.take(3)?;
    r.align(4)?;
    r.u32()?;
    let c = r.count(1)?;
    require(c == 0, "unsupported nonempty module parameter list")?;
    r.align(4)?;
    r.u32()?;
    r.name()?; // module+0x148 name
    r.align(8)?;
    r.take(0x24)?;
    r.align(4)?;
    r.u32()?;
    for _ in 0..r.count(48)? {
        // runtime 0x90 records
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
        // named opaque u32 / u8 / u32 records
        r.name()?;
        r.align(4)?;
        r.take(12)?;
    }
    r.u8()?;
    for _ in 0..2 {
        // two string -> u64 maps
        for _ in 0..r.count(16)? {
            r.name()?;
            r.align(8)?;
            r.u64()?;
        }
    }
    for _ in 0..r.count(16)? {
        // string -> string-set map
        r.name()?;
        for _ in 0..r.count(8)? {
            r.name()?;
        }
    }
    r.align(4)?;
    r.u32()?;
    r.u32()?;
    for _ in 0..r.count(56)? {
        resource(&mut r)?;
    }
    r.u8()?;
    r.align(4)?;
    r.u32()?;
    r.vector(1)?;
    require(r.pos == r.end, "unconsumed inner metadata variant")?;
    Ok(entries)
}

/// `parse_module(metadata, code)`: `Ok(())` when the module validates, else the text of the `InvalidModule`
pub fn parse_module(metadata: &[u8], code: &[u8]) -> Result<(), String> {
    require(
        !code.is_empty() && code.len().is_multiple_of(16),
        "code is empty or not 16-byte aligned",
    )?;
    let hb = span(metadata, 0, 56, "outer header")?;
    let h: Vec<u32> = hb
        .chunks(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect();
    require(
        h[3] >= 56 && h[3].is_multiple_of(8),
        "misaligned inner metadata",
    )?;
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
    let _selectors = words(h[8], h[4], "region selectors")?;
    let entry_counts = words(h[12], h[11], "entry region counts")?;
    require(
        entry_counts.iter().all(|&c| c != 0)
            && entry_counts.iter().map(|&c| c as u64).sum::<u64>() == h[4] as u64,
        "entry regions do not cover region table",
    )?;
    let mut regions: Vec<(u32, u32, String)> = Vec::new();
    for i in 0..h[4] as usize {
        let (offset, size, name_offset) = (starts[i], sizes[i], names[i]);
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
        regions.push((offset, size, name.to_string()));
    }
    let entries = inner(metadata, h[3] as u64, h[2] as u64)?;
    require(
        entries.len() as u64 == h[11] as u64,
        "inner/outer entry count mismatch",
    )?;
    require(
        entries
            .iter()
            .map(|e| e.len() as u64)
            .eq(entry_counts.iter().map(|&c| c as u64)),
        "inner/outer region count mismatch",
    )?;
    require(
        entries.iter().flatten().eq(regions.iter().map(|r| &r.2)),
        "inner/outer region names disagree",
    )?;
    let raw = span(metadata, h[10] as u64, h[9] as u64 * 40, "relocation table")?;
    require(h[10].is_multiple_of(8), "misaligned relocation table")?;
    for i in 0..h[9] as usize {
        let b = &raw[40 * i..40 * i + 40];
        let patch_offset = u64::from_le_bytes(b[0..8].try_into().unwrap());
        let kind = i32::from_le_bytes(b[24..28].try_into().unwrap());
        let region_base = u32::from_le_bytes(b[28..32].try_into().unwrap());
        let target = u32::from_le_bytes(b[32..36].try_into().unwrap());
        let sources: Vec<&(u32, u32, String)> =
            regions.iter().filter(|r| r.0 == region_base).collect();
        require(!sources.is_empty(), "relocation base has no region")?;
        require(
            patch_offset % 16 == 0 && sources.iter().any(|r| patch_offset + 16 <= r.1 as u64),
            "relocation instruction outside region",
        )?;
        span(
            code,
            region_base as u64 + patch_offset,
            16,
            "relocation instruction",
        )?;
        if kind < 0 {
            require(
                (target as usize) < regions.len(),
                "logical relocation target outside module",
            )?;
        } else {
            require(
                (target as usize) < code.len(),
                "code relocation target outside module",
            )?;
        }
    }
    Ok(())
}
