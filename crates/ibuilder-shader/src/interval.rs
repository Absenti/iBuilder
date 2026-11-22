//! c1_guard.py's interval arithmetic over the integer temporaries of one shader's MSL (tokenizer + recursive descent), exactly as the Python evaluates them.
//! Python integers are unbounded: here they are i128 and an overflow gives UNBOUNDED (the Python's value would be astronomically large, which no caller accepts as a bound either).
use crate::text::*;
use std::collections::HashMap;

pub type Val = Option<i128>;
pub type Iv = (Val, Val);
pub const UNBOUNDED: Iv = (None, None);

fn known1(a: Val) -> bool {
    a.is_some()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Bad;

type R<T> = Result<T, Bad>;

/// `TOKEN` of c1_guard.py (leading `\s*` included): -> (token text range, end) at `i`, None when nothing matches
fn token(s: &[u8], i: usize) -> Option<(usize, usize)> {
    let st = skip_space(s, i);
    if st >= s.len() {
        return None;
    }
    let c = s[st];
    // 1. hex `0[xX][0-9a-fA-F]+[uU]?`
    if c == b'0'
        && st + 2 < s.len()
        && (s[st + 1] == b'x' || s[st + 1] == b'X')
        && s[st + 2].is_ascii_hexdigit()
    {
        let mut e = st + 2;
        while e < s.len() && s[e].is_ascii_hexdigit() {
            e += 1;
        }
        if e < s.len() && (s[e] == b'u' || s[e] == b'U') {
            e += 1;
        }
        return Some((st, e));
    }
    if c.is_ascii_digit() {
        let d1 = digits_end(s, st);
        // 2. float `\d+\.\d*(?:[eE][-+]?\d+)?f?`
        if d1 < s.len() && s[d1] == b'.' {
            let mut e = digits_end(s, d1 + 1);
            if e < s.len() && (s[e] == b'e' || s[e] == b'E') {
                let mut k = e + 1;
                if k < s.len() && (s[k] == b'+' || s[k] == b'-') {
                    k += 1;
                }
                let k2 = digits_end(s, k);
                if k2 > k {
                    e = k2;
                }
            }
            if e < s.len() && s[e] == b'f' {
                e += 1;
            }
            return Some((st, e));
        }
        // 3. `\d+[uU]?`
        let mut e = d1;
        if e < s.len() && (s[e] == b'u' || s[e] == b'U') {
            e += 1;
        }
        return Some((st, e));
    }
    // 4. identifier with member chain `[A-Za-z_]\w*(?:\.\w+)*`
    if c.is_ascii_alphabetic() || c == b'_' {
        let mut e = word_end(s, st);
        loop {
            if e < s.len() && s[e] == b'.' {
                let w = word_end(s, e + 1);
                if w > e + 1 {
                    e = w;
                    continue;
                }
            }
            break;
        }
        return Some((st, e));
    }
    // 5. `\.\w+`
    if c == b'.' {
        let w = word_end(s, st + 1);
        if w > st + 1 {
            return Some((st, w));
        }
        return None;
    }
    // 6. two-character operators
    if st + 1 < s.len() {
        let two = &s[st..st + 2];
        if matches!(
            two,
            b"<<" | b">>" | b"<=" | b">=" | b"==" | b"!=" | b"&&" | b"||"
        ) {
            return Some((st, st + 2));
        }
    }
    // 7. single characters `[-+*/%&|^~!?:(),<>\[\]]`
    if matches!(
        c,
        b'-' | b'+'
            | b'*'
            | b'/'
            | b'%'
            | b'&'
            | b'|'
            | b'^'
            | b'~'
            | b'!'
            | b'?'
            | b':'
            | b'('
            | b')'
            | b','
            | b'<'
            | b'>'
            | b'['
            | b']'
    ) {
        return Some((st, st + 1));
    }
    None
}

/// c1_guard.tokens: the as_type casts become `as_type_u(` / `as_type(`, then the tokens; Err on text that does not tokenize
fn tokens(text: &str) -> R<Vec<String>> {
    let b = text.as_bytes();
    // the cast rewrite (`\bas_type<u(?:int|short|long)>\s*\(` -> `as_type_u(`, then `\bas_type<\w+>\s*\(` -> `as_type(`)
    let mut norm = String::new();
    let t: &str = if contains(text, "as_type<") {
        let mut at = 0;
        let mut from = 0;
        while let Some(p) = find_b(b, b"as_type<", from) {
            from = p + 1;
            if in_word(b, p) {
                continue;
            }
            let w0 = p + 8;
            let w1 = word_end(b, w0);
            if w1 == w0 || w1 >= b.len() || b[w1] != b'>' {
                continue;
            }
            let q = skip_space(b, w1 + 1);
            if q >= b.len() || b[q] != b'(' {
                continue;
            }
            let name = &text[w0..w1];
            let rep = if matches!(name, "uint" | "ushort" | "ulong") {
                "as_type_u("
            } else {
                "as_type("
            };
            norm.push_str(&text[at..p]);
            norm.push_str(rep);
            at = q + 1;
            from = q + 1;
        }
        norm.push_str(&text[at..]);
        &norm
    } else {
        text
    };
    let s = t.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        match token(s, i) {
            Some((a, e)) => {
                out.push(t[a..e].to_string());
                i = e;
            }
            None => {
                if t[i..]
                    .trim_matches(|c: char| c.is_ascii() && is_space(c as u8))
                    .is_empty()
                {
                    break;
                }
                return Err(Bad);
            }
        }
    }
    Ok(out)
}

