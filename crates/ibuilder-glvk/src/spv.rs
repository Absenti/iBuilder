//! SPIR-V words: `struct.unpack('<%dI')` and back, an instruction's string operand (`OpName`'s literal).

/// `struct.unpack(f'<{len(data) // 4}I', data)`: the caller checks `len % 4` first (the Python's `struct.error`)
pub fn words(data: &[u8]) -> Vec<u32> {
    data.as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect()
}

/// `struct.pack(f'<{len(w)}I', *w)`
pub fn bytes(w: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(w.len() * 4);
    for x in w {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

/// `struct.pack(f'<{n}I', *w).split(b'\0')[0]`: the literal string a run of words holds, up to its first NUL
pub fn literal(w: &[u32]) -> Vec<u8> {
    let b = bytes(w);
    match b.iter().position(|&c| c == 0) {
        Some(z) => b[..z].to_vec(),
        None => b,
    }
}

/// The words of a NUL-terminated string padded to whole words, as SPIR-V stores a literal
pub fn string_words(s: &[u8]) -> Vec<u32> {
    let mut b = s.to_vec();
    b.push(0);
    while !b.len().is_multiple_of(4) {
        b.push(0);
    }
    words(&b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_round_trip() {
        let w = string_words(b"main");
        assert_eq!(w.len(), 2);
        assert_eq!(literal(&w), b"main");
        assert_eq!(literal(&string_words(b"abc")), b"abc");
    }
}
