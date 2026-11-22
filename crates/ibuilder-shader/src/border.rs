//! dmsh_rewrite.border_emu: samples of float textures through the argument buffer's samplers get a twin sampler and a correction (`dm_bs`).
use crate::templates as t;
use crate::text::*;
use crate::{PostError, failed};
use std::borrow::Cow;
use std::collections::HashSet;

/// one argument-buffer member line: `lead` the indentation, `attrs` the `, name(n)` attributes after the id, `end` the byte just after the `]];`
pub struct Member<'a> {
    pub ty: &'a str,
    pub name: &'a str,
    pub id: u32,
    pub lead: &'a str,
    pub attrs: &'a str,
    pub end: usize,
}

/// `struct spvDescriptorSetBuffer0\s*\{(.*?)\};` -> (start of the match, start of group 1, end of group 1)
pub fn descriptor_struct(src: &str) -> Option<(usize, usize, usize)> {
    let b = src.as_bytes();
    let needle = b"struct spvDescriptorSetBuffer0";
    let mut from = 0;
    while let Some(p) = find_b(b, needle, from) {
        from = p + 1;
        let q = skip_space(b, p + needle.len());
        if q < b.len() && b[q] == b'{' {
            let g0 = q + 1;
            if let Some(e) = find_b(b, b"};", g0) {
                return Some((p, g0, e));
            }
            return None;
        }
    }
    None
}

/// one line against `^\s*(.+?)\s+(\w+)\s*\[\[id\((\d+)\)(?:\s*,\s*\w+\(\d+\))*\]\];`
pub fn parse_member(line: &str) -> Option<Member<'_>> {
    let b = line.as_bytes();
    let st = skip_space(b, 0);
    let q = find_b(b, b"[[id(", st)?;
    let mut k = q;
    while k > st && is_space(b[k - 1]) {
        k -= 1;
    }
    let name_end = k;
    while k > st && is_word(b[k - 1]) {
        k -= 1;
    }
    let name_start = k;
    if name_start == name_end {
        return None;
    }
    let mut ws = name_start;
    while ws > st && is_space(b[ws - 1]) {
        ws -= 1;
    }
    if ws == name_start || ws == st {
        return None;
    } // `\s+` needs whitespace, `.+?` at least one character
    let d0 = q + 5;
    let d1 = digits_end(b, d0);
    if d1 == d0 || d1 >= b.len() || b[d1] != b')' {
        return None;
    }
    let mut i = d1 + 1;
    let attrs0 = i;
    // (?:\s*,\s*\w+\(\d+\))*
    loop {
        let mut j = skip_space(b, i);
        if j < b.len() && b[j] == b',' {
            j = skip_space(b, j + 1);
            let we = word_end(b, j);
            if we > j && we < b.len() && b[we] == b'(' {
                let de = digits_end(b, we + 1);
                if de > we + 1 && de < b.len() && b[de] == b')' {
                    i = de + 1;
                    continue;
                }
            }
        }
        break;
    }
    if !starts(b, i, "]];") {
        return None;
    }
    let id = line[d0..d1].parse().ok()?;
    Some(Member {
        ty: &line[st..ws],
        name: &line[name_start..name_end],
        id,
        lead: &line[..st],
        attrs: &line[attrs0..i],
        end: i + 3,
    })
}

pub fn members(body: &str) -> Vec<Member<'_>> {
    lines(body)
        .into_iter()
        .filter_map(|(a, e)| parse_member(&body[a..e]))
        .collect()
}

/// `(texture|depth)\w*<float(,.*)?>` fullmatch
fn is_float_tex(ty: &str) -> bool {
    let b = ty.as_bytes();
    let rest = if starts(b, 0, "texture") {
        7
    } else if starts(b, 0, "depth") {
        5
    } else {
        return false;
    };
    let w = word_end(b, rest);
    if !starts(b, w, "<float") {
        return false;
    }
    let tail = &ty[w + 6..];
    tail == ">" || (tail.len() >= 2 && tail.starts_with(',') && tail.ends_with('>'))
}

fn width(ty: &str) -> u32 {
    // array<.*,\s*(\d+)>
    if let Some(r) = ty.strip_prefix("array<")
        && let Some(r) = r.strip_suffix('>')
        && let Some(c) = r.rfind(',')
    {
        let n = r[c + 1..].trim_start_matches(|ch: char| ch.is_ascii() && is_space(ch as u8));
        if !n.is_empty() && n.bytes().all(|x| x.is_ascii_digit()) {
            return n.parse().unwrap_or(1);
        }
    }
    1
}

struct Call<'a> {
    start: usize,
    end: usize,
    tex: &'a str,
    smp: &'a str,
}

