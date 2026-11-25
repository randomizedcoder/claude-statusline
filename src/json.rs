// Tiny JSON: a parser sufficient for Claude Code's and Codex's inputs, and
// the writer helpers the Codex MCP replies need. std only.

use std::borrow::Cow;

// Strings borrow from the input unless they contain escapes (rare), so a
// parse allocates little more than the object/array Vecs.
pub(crate) enum Val<'a> {
    Null,
    Bool(bool),
    Num(f64),
    Str(Cow<'a, str>),
    Arr(Vec<Val<'a>>),
    Obj(Vec<(Cow<'a, str>, Val<'a>)>),
}

impl<'a> Val<'a> {
    pub(crate) fn get(&self, k: &str) -> Option<&Val<'a>> {
        match self {
            Val::Obj(o) => o.iter().find(|(n, _)| n == k).map(|(_, v)| v),
            _ => None,
        }
    }
    pub(crate) fn as_f64(&self) -> Option<f64> {
        match self {
            Val::Num(n) => Some(*n),
            _ => None,
        }
    }
    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Val::Str(s) => Some(s),
            _ => None,
        }
    }
}

struct P<'a> {
    s: &'a str,
    b: &'a [u8],
    i: usize,
}

impl<'a> P<'a> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }
    fn eat(&mut self) -> Option<u8> {
        let c = self.peek()?;
        self.i += 1;
        Some(c)
    }
    fn expect(&mut self, c: u8) -> Option<()> {
        if self.eat()? == c {
            Some(())
        } else {
            None
        }
    }
    fn lit<const N: usize>(&mut self, s: &[u8; N]) -> Option<()> {
        if self.b.get(self.i..self.i + N)? == &s[..] {
            self.i += N;
            Some(())
        } else {
            None
        }
    }

    fn value(&mut self) -> Option<Val<'a>> {
        self.ws();
        match self.peek()? {
            b'"' => Some(Val::Str(self.string()?)),
            b'{' => self.object(),
            b'[' => self.array(),
            b't' => {
                self.lit(b"true")?;
                Some(Val::Bool(true))
            }
            b'f' => {
                self.lit(b"false")?;
                Some(Val::Bool(false))
            }
            b'n' => {
                self.lit(b"null")?;
                Some(Val::Null)
            }
            _ => self.number(),
        }
    }

    fn string(&mut self) -> Option<Cow<'a, str>> {
        self.expect(b'"')?;
        let start = self.i;
        // Fast path: no escapes, so borrow. Both ends are at an ASCII '"',
        // which is always a char boundary, so the slice can't panic.
        while let Some(&c) = self.b.get(self.i) {
            match c {
                b'"' => {
                    let s = &self.s[start..self.i];
                    self.i += 1;
                    return Some(Cow::Borrowed(s));
                }
                b'\\' => break,
                _ => self.i += 1,
            }
        }
        // Slow path: unescape into an owned copy (or hit EOF and fail).
        let mut raw = self.b[start..self.i].to_vec();
        loop {
            let c = self.eat()?;
            match c {
                b'"' => return String::from_utf8(raw).ok().map(Cow::Owned),
                b'\\' => {
                    let e = self.eat()?;
                    match e {
                        b'"' => raw.push(b'"'),
                        b'\\' => raw.push(b'\\'),
                        b'/' => raw.push(b'/'),
                        b'n' => raw.push(b'\n'),
                        b't' => raw.push(b'\t'),
                        b'r' => raw.push(b'\r'),
                        b'b' => raw.push(0x08),
                        b'f' => raw.push(0x0C),
                        b'u' => {
                            let hex =
                                std::str::from_utf8(self.b.get(self.i..self.i + 4)?).ok()?;
                            self.i += 4;
                            let cp = u32::from_str_radix(hex, 16).ok()?;
                            if let Some(ch) = char::from_u32(cp) {
                                let mut buf = [0u8; 4];
                                raw.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                            }
                        }
                        _ => return None,
                    }
                }
                _ => raw.push(c),
            }
        }
    }

    fn number(&mut self) -> Option<Val<'a>> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-') {
                self.i += 1;
            } else {
                break;
            }
        }
        // Only ASCII was consumed, so both ends are char boundaries.
        self.s[start..self.i].parse::<f64>().ok().map(Val::Num)
    }

    fn object(&mut self) -> Option<Val<'a>> {
        self.expect(b'{')?;
        // Most objects in both inputs have a handful of keys.
        let mut o = Vec::with_capacity(8);
        self.ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Some(Val::Obj(o));
        }
        loop {
            self.ws();
            let k = self.string()?;
            self.ws();
            self.expect(b':')?;
            let v = self.value()?;
            o.push((k, v));
            self.ws();
            match self.eat()? {
                b',' => continue,
                b'}' => return Some(Val::Obj(o)),
                _ => return None,
            }
        }
    }

    fn array(&mut self) -> Option<Val<'a>> {
        self.expect(b'[')?;
        let mut a = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            return Some(Val::Arr(a));
        }
        loop {
            let v = self.value()?;
            a.push(v);
            self.ws();
            match self.eat()? {
                b',' => continue,
                b']' => return Some(Val::Arr(a)),
                _ => return None,
            }
        }
    }
}

pub(crate) fn parse(s: &str) -> Option<Val<'_>> {
    let mut p = P { s, b: s.as_bytes(), i: 0 };
    p.value()
}

// ---------- writing ----------

// Append `s` as a JSON string literal (quotes included).
pub(crate) fn push_str(out: &mut String, s: &str) {
    out.push('"');
    let mut start = 0;
    for (i, b) in s.bytes().enumerate() {
        let esc: &str = match b {
            b'"' => "\\\"",
            b'\\' => "\\\\",
            b'\n' => "\\n",
            b'\r' => "\\r",
            b'\t' => "\\t",
            0..=0x1f => "",
            _ => continue,
        };
        out.push_str(&s[start..i]);
        if esc.is_empty() {
            out.push_str(&format!("\\u{b:04x}"));
        } else {
            out.push_str(esc);
        }
        start = i + 1;
    }
    out.push_str(&s[start..]);
    out.push('"');
}

// Append any parsed value back as JSON (used to echo JSON-RPC ids).
pub(crate) fn push_val(out: &mut String, v: &Val) {
    match v {
        Val::Null => out.push_str("null"),
        Val::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Val::Num(n) if n.fract() == 0.0 && n.abs() < 9.0e15 => out.push_str(&(*n as i64).to_string()),
        Val::Num(n) if n.is_finite() => out.push_str(&n.to_string()),
        Val::Num(_) => out.push_str("null"),
        Val::Str(s) => push_str(out, s),
        Val::Arr(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                push_val(out, x);
            }
            out.push(']');
        }
        Val::Obj(o) => {
            out.push('{');
            for (i, (k, x)) in o.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                push_str(out, k);
                out.push(':');
                push_val(out, x);
            }
            out.push('}');
        }
    }
}