fn binary_level(op: &str) -> Option<u8> {
    Some(match op {
        "||" => 1,
        "&&" => 2,
        "|" => 3,
        "^" => 4,
        "&" => 5,
        "==" | "!=" => 6,
        "<" | ">" | "<=" | ">=" => 7,
        "<<" | ">>" => 8,
        "+" | "-" => 9,
        "*" | "/" | "%" => 10,
        _ => return None,
    })
}

fn add(a: Val, b: Val) -> Val {
    match (a, b) {
        (Some(x), Some(y)) => x.checked_add(y),
        _ => None,
    }
}
fn sub(a: Val, b: Val) -> Val {
    match (a, b) {
        (Some(x), Some(y)) => x.checked_sub(y),
        _ => None,
    }
}
fn neg(a: Val) -> Val {
    a.and_then(|x| x.checked_neg())
}

pub struct Interval<'d> {
    defs: &'d HashMap<String, (String, String)>,
    memo: HashMap<String, Iv>,
    depth: u32,
    nest: u32,
    toks: Vec<String>,
    pos: usize,
}

fn parse_int(t: &str) -> R<i128> {
    // int(t.rstrip("uU"), 0)
    let u = t.trim_end_matches(['u', 'U']);
    if let Some(h) = u.strip_prefix("0x").or_else(|| u.strip_prefix("0X")) {
        return i128::from_str_radix(h, 16).map_err(|_| Bad);
    }
    if u.len() > 1 && u.starts_with('0') && u.bytes().any(|c| c != b'0') {
        return Err(Bad);
    } // Python: invalid literal for base 0
    u.parse::<i128>().map_err(|_| Bad)
}

impl<'d> Interval<'d> {
    pub fn new(defs: &'d HashMap<String, (String, String)>) -> Self {
        Interval {
            defs,
            memo: HashMap::new(),
            depth: 0,
            nest: 0,
            toks: Vec::new(),
            pos: 0,
        }
    }

    fn variable(&mut self, name: &str) -> Iv {
        if let Some(v) = self.memo.get(name) {
            return *v;
        }
        if !self.defs.contains_key(name) || self.depth > 32 {
            return UNBOUNDED;
        }
        self.memo.insert(name.to_string(), UNBOUNDED);
        self.depth += 1;
        let (kind, text) = self.defs[name].clone();
        let mut value = self.expression(&text);
        self.depth -= 1;
        if kind == "bool" {
            value = (Some(0), Some(1));
        } else if kind.starts_with('u') && !(known1(value.0) && value.0.unwrap() >= 0) {
            value = (Some(0), None);
        }
        self.memo.insert(name.to_string(), value);
        value
    }

    pub fn expression(&mut self, text: &str) -> Iv {
        let saved_nest = self.nest;
        let r = (|| -> R<Iv> {
            self.toks = tokens(text)?;
            self.pos = 0;
            self.nest = 0;
            let v = self.ternary()?;
            if self.pos != self.toks.len() {
                return Ok(UNBOUNDED);
            }
            Ok(v)
        })();
        self.nest = saved_nest;
        r.unwrap_or(UNBOUNDED)
    }

    fn peek(&self) -> Option<&str> {
        self.toks.get(self.pos).map(|s| s.as_str())
    }
    fn take(&mut self, want: Option<&str>) -> R<String> {
        let t = self.toks.get(self.pos).ok_or(Bad)?.clone();
        if let Some(w) = want
            && t != w
        {
            return Err(Bad);
        }
        self.pos += 1;
        Ok(t)
    }

