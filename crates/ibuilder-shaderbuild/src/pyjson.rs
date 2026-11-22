//! `json.dumps` as CPython writes it, over `serde_json::Value` (with `preserve_order`: insertion order is the Python dict's).
//!
//! The translator's job file is `json.dump([job, ...], f)` (default separators `", "` and `": "`, `ensure_ascii=True`); the live facts rows are
//! `json.dumps(x, sort_keys=True, separators=(',', ':'))`; the reflection is `json.dump(indent=1, ensure_ascii=False)`. `serde_json` writes
//! none of these spellings, so the few differences are reproduced here (they are visible in files the engine reads back and in byte comparisons).
use serde_json::Value;

#[derive(Clone, Copy, Debug)]
pub struct Opts {
    pub indent: Option<usize>,
    pub item_sep: &'static str,
    pub key_sep: &'static str,
    pub ensure_ascii: bool,
    pub sort_keys: bool,
}

impl Opts {
    /// `json.dumps(x)`
    pub const DEFAULT: Opts = Opts {
        indent: None,
        item_sep: ", ",
        key_sep: ": ",
        ensure_ascii: true,
        sort_keys: false,
    };
    /// `json.dumps(x, sort_keys=True, separators=(',', ':'))`
    pub const COMPACT_SORTED: Opts = Opts {
        indent: None,
        item_sep: ",",
        key_sep: ":",
        ensure_ascii: true,
        sort_keys: true,
    };
    /// `json.dump(x, f, ensure_ascii=False, indent=1)`
    pub const INDENT1: Opts = Opts {
        indent: Some(1),
        item_sep: ",",
        key_sep: ": ",
        ensure_ascii: false,
        sort_keys: false,
    };
}

pub fn dumps(v: &Value, o: &Opts) -> String {
    let mut s = String::new();
    write(&mut s, v, o, 0);
    s
}

fn newline(s: &mut String, o: &Opts, level: usize) {
    if let Some(n) = o.indent {
        s.push('\n');
        for _ in 0..n * level {
            s.push(' ');
        }
    }
}

fn write(s: &mut String, v: &Value, o: &Opts, level: usize) {
    match v {
        Value::Null => s.push_str("null"),
        Value::Bool(true) => s.push_str("true"),
        Value::Bool(false) => s.push_str("false"),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                s.push_str(&i.to_string());
            } else if let Some(u) = n.as_u64() {
                s.push_str(&u.to_string());
            } else {
                s.push_str(&float_repr(n.as_f64().unwrap_or(0.0)));
            }
        }
        Value::String(t) => write_str(s, t, o.ensure_ascii),
        Value::Array(a) => {
            if a.is_empty() {
                s.push_str("[]");
                return;
            }
            s.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    s.push_str(o.item_sep);
                }
                newline(s, o, level + 1);
                write(s, x, o, level + 1);
            }
            newline(s, o, level);
            s.push(']');
        }
        Value::Object(m) => {
            if m.is_empty() {
                s.push_str("{}");
                return;
            }
            s.push('{');
            let mut entries: Vec<(&String, &Value)> = m.iter().collect();
            if o.sort_keys {
                // Python sorts str keys by code point; UTF-8 byte order is the same order
                entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            }
            for (i, (k, x)) in entries.into_iter().enumerate() {
                if i > 0 {
                    s.push_str(o.item_sep);
                }
                newline(s, o, level + 1);
                write_str(s, k, o.ensure_ascii);
                s.push_str(o.key_sep);
                write(s, x, o, level + 1);
            }
            newline(s, o, level);
            s.push('}');
        }
    }
}

/// Python's JSON string escaping (`json.encoder.py_encode_basestring[_ascii]`)
pub fn write_str(s: &mut String, t: &str, ensure_ascii: bool) {
    s.push('"');
    for c in t.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            '\t' => s.push_str("\\t"),
            '\u{8}' => s.push_str("\\b"),
            '\u{c}' => s.push_str("\\f"),
            c if (c as u32) < 0x20 => s.push_str(&format!("\\u{:04x}", c as u32)),
            c if ensure_ascii && (c as u32) > 0x7e => {
                // 0x7f is ASCII DEL: Python's ESCAPE_ASCII regex is `([\\"]|[^\ -~])`, so it is escaped too
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    s.push_str(&format!("\\u{:04x}", u));
                }
            }
            c => s.push(c),
        }
    }
    s.push('"');
}

/// `float.__repr__`: the shortest round-trip digits, positional between 1e-4 and 1e16, else `d.ddde+XX`
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
    let e = format!("{:e}", f); // shortest digits: "1.2345e-5", "1e16", "-3e0"
    let (mant, exp) = e.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    if (-4..16).contains(&exp) {
        let d = format!("{}", f); // Display: positional, shortest
        if d.contains('.') { d } else { format!("{d}.0") }
    } else {
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{}e{}{:02}", mant, sign, exp.abs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn spellings_match_python() {
        let v = json!({"b": [1, 2, {"x": "é\u{7f}\n"}], "a": {}, "c": [], "d": true, "e": null});
        assert_eq!(
            dumps(&v, &Opts::DEFAULT),
            r#"{"b": [1, 2, {"x": "\u00e9\u007f\n"}], "a": {}, "c": [], "d": true, "e": null}"#
        );
        assert_eq!(
            dumps(&v, &Opts::COMPACT_SORTED),
            r#"{"a":{},"b":[1,2,{"x":"\u00e9\u007f\n"}],"c":[],"d":true,"e":null}"#
        );
        assert_eq!(
            dumps(&json!({"k": [1, {"z": 2}]}), &Opts::INDENT1),
            "{\n \"k\": [\n  1,\n  {\n   \"z\": 2\n  }\n ]\n}"
        );
        assert_eq!(dumps(&json!("é"), &Opts::INDENT1), "\"é\"");
    }

    #[test]
    fn floats() {
        for (f, s) in [
            (1.0, "1.0"),
            (0.1, "0.1"),
            (1e16, "1e+16"),
            (1.5e-5, "1.5e-05"),
            (123456.789, "123456.789"),
            (-2.5, "-2.5"),
            (1e-4, "0.0001"),
        ] {
            assert_eq!(float_repr(f), s);
        }
    }
}
