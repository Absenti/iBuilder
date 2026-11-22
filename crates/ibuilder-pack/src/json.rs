//! A small, order-preserving JSON value with Python's `json.dumps` formatting.
//!
//! The package files are compared byte for byte with what Python's `json.dumps` writes, and `serde_json` neither
//! keeps insertion order (without an extra dependency) nor reproduces Python's spellings (`1e+16`, `\u00e9`,
//! `" : "`), so the few features needed are implemented here: parse (`json.loads`), and dump with
//! `indent`, `ensure_ascii`, `sort_keys`, `separators`.
use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i128),
    Float(f64),
    Str(String),
    Arr(Vec<Json>),
    /// Insertion-ordered (a Python dict). Duplicate keys do not occur in values built here.
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn str(s: impl Into<String>) -> Json {
        Json::Str(s.into())
    }
    pub fn obj() -> Json {
        Json::Obj(Vec::new())
    }
    /// Append / replace a key (a Python `d[k] = v`: an existing key keeps its position).
    pub fn set(&mut self, key: &str, v: Json) {
        if let Json::Obj(items) = self {
            if let Some(slot) = items.iter_mut().find(|(k, _)| k == key) {
                slot.1 = v;
            } else {
                items.push((key.to_string(), v));
            }
        }
    }
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(items) => items.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn keys(&self) -> Vec<String> {
        match self {
            Json::Obj(items) => items.iter().map(|(k, _)| k.clone()).collect(),
            _ => Vec::new(),
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        if let Json::Str(s) = self {
            Some(s)
        } else {
            None
        }
    }
    pub fn is_obj(&self) -> bool {
        matches!(self, Json::Obj(_))
    }

    /// Python's `type(v).__name__`-style tag used by compare_nxgame.py's `typ()`.
    pub fn type_name(&self) -> &'static str {
        match self {
            Json::Null => "null",
            Json::Bool(_) => "bool",
            Json::Int(_) => "int",
            Json::Float(_) => "float",
            Json::Str(_) => "str",
            Json::Arr(_) => "array",
            Json::Obj(_) => "object",
        }
    }

    /// Python's `==` on loaded JSON values (`True == 1`, `1 == 1.0`, dicts order-insensitive).
    pub fn py_eq(&self, other: &Json) -> bool {
        use Json::*;
        fn num(v: &Json) -> Option<f64> {
            match v {
                Bool(b) => Some(*b as u8 as f64),
                Int(i) => Some(*i as f64),
                Float(f) => Some(*f),
                _ => None,
            }
        }
        match (self, other) {
            (Null, Null) => true,
            (Str(a), Str(b)) => a == b,
            (Arr(a), Arr(b)) => a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.py_eq(y)),
            (Obj(a), Obj(b)) => {
                a.len() == b.len()
                    && a.iter()
                        .all(|(k, v)| b.iter().any(|(k2, v2)| k == k2 && v.py_eq(v2)))
            }
            (Int(a), Int(b)) => a == b,
            (Bool(a), Bool(b)) => a == b,
            (a, b) => match (num(a), num(b)) {
                (Some(x), Some(y)) => x == y,
                _ => false,
            },
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// dump
// ---------------------------------------------------------------------------------------------------------------

/// `json.dumps` options.
#[derive(Clone, Debug)]
pub struct Dump {
    pub indent: Option<usize>,
    pub ensure_ascii: bool,
    pub sort_keys: bool,
    pub item_sep: String,
    pub key_sep: String,
}

impl Dump {
    /// `json.dumps(v)`: ensure_ascii, separators `(', ', ': ')`.
    pub fn compact() -> Dump {
        Dump {
            indent: None,
            ensure_ascii: true,
            sort_keys: false,
            item_sep: ", ".into(),
            key_sep: ": ".into(),
        }
    }
    /// `json.dumps(v, indent=n, ensure_ascii=...)`: separators `(',', ': ')`.
    pub fn indented(n: usize, ensure_ascii: bool) -> Dump {
        Dump {
            indent: Some(n),
            ensure_ascii,
            sort_keys: false,
            item_sep: ",".into(),
            key_sep: ": ".into(),
        }
    }
    /// The Mac app's PackageWriter: `indent=2, sort_keys=True, ensure_ascii=False, separators=(',', ' : ')`.
    pub fn swift_pretty() -> Dump {
        Dump {
            indent: Some(2),
            ensure_ascii: false,
            sort_keys: true,
            item_sep: ",".into(),
            key_sep: " : ".into(),
        }
    }
}

pub fn dumps(v: &Json, o: &Dump) -> String {
    let mut out = String::new();
    write_value(&mut out, v, o, 0);
    out
}

fn write_value(out: &mut String, v: &Json, o: &Dump, level: usize) {
    match v {
        Json::Null => out.push_str("null"),
        Json::Bool(true) => out.push_str("true"),
        Json::Bool(false) => out.push_str("false"),
        Json::Int(i) => out.push_str(&i.to_string()),
        Json::Float(f) => out.push_str(&float_repr(*f)),
        Json::Str(s) => write_str(out, s, o.ensure_ascii),
        Json::Arr(a) => {
            if a.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(&o.item_sep);
                }
                newline(out, o, level + 1);
                write_value(out, x, o, level + 1);
            }
            newline(out, o, level);
            out.push(']');
        }
        Json::Obj(items) => {
            if items.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            let mut order: Vec<&(String, Json)> = items.iter().collect();
            if o.sort_keys {
                order.sort_by(|a, b| a.0.cmp(&b.0)); // UTF-8 byte order == code point order
            }
            for (i, (k, x)) in order.into_iter().enumerate() {
                if i > 0 {
                    out.push_str(&o.item_sep);
                }
                newline(out, o, level + 1);
                write_str(out, k, o.ensure_ascii);
                out.push_str(&o.key_sep);
                write_value(out, x, o, level + 1);
            }
            newline(out, o, level);
            out.push('}');
        }
    }
}