/// `spvDescriptorSet0\.(\w+)\.sample\(spvDescriptorSet0\.(\w+), `
fn calls(s: &str) -> Vec<Call<'_>> {
    let b = s.as_bytes();
    let mut v = Vec::new();
    let mid = b".sample(spvDescriptorSet0.";
    let pre = b"spvDescriptorSet0.";
    let mut from = 0;
    while let Some(p) = find_b(b, mid, from) {
        from = p + 1;
        let mut k = p;
        while k > 0 && is_word(b[k - 1]) {
            k -= 1;
        }
        if k == p || k < pre.len() || &b[k - pre.len()..k] != pre {
            continue;
        }
        let s0 = p + mid.len();
        let e = word_end(b, s0);
        if e == s0 || !starts(b, e, ", ") {
            continue;
        }
        v.push(Call {
            start: k - pre.len(),
            end: e + 2,
            tex: &s[k..p],
            smp: &s[s0..e],
        });
        from = e + 2;
    }
    v
}

pub fn border_emu(src: &str) -> Result<Cow<'_, str>, PostError> {
    let Some((_, g0, g1)) = descriptor_struct(src) else {
        return Ok(Cow::Borrowed(src));
    };
    let body = &src[g0..g1];
    let mem = members(body);
    let samplers: Vec<&str> = mem
        .iter()
        .filter(|m| m.ty == "sampler")
        .map(|m| m.name)
        .collect();
    let ftex: HashSet<&str> = mem
        .iter()
        .filter(|m| is_float_tex(m.ty))
        .map(|m| m.name)
        .collect();
    let cs = calls(src);
    let used: HashSet<&str> = cs
        .iter()
        .filter(|c| ftex.contains(c.tex))
        .map(|c| c.smp)
        .collect();
    let mut bit: Vec<(&str, usize)> = Vec::new();
    for (i, n) in samplers
        .iter()
        .filter(|n| used.contains(**n))
        .take(t::BORDER_SAMPLERS)
        .enumerate()
    {
        if let Some(e) = bit.iter_mut().find(|(x, _)| x == n) {
            e.1 = i;
        } else {
            bit.push((n, i));
        }
    }
    if bit.is_empty() {
        return Ok(Cow::Borrowed(src));
    }
    let nid = mem.iter().map(|m| m.id + width(m.ty)).max().unwrap();
    let mut add = String::new();
    for (n, i) in &bit {
        add.push_str(&format!(
            "    sampler {n}_w [[id({})]];\n",
            nid as usize + i
        ));
    }
    add.push_str(&format!(
        "    constant dm_border_t* dm_border [[id({})]];\n",
        nid as usize + bit.len()
    ));
    let mut text = String::with_capacity(src.len() + add.len() + 2048);
    text.push_str(&src[..g1]);
    if src.as_bytes()[g1 - 1] != b'\n' {
        text.push('\n');
    }
    text.push_str(&add);
    text.push_str(&src[g1..]);
    let bitof = |n: &str| bit.iter().find(|(x, _)| *x == n).map(|(_, i)| *i);
    // the calls again in the new text (same texts, shifted), last first
    let new_calls: Vec<(usize, usize, String, String)> = calls(&text)
        .into_iter()
        .filter(|c| ftex.contains(c.tex) && bitof(c.smp).is_some())
        .map(|c| (c.start, c.end, c.tex.to_string(), c.smp.to_string()))
        .collect();
    for (start, mend, tex, smp) in new_calls.into_iter().rev() {
        let b = text.as_bytes();
        let mut depth = 1i32;
        let mut i = mend;
        while depth != 0 {
            if i >= b.len() {
                return Err(failed(
                    "border_emu",
                    "unbalanced parentheses in a sample call",
                ));
            }
            match b[i] {
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            i += 1;
        }
        let args = text[mend..i - 1].to_string();
        let whole = text[start..i].to_string();
        let rep = format!(
            "dm_bs({whole}, spvDescriptorSet0.dm_border, {}u, spvDescriptorSet0.{tex}, spvDescriptorSet0.{smp}_w, {args})",
            bitof(&smp).unwrap()
        );
        text.replace_range(start..i, &rep);
    }
    // msl.replace('using namespace metal;\n', 'using namespace metal;\n' + BORDER_LIVE_FC + BORDER_HELPERS, 1)
    if let Some(p) = find(&text, "using namespace metal;\n", 0) {
        let at = p + "using namespace metal;\n".len();
        text.insert_str(at, &format!("{}{}", t::BORDER_LIVE_FC, t::BORDER_HELPERS));
    }
    Ok(Cow::Owned(text))
}
