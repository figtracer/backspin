//! Envelope records (TCustomMEnvelope.SaveRestore 0x238ab0) and `.fnv` files.

use crate::{Envelope, KIND_MAP, Point};
use std::fmt;

/// The envelope record version the original writes.
pub const RECORD_VERSION: i32 = 3;

/// A read cursor with the semantics of the original's memory stream: a read past the end copies
/// the bytes that are left and leaves the rest of the target unchanged.
#[derive(Clone, Debug)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// Creates a reader at the start of `data`.
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// The number of bytes consumed.
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// Copies up to `buf.len()` bytes into `buf`.
    pub fn read(&mut self, buf: &mut [u8]) {
        let n = buf.len().min(self.data.len() - self.pos);
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
    }

    /// Reads a little-endian i32 over `init`.
    pub fn i32(&mut self, init: i32) -> i32 {
        let mut b = init.to_le_bytes();
        self.read(&mut b);
        i32::from_le_bytes(b)
    }

    /// Reads a little-endian u32 over `init`.
    pub fn u32(&mut self, init: u32) -> u32 {
        let mut b = init.to_le_bytes();
        self.read(&mut b);
        u32::from_le_bytes(b)
    }

    /// Reads one byte over `init`.
    pub fn u8(&mut self, init: u8) -> u8 {
        let mut b = [init];
        self.read(&mut b);
        b[0]
    }
}

/// An error from [`read_fnv`].
#[derive(Debug)]
pub enum Error {
    /// The file could not be read or written.
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl Envelope {
    /// Loads a record into this envelope (0x238ab0 with `save = false`).
    ///
    /// The record starts with the kind. If it is not 3, nothing more is read and the envelope
    /// stays as it is. A version of 4 or more is read but nothing else is. Points that already
    /// exist keep their derived bytes, as `SetLength` keeps them in the original.
    pub fn load(&mut self, r: &mut Reader<'_>) {
        self.load_with(r, false);
    }

    /// Loads a record of any kind (the "bypass" load of 0x239070 for `.fnv` files and presets).
    pub fn load_any(&mut self, data: &[u8]) {
        self.load_with(&mut Reader::new(data), true);
    }

    fn load_with(&mut self, r: &mut Reader<'_>, bypass: bool) {
        let kind = r.i32(i32::MAX);
        if kind != KIND_MAP && !bypass {
            return;
        }
        let version = r.i32(i32::MAX);
        if version >= 4 {
            return;
        }
        let count = r.i32(0).max(0) as usize;
        self.points.resize(count, Point::new(0.0, 0.0, 0.0, 0));
        if version < 1 {
            for p in &mut self.points {
                p.shape = 0;
                p.arp = 0;
                p.flags = 0;
            }
        }
        let mut prev = 0f32;
        for p in &mut self.points {
            if version < 2 {
                let mut b = (p.dx as f32).to_bits().to_le_bytes();
                r.read(&mut b);
                p.dx = f32::from_le_bytes(b) as f64;
                let mut b = p.value.to_le_bytes();
                r.read(&mut b);
                p.value = f32::from_le_bytes(b);
            } else {
                let mut b = [0u8; 16];
                b[..8].copy_from_slice(&p.dx.to_le_bytes());
                b[8..].copy_from_slice(&(p.value as f64).to_le_bytes());
                r.read(&mut b);
                p.dx = f64::from_le_bytes(b[..8].try_into().unwrap());
                p.value = f64::from_le_bytes(b[8..].try_into().unwrap()) as f32;
            }
            // 0xf18a0: a NaN value is replaced by the previous point's value (0 for the first).
            if p.value.to_bits() & 0x7fff_ffff > 0x7f80_0000 {
                p.value = prev;
            }
            prev = p.value;
            let mut tail = tail_bytes(p);
            let n = if version < 1 { 4 } else { 8 };
            r.read(&mut tail[..n]);
            set_tail_bytes(p, &tail);
        }
        let mut header = [0u8; 20];
        header[..4].copy_from_slice(&self.flags.to_le_bytes());
        header[4..8].copy_from_slice(&self.marker.to_le_bytes());
        header[8..].copy_from_slice(&[0xff; 12]);
        r.read(&mut header);
        self.flags = u32::from_le_bytes(header[..4].try_into().unwrap());
        self.marker = i32::from_le_bytes(header[4..8].try_into().unwrap());
        // TMMap load flags (0x20): arp bytes and the 0x4c..0x54 markers are reset.
        for p in &mut self.points {
            p.arp = 0;
        }
        self.fix_length();
        if self.points.is_empty() {
            self.points.push(Point::new(0.0, 0.0, 0.0, 0));
        }
        self.prepare();
    }

    /// Writes the record (0x238ab0 with `save = true`): version 3, every point with its 8 raw
    /// tail bytes, then the flags, the extra marker and three reset markers.
    pub fn save(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&KIND_MAP.to_le_bytes());
        out.extend_from_slice(&RECORD_VERSION.to_le_bytes());
        out.extend_from_slice(&(self.points.len() as i32).to_le_bytes());
        for p in &self.points {
            out.extend_from_slice(&p.dx.to_le_bytes());
            out.extend_from_slice(&(p.value as f64).to_le_bytes());
            out.extend_from_slice(&tail_bytes(p));
        }
        out.extend_from_slice(&self.flags.to_le_bytes());
        out.extend_from_slice(&self.marker.to_le_bytes());
        for _ in 0..3 {
            out.extend_from_slice(&(-1i32).to_le_bytes());
        }
    }

