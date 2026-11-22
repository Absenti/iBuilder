//! An insertion-ordered JSON value and Python's `json.dump(value, f, ensure_ascii=False, indent=1)` spelling (what `dmsh_build.atomic_json` writes).
//!
//! `serde_json::Value` sorts object keys (BTreeMap) and its pretty printer spells empty containers and separators differently, so the reflection is built and written with this type.
//! Dict semantics follow Python's: `set` on an existing key replaces the value **in place** (the key keeps its position), a new key goes last.

#[derive(Clone, Debug, PartialEq)]
pub enum J {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

impl From<bool> for J {
    fn from(b: bool) -> J {
        J::Bool(b)
    }
}
impl From<i64> for J {
    fn from(n: i64) -> J {
        J::Int(n)
    }
}
impl From<i32> for J {
    fn from(n: i32) -> J {
        J::Int(n as i64)
    }
}
impl From<u32> for J {
    fn from(n: u32) -> J {
        J::Int(n as i64)
    }
}
impl From<usize> for J {
    fn from(n: usize) -> J {
        J::Int(n as i64)
    }
}
impl From<&str> for J {
    fn from(s: &str) -> J {
        J::Str(s.to_string())
    }
}
impl From<String> for J {
    fn from(s: String) -> J {
        J::Str(s)
    }
}
impl<T: Into<J>> From<Option<T>> for J {
    fn from(o: Option<T>) -> J {
        match o {
            Some(v) => v.into(),
            None => J::Null,
        }
    }
}
impl<T: Into<J>> From<Vec<T>> for J {
    fn from(v: Vec<T>) -> J {
        J::Arr(v.into_iter().map(Into::into).collect())
    }
}

impl J {
    pub fn obj() -> J {
        J::Obj(Vec::new())
    }

