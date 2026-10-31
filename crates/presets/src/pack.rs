//! The preset pack: `BKSPPACK`, a u32 version, then five sections. Three hold named files (the
//! Gross Beat state presets, the envelope maps and the shape presets), the fourth holds the
//! shape preset groups as indices into the shape presets, and the fifth the editor's FL fonts. Integers are little-endian u32;
//! strings and files are a u32 length and the bytes.

const MAGIC: &[u8; 8] = b"BKSPPACK";
const VERSION: u32 = 2;

/// A pack being built.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PackData {
    pub gross_beat: Vec<(String, Vec<u8>)>,
    pub maps: Vec<(String, Vec<u8>)>,
    pub shapes: Vec<(String, Vec<u8>)>,
    pub groups: Vec<(String, Vec<u32>)>,
    pub fonts: Vec<(String, Vec<u8>)>,
}

/// A pack read from bytes that live for the whole program (the embedded pack).
#[derive(Clone, Debug, Default)]
pub struct Embedded {
    pub gross_beat: Vec<(&'static str, &'static [u8])>,
    pub maps: Vec<(&'static str, &'static [u8])>,
    pub shapes: Vec<(&'static str, &'static [u8])>,
    pub groups: Vec<(&'static str, Vec<u32>)>,
    pub fonts: Vec<(&'static str, &'static [u8])>,
}

fn put(out: &mut Vec<u8>, b: &[u8]) {
    out.extend((b.len() as u32).to_le_bytes());
    out.extend(b);
}

fn put_files(out: &mut Vec<u8>, files: &[(String, Vec<u8>)]) {
    out.extend((files.len() as u32).to_le_bytes());
    for (name, data) in files {
        put(out, name.as_bytes());
        put(out, data);
    }
}

impl PackData {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        out.extend(VERSION.to_le_bytes());
        put_files(&mut out, &self.gross_beat);
        put_files(&mut out, &self.maps);
        put_files(&mut out, &self.shapes);
        out.extend((self.groups.len() as u32).to_le_bytes());
        for (name, items) in &self.groups {
            put(&mut out, name.as_bytes());
            out.extend((items.len() as u32).to_le_bytes());
            for i in items {
                out.extend(i.to_le_bytes());
            }
        }
        put_files(&mut out, &self.fonts);
        out
    }
}

struct Reader {
    d: &'static [u8],
    p: usize,
}

impl Reader {
    fn u32(&mut self) -> Option<u32> {
        let b = self.d.get(self.p..self.p + 4)?;
        self.p += 4;
        Some(u32::from_le_bytes(b.try_into().ok()?))
    }

    fn bytes(&mut self) -> Option<&'static [u8]> {
        let n = self.u32()? as usize;
        let b = self.d.get(self.p..self.p + n)?;
        self.p += n;
        Some(b)
    }

    fn files(&mut self) -> Option<Vec<(&'static str, &'static [u8])>> {
        (0..self.u32()?)
            .map(|_| Some((std::str::from_utf8(self.bytes()?).ok()?, self.bytes()?)))
            .collect()
    }
}

impl Embedded {
    /// Reads a pack; `None` for empty or malformed bytes.
    pub fn decode(d: &'static [u8]) -> Option<Self> {
        if d.get(..8)? != MAGIC {
            return None;
        }
        let mut r = Reader { d, p: 8 };
        if r.u32()? != VERSION {
            return None;
        }
        let (gross_beat, maps, shapes) = (r.files()?, r.files()?, r.files()?);
        let groups = (0..r.u32()?)
            .map(|_| {
                let name = std::str::from_utf8(r.bytes()?).ok()?;
                let items = (0..r.u32()?).map(|_| r.u32()).collect::<Option<_>>()?;
                Some((name, items))
            })
            .collect::<Option<_>>()?;
        let fonts = r.files()?;
        Some(Self {
            gross_beat,
            maps,
            shapes,
            groups,
            fonts,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let pack = PackData {
            gross_beat: vec![("Momentary".into(), vec![1, 2, 3])],
            maps: vec![("Default".into(), vec![])],
            shapes: vec![("Init".into(), vec![9; 40]), ("Wobble".into(), vec![7])],
            groups: vec![("Motion@Wobble".into(), vec![1, 0])],
            fonts: vec![("Ubuntu-R.ttf".into(), vec![5, 6])],
        };
        let bytes: &'static [u8] = pack.encode().leak();
        let e = Embedded::decode(bytes).unwrap();
        assert_eq!(e.gross_beat, [("Momentary", &[1u8, 2, 3][..])]);
        assert_eq!(e.maps, [("Default", &[][..])]);
        assert_eq!(e.shapes[1], ("Wobble", &[7u8][..]));
        assert_eq!(e.groups, [("Motion@Wobble", vec![1, 0])]);
        assert_eq!(e.fonts, [("Ubuntu-R.ttf", &[5u8, 6][..])]);
        assert!(Embedded::decode(&[]).is_none());
        assert!(Embedded::decode(&bytes[..bytes.len() - 1]).is_none());
    }
}