    fn ternary(&mut self) -> R<Iv> {
        self.nest += 1;
        if self.nest > 150 {
            return Err(Bad);
        } // Python's RecursionError, which expression() turns into UNBOUNDED
        let r = (|| {
            let cond = self.binary(1)?;
            if self.peek() != Some("?") {
                return Ok(cond);
            }
            self.take(Some("?"))?;
            let a = self.ternary()?;
            self.take(Some(":"))?;
            let b = self.ternary()?;
            let lo = if known1(a.0) && known1(b.0) {
                Some(a.0.unwrap().min(b.0.unwrap()))
            } else {
                None
            };
            let hi = if known1(a.1) && known1(b.1) {
                Some(a.1.unwrap().max(b.1.unwrap()))
            } else {
                None
            };
            Ok((lo, hi))
        })();
        self.nest -= 1;
        r
    }

    fn binary(&mut self, level: u8) -> R<Iv> {
        let mut left = self.unary()?;
        while let Some(op) = self.peek() {
            let Some(l) = binary_level(op) else { break };
            if l < level {
                break;
            }
            let op = self.take(None)?;
            let right = self.binary(l + 1)?;
            left = combine(&op, left, right);
        }
        Ok(left)
    }

    fn unary(&mut self) -> R<Iv> {
        match self.peek() {
            Some("-") => {
                self.take(None)?;
                let (l, h) = self.unary()?;
                Ok((neg(h), neg(l)))
            }
            Some("+") => {
                self.take(None)?;
                self.unary()
            }
            Some("!") => {
                self.take(None)?;
                self.unary()?;
                Ok((Some(0), Some(1)))
            }
            Some("~") | Some("*") => {
                self.take(None)?;
                self.unary()?;
                Ok(UNBOUNDED)
            }
            _ => {
                let p = self.primary()?;
                self.postfix(p)
            }
        }
    }

    fn postfix(&mut self, value: Iv) -> R<Iv> {
        let mut value = value;
        while let Some(t) = self.peek() {
            if !(t == "[" || t.starts_with('.')) {
                break;
            }
            if self.take(None)? == "[" {
                self.ternary()?;
                self.take(Some("]"))?;
            }
            value = UNBOUNDED;
        }
        Ok(value)
    }

    fn primary(&mut self) -> R<Iv> {
        let t = self.take(None)?;
        if t == "(" {
            let v = self.ternary()?;
            self.take(Some(")"))?;
            return Ok(v);
        }
        let tb = t.as_bytes();
        // `0[xX][0-9a-fA-F]+[uU]?|\d+[uU]?` fullmatch
        let is_int = {
            let core = if t.ends_with(['u', 'U']) {
                &t[..t.len() - 1]
            } else {
                &t[..]
            };
            !core.is_empty()
                && (core.bytes().all(|c| c.is_ascii_digit())
                    || (core.len() > 2
                        && (core.starts_with("0x") || core.starts_with("0X"))
                        && core[2..].bytes().all(|c| c.is_ascii_hexdigit())))
        };
        if is_int {
            let v = parse_int(&t)?;
            return Ok((Some(v), Some(v)));
        }
        // float `\d+\.\d*(?:[eE][-+]?\d+)?f?` fullmatch
        if tb[0].is_ascii_digit() {
            return Ok(UNBOUNDED);
        }
        // identifier `[A-Za-z_]\w*(?:\.\w+)*` fullmatch
        if !(tb[0].is_ascii_alphabetic() || tb[0] == b'_') {
            return Err(Bad);
        }
        if self.peek() == Some("(") {
            self.take(None)?;
            let mut args: Vec<Iv> = Vec::new();
            if self.peek() != Some(")") {
                args.push(self.ternary()?);
                while self.peek() == Some(",") {
                    self.take(None)?;
                    args.push(self.ternary()?);
                }
            }
            self.take(Some(")"))?;
            return Ok(call(&t, &args));
        }
        // a temporary `_\d+`
        if tb[0] == b'_' && tb.len() > 1 && tb[1..].iter().all(|c| c.is_ascii_digit()) {
            let saved = (std::mem::take(&mut self.toks), self.pos);
            let v = self.variable(&t);
            self.toks = saved.0;
            self.pos = saved.1;
            return Ok(v);
        }
        if t == "true" || t == "false" {
            return Ok((Some(0), Some(1)));
        }
        Ok(UNBOUNDED)
    }
}