fn newline(out: &mut String, o: &Dump, level: usize) {
    if let Some(n) = o.indent {
        out.push('\n');
        for _ in 0..n * level {
            out.push(' ');
        }
    }
}

fn write_str(out: &mut String, s: &str, ensure_ascii: bool) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if ensure_ascii && (c as u32) > 0x7e => {
                // Python escapes everything above ASCII, DEL (0x7f) included, when ensure_ascii is on
                let mut b = [0u16; 2];
                for u in c.encode_utf16(&mut b) {
                    out.push_str(&format!("\\u{:04x}", u));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Python's `repr(float)`.
pub fn float_repr(f: f64) -> String {
    if f.is_nan() {
        return "NaN".into();
    }
    if f.is_infinite() {
        return if f > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        };
    }
    if f == 0.0 {
        return if f.is_sign_negative() {
            "-0.0".into()
        } else {
            "0.0".into()
        };
    }
    // shortest round-trip digits and exponent from Rust's `{:e}`: "d.ddde-5"
    let e = format!("{:e}", f.abs());
    let (mant, exp) = e.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let decpt = exp + 1; // position of the decimal point relative to the digits
    let sign = if f < 0.0 { "-" } else { "" };
    if !(-3..=16).contains(&decpt) {
        let mut m = digits[..1].to_string();
        if digits.len() > 1 {
            m.push('.');
            m.push_str(&digits[1..]);
        }
        let x = decpt - 1;
        format!("{sign}{m}e{}{:02}", if x < 0 { '-' } else { '+' }, x.abs())
    } else if decpt <= 0 {
        format!("{sign}0.{}{}", "0".repeat((-decpt) as usize), digits)
    } else if (decpt as usize) >= digits.len() {
        format!(
            "{sign}{}{}.0",
            digits,
            "0".repeat(decpt as usize - digits.len())
        )
    } else {
        format!(
            "{sign}{}.{}",
            &digits[..decpt as usize],
            &digits[decpt as usize..]
        )
    }
}

// ---------------------------------------------------------------------------------------------------------------
// parse
// ---------------------------------------------------------------------------------------------------------------

#[derive(Debug)]
pub struct ParseError {
    pub msg: String,
    pub pos: usize,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at char {}", self.msg, self.pos)
    }
}

impl std::error::Error for ParseError {}

