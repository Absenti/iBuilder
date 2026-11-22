//! Python's `json.dumps` spelling of the few values the split writes (ensure_ascii, `", "` / `": "` separators, sorted keys)

/// a JSON string as `json.dumps(str)` writes it (ASCII only: everything else is `\uXXXX`, astral characters as surrogate pairs)
pub fn string(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            '\u{8}' => o.push_str("\\b"),
            '\u{c}' => o.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut b = [0u16; 2];
                for u in c.encode_utf16(&mut b) {
                    o.push_str(&format!("\\u{:04x}", u));
                }
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// `json.dumps(value, sort_keys=True, separators=(',', ':'))` of a `serde_json::Value`
pub fn compact_sorted(v: &serde_json::Value) -> String {
    use serde_json::Value;
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => if *b { "true" } else { "false" }.into(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => string(s),
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(compact_sorted).collect::<Vec<_>>().join(",")
        ),
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes())); // Python sorts str by code point, and UTF-8 byte order is code point order. (UTF-16 order is NOT: an astral character's surrogates, D800..DFFF, sort before U+E000..U+FFFF.)
            format!(
                "{{{}}}",
                keys.iter()
                    .map(|k| format!("{}:{}", string(k), compact_sorted(&m[*k])))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_ascii_strings() {
        assert_eq!(string("a\"b\\c\n"), "\"a\\\"b\\\\c\\n\"");
        assert_eq!(string("\u{e9}"), "\"\\u00e9\"");
        assert_eq!(string("\u{1f600}"), "\"\\ud83d\\ude00\"");
        assert_eq!(string("\u{7f}"), "\"\\u007f\"");
    }

    /// Python: `json.dumps({"\U00010000": 1, "\uffff": 2, "a": 3}, sort_keys=True, separators=(",", ":"))` is `{"a":3,"\uffff":2,"\ud800\udc00":1}` (checked with python3 3.14): U+FFFF < U+10000 by code point,
    /// whereas in UTF-16 the astral character's first unit (D800) is below FFFF.
    #[test]
    fn keys_sort_by_code_point_astral_ones_after_the_bmp() {
        let v: serde_json::Value =
            serde_json::from_str("{\"\u{10000}\":1,\"\u{ffff}\":2,\"a\":3}").unwrap();
        assert_eq!(compact_sorted(&v), r#"{"a":3,"\uffff":2,"\ud800\udc00":1}"#);
        // U+E000 (a BMP private-use character) also sorts below U+10000, which UTF-16 order gets wrong the same way
        let v: serde_json::Value =
            serde_json::from_str("{\"\u{1f600}\":1,\"\u{e000}\":2}").unwrap();
        assert_eq!(compact_sorted(&v), r#"{"\ue000":2,"\ud83d\ude00":1}"#);
    }

    #[test]
    fn compact_sorted_objects() {
        let v: serde_json::Value =
            serde_json::from_str(r#"{"shared":0,"local":[1,2,3],"z":{"b":1,"a":"x"}}"#).unwrap();
        assert_eq!(
            compact_sorted(&v),
            r#"{"local":[1,2,3],"shared":0,"z":{"a":"x","b":1}}"#
        );
    }
}
