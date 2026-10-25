//! JUCE `ValueTree` binary streams, as ShaperBox 3 stores its state and presets.
//!
//! A preset file is `#zip#\0` followed by a zlib stream of `ValueTree::writeToStream`:
//! `tree := name\0 count:cint (prop\0 var)* children:cint tree*`, `var := size:cint type:u8 payload`,
//! and `cint` is one byte holding the byte count (bit 7 = negative) followed by that many
//! little-endian bytes. Variant types follow `juce_Variant.cpp`.

const MAGIC: &[u8] = b"#zip#\0";

/// A `juce::var` value.
#[derive(Clone, Debug, PartialEq)]
pub enum Var {
    Void,
    Int(i32),
    Bool(bool),
    Double(f64),
    String(String),
    Int64(i64),
    Array(Vec<Var>),
    Binary(Vec<u8>),
}

/// A `juce::ValueTree` node: a type name, ordered properties and children.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tree {
    pub kind: String,
    pub props: Vec<(String, Var)>,
    pub children: Vec<Tree>,
}

impl Tree {
    /// The value of a property.
    pub fn prop(&self, name: &str) -> Option<&Var> {
        self.props.iter().find(|(k, _)| k == name).map(|(_, v)| v)
    }

    /// The first child of a type.
    pub fn child(&self, kind: &str) -> Option<&Tree> {
        self.children.iter().find(|c| c.kind == kind)
    }

    /// The first child whose `id` property is `id` (ShaperBox names its state nodes this way).
    pub fn by_id(&self, id: &str) -> Option<&Tree> {
        self.children
            .iter()
            .find(|c| matches!(c.prop("id"), Some(Var::String(s)) if s == id))
    }

    /// Decodes a tree from `ValueTree::writeToStream` bytes.
    pub fn read(data: &[u8]) -> Result<Self, String> {
        let mut r = Reader { d: data, p: 0 };
        r.tree()
    }

    /// Encodes the tree as `ValueTree::writeToStream` does.
    pub fn write(&self, out: &mut Vec<u8>) {
        cstring(out, &self.kind);
        cint(out, self.props.len() as i64);
        for (k, v) in &self.props {
            cstring(out, k);
            var(out, v);
        }
        cint(out, self.children.len() as i64);
        for c in &self.children {
            c.write(out);
        }
    }
}

/// Decodes a ShaperBox preset file (`#zip#\0` + zlib).
pub fn read_preset(data: &[u8]) -> Result<Tree, String> {
    let body = data
        .strip_prefix(MAGIC)
        .ok_or("not a plain ShaperBox preset (no #zip#\\0 header)")?;
    let raw =
        miniz_oxide::inflate::decompress_to_vec_zlib(body).map_err(|e| format!("zlib: {e:?}"))?;
    Tree::read(&raw)
}

/// Encodes a tree as a ShaperBox preset file.
pub fn write_preset(tree: &Tree) -> Vec<u8> {
    let mut raw = Vec::new();
    tree.write(&mut raw);
    let mut out = MAGIC.to_vec();
    out.extend(miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6));
    out
}

struct Reader<'a> {
    d: &'a [u8],
    p: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let s = self
            .d
            .get(self.p..self.p + n)
            .ok_or_else(|| format!("truncated at byte {}", self.p))?;
        self.p += n;
        Ok(s)
    }

    fn cint(&mut self) -> Result<i64, String> {
        let n = self.take(1)?[0];
        let len = (n & 0x7f) as usize;
        if len > 8 {
            return Err(format!("bad compressed int length {len}"));
        }
        let mut v = 0i64;
        for (i, b) in self.take(len)?.iter().enumerate() {
            v |= (*b as i64) << (8 * i);
        }
        Ok(if n & 0x80 != 0 { -v } else { v })
    }

    fn cstring(&mut self) -> Result<String, String> {
        let rest = &self.d[self.p..];
        let end = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or("unterminated string")?;
        let s = String::from_utf8_lossy(&rest[..end]).into_owned();
        self.p += end + 1;
        Ok(s)
    }

    fn var(&mut self) -> Result<Var, String> {
        let size = self.cint()? as usize;
        if size == 0 {
            return Ok(Var::Void);
        }
        let t = self.take(1)?[0];
        let b = self.take(size - 1)?;
        decode(t, b)
    }

    fn tree(&mut self) -> Result<Tree, String> {
        let kind = self.cstring()?;
        let n = self.cint()?;
        let mut props = Vec::with_capacity(n.max(0) as usize);
        for _ in 0..n {
            let k = self.cstring()?;
            props.push((k, self.var()?));
        }
        let n = self.cint()?;
        let mut children = Vec::with_capacity(n.max(0) as usize);
        for _ in 0..n {
            children.push(self.tree()?);
        }
        Ok(Tree {
            kind,
            props,
            children,
        })
    }
}

fn decode(t: u8, b: &[u8]) -> Result<Var, String> {
    let le4 = |b: &[u8]| -> Result<[u8; 4], String> {
        b.get(..4)
            .and_then(|s| s.try_into().ok())
            .ok_or("short var".into())
    };
    let le8 = |b: &[u8]| -> Result<[u8; 8], String> {
        b.get(..8)
            .and_then(|s| s.try_into().ok())
            .ok_or("short var".into())
    };
    Ok(match t {
        1 => Var::Int(i32::from_le_bytes(le4(b)?)),
        2 => Var::Bool(true),
        3 => Var::Bool(false),
        4 => Var::Double(f64::from_le_bytes(le8(b)?)),
        5 => Var::String(String::from_utf8_lossy(b.strip_suffix(&[0]).unwrap_or(b)).into_owned()),
        6 => Var::Int64(i64::from_le_bytes(le8(b)?)),
        7 => {
            let mut r = Reader { d: b, p: 0 };
            let n = r.cint()?;
            Var::Array((0..n).map(|_| r.var()).collect::<Result<_, _>>()?)
        }
        8 => Var::Binary(b.to_vec()),
        _ => Var::Void,
    })
}

fn cint(out: &mut Vec<u8>, v: i64) {
    let neg = v < 0;
    let mut m = v.unsigned_abs();
    let mut bytes = Vec::new();
    while m > 0 {
        bytes.push(m as u8);
        m >>= 8;
    }
    out.push(bytes.len() as u8 | if neg { 0x80 } else { 0 });
    out.extend(bytes);
}

fn cstring(out: &mut Vec<u8>, s: &str) {
    out.extend(s.as_bytes());
    out.push(0);
}

fn var(out: &mut Vec<u8>, v: &Var) {
    let mut body = Vec::new();
    match v {
        Var::Void => {
            cint(out, 0);
            return;
        }
        Var::Int(i) => {
            body.push(1);
            body.extend(i.to_le_bytes());
        }
        Var::Bool(b) => body.push(if *b { 2 } else { 3 }),
        Var::Double(d) => {
            body.push(4);
            body.extend(d.to_le_bytes());
        }
        Var::String(s) => {
            body.push(5);
            body.extend(s.as_bytes());
            body.push(0);
        }
        Var::Int64(i) => {
            body.push(6);
            body.extend(i.to_le_bytes());
        }
        Var::Array(a) => {
            body.push(7);
            cint(&mut body, a.len() as i64);
            for x in a {
                var(&mut body, x);
            }
        }
        Var::Binary(b) => {
            body.push(8);
            body.extend(b);
        }
    }
    cint(out, body.len() as i64);
    out.extend(body);
}