pub fn loads(text: &str) -> Result<Json, ParseError> {
    let b = text.as_bytes();
    let mut p = Parser { b, i: 0, text };
    p.ws();
    let v = p.value()?;
    p.ws();
    if p.i != b.len() {
        return Err(p.err("Extra data"));
    }
    Ok(v)
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
    text: &'a str,
}

impl Parser<'_> {
    fn err(&self, m: &str) -> ParseError {
        ParseError {
            msg: m.to_string(),
            pos: self.text[..self.i.min(self.text.len())].chars().count(),
        }
    }
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }
    fn lit(&mut self, s: &str, v: Json) -> Result<Json, ParseError> {
        if self.b[self.i..].starts_with(s.as_bytes()) {
            self.i += s.len();
            Ok(v)
        } else {
            Err(self.err("Expecting value"))
        }
    }
    fn value(&mut self) -> Result<Json, ParseError> {
        match self.b.get(self.i) {
            None => Err(self.err("Expecting value")),
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b'n') => self.lit("null", Json::Null),
            Some(b't') => self.lit("true", Json::Bool(true)),
            Some(b'f') => self.lit("false", Json::Bool(false)),
            Some(b'N') => self.lit("NaN", Json::Float(f64::NAN)),
            Some(b'I') => self.lit("Infinity", Json::Float(f64::INFINITY)),
            Some(b'-') if self.b[self.i..].starts_with(b"-Infinity") => {
                self.lit("-Infinity", Json::Float(f64::NEG_INFINITY))
            }
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.err("Expecting value")),
        }
    }
    fn number(&mut self) -> Result<Json, ParseError> {
        let start = self.i;
        if self.b[self.i] == b'-' {
            self.i += 1;
        }
        match self.b.get(self.i) {
            Some(b'0') => self.i += 1,
            Some(b'1'..=b'9') => {
                while self.b.get(self.i).is_some_and(u8::is_ascii_digit) {
                    self.i += 1;
                }
            }
            _ => return Err(self.err("Expecting value")),
        }
        let mut float = false;
        if self.b.get(self.i) == Some(&b'.')
            && self.b.get(self.i + 1).is_some_and(u8::is_ascii_digit)
        {
            float = true;
            self.i += 1;
            while self.b.get(self.i).is_some_and(u8::is_ascii_digit) {
                self.i += 1;
            }
        }
        if matches!(self.b.get(self.i), Some(b'e' | b'E')) {
            let mut j = self.i + 1;
            if matches!(self.b.get(j), Some(b'+' | b'-')) {
                j += 1;
            }
            if self.b.get(j).is_some_and(u8::is_ascii_digit) {
                float = true;
                while self.b.get(j).is_some_and(u8::is_ascii_digit) {
                    j += 1;
                }
                self.i = j;
            }
        }
        let s = &self.text[start..self.i];
        if float {
            s.parse::<f64>()
                .map(Json::Float)
                .map_err(|_| self.err("bad number"))
        } else {
            s.parse::<i128>()
                .map(Json::Int)
                .map_err(|_| self.err("integer too large"))
        }
    }
    fn hex4(&mut self) -> Result<u32, ParseError> {
        let h = self
            .text
            .get(self.i..self.i + 4)
            .ok_or_else(|| self.err("Invalid \\uXXXX escape"))?;
        let v = u32::from_str_radix(h, 16).map_err(|_| self.err("Invalid \\uXXXX escape"))?;
        self.i += 4;
        Ok(v)
    }
    fn string(&mut self) -> Result<String, ParseError> {
        self.i += 1;
        let mut out = String::new();
        loop {
            let Some(&c) = self.b.get(self.i) else {
                return Err(self.err("Unterminated string starting at"));
            };
            match c {
                b'"' => {
                    self.i += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.i += 1;
                    let Some(&e) = self.b.get(self.i) else {
                        return Err(self.err("Unterminated string starting at"));
                    };
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let mut cp = self.hex4()?;
                            if (0xD800..0xDC00).contains(&cp)
                                && self.b[self.i..].starts_with(b"\\u")
                            {
                                let save = self.i;
                                self.i += 2;
                                let lo = self.hex4()?;
                                if (0xDC00..0xE000).contains(&lo) {
                                    cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                                } else {
                                    self.i = save;
                                }
                            }
                            out.push(char::from_u32(cp).unwrap_or('\u{fffd}')); // a lone surrogate cannot live in a Rust String
                        }
                        _ => return Err(self.err("Invalid \\escape")),
                    }
                }
                c if c < 0x20 => return Err(self.err("Invalid control character at")),
                _ => {
                    // copy one UTF-8 char
                    let ch = self.text[self.i..].chars().next().unwrap();
                    out.push(ch);
                    self.i += ch.len_utf8();
                }
            }
        }
    }
    fn array(&mut self) -> Result<Json, ParseError> {
        self.i += 1;
        let mut v = Vec::new();
        self.ws();
        if self.b.get(self.i) == Some(&b']') {
            self.i += 1;
            return Ok(Json::Arr(v));
        }
        loop {
            self.ws();
            v.push(self.value()?);
            self.ws();
            match self.b.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(Json::Arr(v));
                }
                _ => return Err(self.err("Expecting ',' delimiter")),
            }
        }
    }
    fn object(&mut self) -> Result<Json, ParseError> {
        self.i += 1;
        let mut o = Json::obj();
        self.ws();
        if self.b.get(self.i) == Some(&b'}') {
            self.i += 1;
            return Ok(o);
        }
        loop {
            self.ws();
            if self.b.get(self.i) != Some(&b'"') {
                return Err(self.err("Expecting property name enclosed in double quotes"));
            }
            let k = self.string()?;
            self.ws();
            if self.b.get(self.i) != Some(&b':') {
                return Err(self.err("Expecting ':' delimiter"));
            }
            self.i += 1;
            self.ws();
            let v = self.value()?;
            o.set(&k, v);
            self.ws();
            match self.b.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(o);
                }
                _ => return Err(self.err("Expecting ',' delimiter")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_formats() {
        let mut o = Json::obj();
        o.set("b", Json::Int(1));
        o.set(
            "a",
            Json::Arr(vec![
                Json::str("x\u{2122}\"\n"),
                Json::Arr(vec![]),
                Json::obj(),
            ]),
        );
        o.set("c", Json::Bool(true));
        assert_eq!(
            dumps(&o, &Dump::compact()),
            r#"{"b": 1, "a": ["x\u2122\"\n", [], {}], "c": true}"#
        );
        assert_eq!(
            dumps(&o, &Dump::indented(2, false)),
            "{\n  \"b\": 1,\n  \"a\": [\n    \"x™\\\"\\n\",\n    [],\n    {}\n  ],\n  \"c\": true\n}"
        );
        assert_eq!(
            dumps(&o, &Dump::swift_pretty()),
            "{\n  \"a\" : [\n    \"x™\\\"\\n\",\n    [],\n    {}\n  ],\n  \"b\" : 1,\n  \"c\" : true\n}"
        );
        let back = loads(&dumps(&o, &Dump::compact())).unwrap();
        assert!(back.py_eq(&o));
        assert_eq!(back.keys(), vec!["b", "a", "c"]);
    }

    #[test]
    fn float_spelling_follows_python() {
        for (f, s) in [
            (1.0, "1.0"),
            (0.5, "0.5"),
            (1e16, "1e+16"),
            (1e15, "1000000000000000.0"),
            (1e-5, "1e-05"),
            (1e-4, "0.0001"),
            (123456789.125, "123456789.125"),
            (1.5e-7, "1.5e-07"),
            (-2.5, "-2.5"),
            (1.2345678901234568e17, "1.2345678901234568e+17"),
        ] {
            assert_eq!(float_repr(f), s);
        }
    }

    #[test]
    fn astral_and_escapes() {
        let v = Json::str("a\u{1F600}\u{7f}\u{1}");
        assert_eq!(
            dumps(&v, &Dump::compact()),
            "\"a\\ud83d\\ude00\\u007f\\u0001\""
        );
        assert!(
            loads("\"\\ud83d\\ude00\"")
                .unwrap()
                .py_eq(&Json::str("\u{1F600}"))
        );
        assert!(loads("{\"a\":1,}").is_err());
        assert!(loads("[1 2]").is_err());
    }
}