    /// Python `d[k] = v`
    pub fn set(&mut self, k: &str, v: impl Into<J>) {
        let v = v.into();
        if let J::Obj(m) = self {
            if let Some(e) = m.iter_mut().find(|(n, _)| n == k) {
                e.1 = v;
            } else {
                m.push((k.to_string(), v));
            }
        }
    }
    /// builder form of `set`
    pub fn with(mut self, k: &str, v: impl Into<J>) -> J {
        self.set(k, v);
        self
    }
    pub fn get(&self, k: &str) -> Option<&J> {
        if let J::Obj(m) = self {
            m.iter().find(|(n, _)| n == k).map(|(_, v)| v)
        } else {
            None
        }
    }
    pub fn get_mut(&mut self, k: &str) -> Option<&mut J> {
        if let J::Obj(m) = self {
            m.iter_mut().find(|(n, _)| n == k).map(|(_, v)| v)
        } else {
            None
        }
    }
    pub fn has(&self, k: &str) -> bool {
        self.get(k).is_some()
    }
    /// Python `d.pop(k, None)`
    pub fn remove(&mut self, k: &str) {
        if let J::Obj(m) = self {
            m.retain(|(n, _)| n != k);
        }
    }
    /// Python `{**self, **other}`
    pub fn extend(&mut self, other: &J) {
        if let J::Obj(m) = other {
            for (k, v) in m {
                self.set(k, v.clone());
            }
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        if let J::Str(s) = self { Some(s) } else { None }
    }
    pub fn as_i64(&self) -> Option<i64> {
        if let J::Int(n) = self { Some(*n) } else { None }
    }
    pub fn as_bool(&self) -> Option<bool> {
        if let J::Bool(b) = self {
            Some(*b)
        } else {
            None
        }
    }
    pub fn as_arr(&self) -> Option<&Vec<J>> {
        if let J::Arr(a) = self { Some(a) } else { None }
    }
    pub fn is_truthy(&self) -> bool {
        match self {
            J::Null => false,
            J::Bool(b) => *b,
            J::Int(n) => *n != 0,
            J::Str(s) => !s.is_empty(),
            J::Arr(a) => !a.is_empty(),
            J::Obj(m) => !m.is_empty(),
        }
    }

    /// from a parsed `serde_json::Value` (objects keep serde's sorted order: only used for inputs whose key order is not written out)
    pub fn from_value(v: &serde_json::Value) -> J {
        use serde_json::Value as V;
        match v {
            V::Null => J::Null,
            V::Bool(b) => J::Bool(*b),
            V::Number(n) => n
                .as_i64()
                .map(J::Int)
                .unwrap_or_else(|| J::Int(n.as_f64().unwrap_or(0.0) as i64)),
            V::String(s) => J::Str(s.clone()),
            V::Array(a) => J::Arr(a.iter().map(J::from_value).collect()),
            V::Object(o) => J::Obj(
                o.iter()
                    .map(|(k, v)| (k.clone(), J::from_value(v)))
                    .collect(),
            ),
        }
    }
    pub fn to_value(&self) -> serde_json::Value {
        use serde_json::Value as V;
        match self {
            J::Null => V::Null,
            J::Bool(b) => V::Bool(*b),
            J::Int(n) => V::from(*n),
            J::Str(s) => V::String(s.clone()),
            J::Arr(a) => V::Array(a.iter().map(J::to_value).collect()),
            J::Obj(m) => V::Object(m.iter().map(|(k, v)| (k.clone(), v.to_value())).collect()),
        }
    }

    /// `json.dump(self, f, ensure_ascii=False, indent=1)`: no trailing newline
    pub fn dump(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out
    }
    /// `json.dumps(self)` with the default separators (`", "` and `": "`)
    pub fn dumps_compact(&self) -> String {
        let mut out = String::new();
        self.write_flat(&mut out);
        out
    }

    fn write(&self, out: &mut String, level: usize) {
        match self {
            J::Arr(a) if !a.is_empty() => {
                out.push('[');
                for (i, v) in a.iter().enumerate() {
                    out.push_str(if i == 0 { "\n" } else { ",\n" });
                    for _ in 0..=level {
                        out.push(' ');
                    }
                    v.write(out, level + 1);
                }
                out.push('\n');
                for _ in 0..level {
                    out.push(' ');
                }
                out.push(']');
            }
            J::Obj(m) if !m.is_empty() => {
                out.push('{');
                for (i, (k, v)) in m.iter().enumerate() {
                    out.push_str(if i == 0 { "\n" } else { ",\n" });
                    for _ in 0..=level {
                        out.push(' ');
                    }
                    escape(k, out);
                    out.push_str(": ");
                    v.write(out, level + 1);
                }
                out.push('\n');
                for _ in 0..level {
                    out.push(' ');
                }
                out.push('}');
            }
            other => other.write_flat(out),
        }
    }

    fn write_flat(&self, out: &mut String) {
        match self {
            J::Null => out.push_str("null"),
            J::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            J::Int(n) => out.push_str(&n.to_string()),
            J::Str(s) => escape(s, out),
            J::Arr(a) => {
                out.push('[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    v.write_flat(out);
                }
                out.push(']');
            }
            J::Obj(m) => {
                out.push('{');
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    escape(k, out);
                    out.push_str(": ");
                    v.write_flat(out);
                }
                out.push('}');
            }
        }
    }
}

/// Python's `json.encoder.encode_basestring` (the `ensure_ascii=False` form): `"`, `\`, `\b \f \n \r \t` and the other C0 controls as `\u00xx`; everything else, DEL and non-ASCII included, as it is
fn escape(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn spelling_matches_python() {
        let j = J::obj()
            .with("a", 1)
            .with("b", J::Arr(vec![]))
            .with("c", J::obj())
            .with("d", J::Arr(vec![J::Int(1), J::Str("x\"y\n\u{1}é".into())]))
            .with("e", J::Null)
            .with("f", true);
        // python3: json.dumps({"a": 1, "b": [], "c": {}, "d": [1, "x\"y\n\x01é"], "e": None, "f": True}, ensure_ascii=False, indent=1)
        let want = "{\n \"a\": 1,\n \"b\": [],\n \"c\": {},\n \"d\": [\n  1,\n  \"x\\\"y\\n\\u0001é\"\n ],\n \"e\": null,\n \"f\": true\n}";
        assert_eq!(j.dump(), want);
    }
    #[test]
    fn set_replaces_in_place() {
        let mut j = J::obj().with("a", 1).with("b", 2);
        j.set("a", 3);
        assert_eq!(j.dumps_compact(), "{\"a\": 3, \"b\": 2}");
    }
}