    /// Scales every `dx` so the envelope spans its length (0x23c260).
    ///
    /// With no points, a point at 0 with the default value is added. If point 0 does not start at
    /// 0, a copy of it is inserted (it lands at index 1 with `dx` 0, as in the original). If the
    /// total is 0, a point at `length` with the last value is added.
    pub fn fix_length(&mut self) {
        if self.length == 2147483647.0 {
            return;
        }
        if self.points.is_empty() {
            let v = self.default_value() as f32;
            self.insert_point(0.0, v, 0.0, true, false);
        } else if 0.0 < self.points[0].dx {
            let v = self.points[0].value;
            self.insert_point(0.0, v, 0.0, true, false);
        }
        let total = self.abs_pos(self.hi());
        if 0.0 < total {
            let k = self.length / total;
            if k != 1.0 {
                for p in &mut self.points {
                    p.dx *= k;
                }
            }
        } else {
            let v = self.points[self.hi()].value;
            self.insert_point(self.length, v, 0.0, true, false);
        }
    }
}

/// Reads a `.fnv` file (one envelope record, no header) into `env`, which keeps its length.
pub fn read_fnv(path: &std::path::Path, env: &mut Envelope) -> Result<(), Error> {
    let data = std::fs::read(path)?;
    env.load(&mut Reader::new(&data));
    Ok(())
}

/// Writes `env` as a `.fnv` file.
pub fn write_fnv(path: &std::path::Path, env: &Envelope) -> Result<(), Error> {
    let mut out = Vec::new();
    env.save(&mut out);
    std::fs::write(path, out)?;
    Ok(())
}

/// The 8 bytes at point+0x0c: tension, shape, arp, flags, sign.
fn tail_bytes(p: &Point) -> [u8; 8] {
    let t = p.tension.to_le_bytes();
    [
        t[0],
        t[1],
        t[2],
        t[3],
        p.shape,
        p.arp,
        p.flags,
        p.sign as u8,
    ]
}

fn set_tail_bytes(p: &mut Point, b: &[u8; 8]) {
    p.tension = f32::from_le_bytes(b[..4].try_into().unwrap());
    p.shape = b[4];
    p.arp = b[5];
    p.flags = b[6];
    p.sign = b[7] as i8;
}