fn combine(op: &str, a: Iv, b: Iv) -> Iv {
    let ((al, ah), (bl, bh)) = (a, b);
    let all4 = al.is_some() && ah.is_some() && bl.is_some() && bh.is_some();
    match op {
        "||" | "&&" | "==" | "!=" | "<" | ">" | "<=" | ">=" => (Some(0), Some(1)),
        "+" => (add(al, bl), add(ah, bh)),
        "-" => (sub(al, bh), sub(ah, bl)),
        "*" => {
            if !all4 {
                return UNBOUNDED;
            }
            let (al, ah, bl, bh) = (al.unwrap(), ah.unwrap(), bl.unwrap(), bh.unwrap());
            let mut p = [0i128; 4];
            for (k, (x, y)) in [(al, bl), (al, bh), (ah, bl), (ah, bh)]
                .into_iter()
                .enumerate()
            {
                match x.checked_mul(y) {
                    Some(v) => p[k] = v,
                    None => return UNBOUNDED,
                }
            }
            (
                Some(*p.iter().min().unwrap()),
                Some(*p.iter().max().unwrap()),
            )
        }
        "/" | ">>" | "<<" => {
            if !all4 {
                return UNBOUNDED;
            }
            let (al, ah, bl, bh) = (al.unwrap(), ah.unwrap(), bl.unwrap(), bh.unwrap());
            if bl != bh || bl < 0 || (op == "/" && bl == 0) {
                return UNBOUNDED;
            }
            match op {
                "<<" => {
                    if bl >= 126 {
                        return UNBOUNDED;
                    }
                    let (x, y) = (al.checked_mul(1i128 << bl), ah.checked_mul(1i128 << bl));
                    (x, y)
                }
                ">>" => {
                    let sh = |v: i128| {
                        if bl >= 127 {
                            if v < 0 { -1 } else { 0 }
                        } else {
                            v >> bl
                        }
                    };
                    (Some(sh(al)), Some(sh(ah)))
                }
                _ => {
                    if al >= 0 {
                        (Some(al.div_euclid(bl)), Some(ah.div_euclid(bl)))
                    } else {
                        UNBOUNDED
                    }
                }
            }
        }
        "%" => {
            if bl.is_none() || bh.is_none() || bl != bh || bl.unwrap() <= 0 {
                return UNBOUNDED;
            }
            let m = bl.unwrap();
            if al.is_some() && al.unwrap() >= 0 {
                (Some(0), Some(m - 1))
            } else {
                (Some(-(m - 1)), Some(m - 1))
            }
        }
        "&" => {
            let mut caps: Vec<i128> = Vec::new();
            for (l, h) in [a, b] {
                if let (Some(l), Some(h)) = (l, h)
                    && l >= 0
                {
                    caps.push(h);
                }
            }
            match caps.iter().min() {
                Some(m) => (Some(0), Some(*m)),
                None => UNBOUNDED,
            }
        }
        "|" | "^" => {
            if !all4 {
                return UNBOUNDED;
            }
            let (al, ah, bl, bh) = (al.unwrap(), ah.unwrap(), bl.unwrap(), bh.unwrap());
            if al < 0 || bl < 0 {
                return UNBOUNDED;
            }
            let m = ah.max(bh);
            let bits = 128 - (m as u128).leading_zeros();
            if bits >= 127 {
                return UNBOUNDED;
            }
            (Some(0), Some((1i128 << bits) - 1))
        }
        _ => UNBOUNDED,
    }
}

fn call(name: &str, args: &[Iv]) -> Iv {
    if name == "as_type_u" && args.len() == 1 {
        return (Some(0), None);
    }
    if matches!(name, "int" | "uint" | "short" | "ushort" | "long" | "ulong") && args.len() == 1 {
        let (l, h) = args[0];
        if name.starts_with('u') && !(l.is_some() && l.unwrap() >= 0) {
            return (Some(0), None);
        }
        return (l, h);
    }
    if name == "min" && args.len() == 2 {
        let ((al, ah), (bl, bh)) = (args[0], args[1]);
        let hs: Vec<i128> = [ah, bh].into_iter().flatten().collect();
        let lo = match (al, bl) {
            (Some(a), Some(b)) => Some(a.min(b)),
            _ => None,
        };
        return (lo, hs.into_iter().min());
    }
    if name == "max" && args.len() == 2 {
        let ((al, ah), (bl, bh)) = (args[0], args[1]);
        let ls: Vec<i128> = [al, bl].into_iter().flatten().collect();
        let hi = match (ah, bh) {
            (Some(a), Some(b)) => Some(a.max(b)),
            _ => None,
        };
        return (ls.into_iter().max(), hi);
    }
    if name == "clamp" && args.len() == 3 {
        return (args[1].0, args[2].1);
    }
    UNBOUNDED
}

pub fn known(low: Val, high: Val) -> bool {
    low.is_some() && high.is_some()
}
