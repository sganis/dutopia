// src/filepack.rs
//
// Storage format of the per-folder file index (`folder_files.data`, DB schema
// v4): one blob per folder instead of one SQLite row per file (~57 B/file in
// v3). It holds the totals of all regular files directly in the folder, per
// (user, age), plus a short largest-first list of the files worth acting on
// (dudb keeps the largest few above a size floor; the rest only count).
//
// Blob = 1 encoding byte (0 = raw, 1 = zstd) + payload. Tiny payloads stay
// raw: a zstd frame header alone outweighs a one-group, no-file folder.
// Payload (all integers LEB128 varints, columnar so similar values sit next
// to each other):
//   groups, then per group: user_id, age (1 byte), count, bytes,
//                           max atime (zigzag), max mtime (zigzag)
//   files,  then name_len × files, the name bytes concatenated,
//           user_id × files, age × files (1 byte each), size × files,
//           mtime × files and atime × files (zigzag delta from previous)
use anyhow::{bail, Context, Result};

const ZSTD_LEVEL: i32 = 3;
/// Payloads up to this size are stored raw.
const RAW_MAX: usize = 48;
const ENC_RAW: u8 = 0;
const ENC_ZSTD: u8 = 1;

/// One listed file.
#[derive(Debug, Clone, PartialEq)]
pub struct PackedFile {
    pub name: String,
    pub user_id: u32,
    pub age: u8,
    pub size: u64,
    pub atime: i64,
    pub mtime: i64,
}

/// Every regular file directly in the folder for one (user, age), listed or not.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GroupTotals {
    pub user_id: u32,
    pub age: u8,
    pub count: u64,
    pub bytes: u64,
    pub atime: i64,
    pub mtime: i64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PackedFolder {
    pub totals: Vec<GroupTotals>,
    /// Largest first.
    pub files: Vec<PackedFile>,
}

/// Reusable encoder: keeps one zstd context and scratch buffer across the
/// millions of folders of a `dudb --raw` build.
pub struct Packer {
    zstd: zstd::bulk::Compressor<'static>,
    raw: Vec<u8>,
}

impl Packer {
    pub fn new() -> Result<Self> {
        Ok(Self {
            zstd: zstd::bulk::Compressor::new(ZSTD_LEVEL)?,
            raw: Vec::new(),
        })
    }

    pub fn pack(&mut self, folder: &PackedFolder) -> Result<Vec<u8>> {
        let out = &mut self.raw;
        out.clear();
        put_varint(out, folder.totals.len() as u64);
        for t in &folder.totals {
            put_varint(out, t.user_id as u64);
            out.push(t.age);
            put_varint(out, t.count);
            put_varint(out, t.bytes);
            put_varint(out, zigzag(t.atime));
            put_varint(out, zigzag(t.mtime));
        }
        let files = &folder.files;
        put_varint(out, files.len() as u64);
        for f in files {
            put_varint(out, f.name.len() as u64);
        }
        for f in files {
            out.extend_from_slice(f.name.as_bytes());
        }
        for f in files {
            put_varint(out, f.user_id as u64);
        }
        out.extend(files.iter().map(|f| f.age));
        for f in files {
            put_varint(out, f.size);
        }
        put_deltas(out, files.iter().map(|f| f.mtime));
        put_deltas(out, files.iter().map(|f| f.atime));

        if self.raw.len() <= RAW_MAX {
            let mut blob = Vec::with_capacity(self.raw.len() + 1);
            blob.push(ENC_RAW);
            blob.extend_from_slice(&self.raw);
            return Ok(blob);
        }
        let mut blob = vec![ENC_ZSTD];
        blob.extend_from_slice(&self.zstd.compress(&self.raw)?);
        Ok(blob)
    }
}

/// Decode a blob written by `Packer::pack`.
pub fn unpack(blob: &[u8]) -> Result<PackedFolder> {
    let (&enc, body) = blob.split_first().context("filepack: empty blob")?;
    let raw;
    let payload = match enc {
        ENC_RAW => body,
        ENC_ZSTD => {
            raw = zstd::stream::decode_all(body).context("filepack: zstd")?;
            &raw[..]
        }
        other => bail!("filepack: unknown encoding {other}"),
    };
    let mut r = Reader { buf: payload, pos: 0 };

    let groups = r.count()?;
    let mut totals = Vec::with_capacity(groups);
    for _ in 0..groups {
        totals.push(GroupTotals {
            user_id: r.user_id()?,
            age: r.byte()?,
            count: r.varint()?,
            bytes: r.varint()?,
            atime: unzigzag(r.varint()?),
            mtime: unzigzag(r.varint()?),
        });
    }

    let count = r.count()?;
    let lens = (0..count)
        .map(|_| Ok(usize::try_from(r.varint()?)?))
        .collect::<Result<Vec<_>>>()?;
    let mut files = Vec::with_capacity(count);
    for len in lens {
        let name = String::from_utf8_lossy(r.take(len)?).into_owned();
        files.push(PackedFile { name, user_id: 0, age: 0, size: 0, atime: 0, mtime: 0 });
    }
    for f in files.iter_mut() {
        f.user_id = r.user_id()?;
    }
    for f in files.iter_mut() {
        f.age = r.byte()?;
    }
    for f in files.iter_mut() {
        f.size = r.varint()?;
    }
    let mut prev = 0i64;
    for f in files.iter_mut() {
        prev = prev.wrapping_add(unzigzag(r.varint()?));
        f.mtime = prev;
    }
    prev = 0;
    for f in files.iter_mut() {
        prev = prev.wrapping_add(unzigzag(r.varint()?));
        f.atime = prev;
    }
    if r.pos != payload.len() {
        bail!("filepack: {} trailing bytes", payload.len() - r.pos);
    }
    Ok(PackedFolder { totals, files })
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn put_deltas(out: &mut Vec<u8>, values: impl Iterator<Item = i64>) {
    let mut prev = 0i64;
    for v in values {
        put_varint(out, zigzag(v.wrapping_sub(prev)));
        prev = v;
    }
}

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn unzigzag(v: u64) -> i64 {
    (v >> 1) as i64 ^ -((v & 1) as i64)
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.buf.len());
        let Some(end) = end else { bail!("filepack: truncated") };
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn varint(&mut self) -> Result<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = self.byte()?;
            v |= ((b & 0x7F) as u64) << shift;
            if b < 0x80 {
                return Ok(v);
            }
        }
        bail!("filepack: varint overflow")
    }

    fn user_id(&mut self) -> Result<u32> {
        u32::try_from(self.varint()?).context("filepack: user_id")
    }

    /// An element count; every element takes at least one byte, so a count
    /// beyond the remaining bytes is corrupt (and must not drive an allocation).
    fn count(&mut self) -> Result<usize> {
        let n = usize::try_from(self.varint()?)?;
        if n > self.buf.len() - self.pos {
            bail!("filepack: count {n} exceeds blob size");
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, user_id: u32, age: u8, size: u64, atime: i64, mtime: i64) -> PackedFile {
        PackedFile { name: name.into(), user_id, age, size, atime, mtime }
    }

    fn totals(user_id: u32, age: u8, count: u64, bytes: u64) -> GroupTotals {
        GroupTotals { user_id, age, count, bytes, atime: -7, mtime: 1_700_000_000 }
    }

    fn round_trip(folder: &PackedFolder) -> Vec<u8> {
        let blob = Packer::new().unwrap().pack(folder).unwrap();
        assert_eq!(&unpack(&blob).unwrap(), folder);
        blob
    }

    #[test]
    fn round_trip_keeps_order_and_values() {
        let folder = PackedFolder {
            totals: vec![totals(3, 2, 10_000, u64::MAX), totals(0, 0, 1, 0)],
            files: vec![
                file("big.bin", 3, 2, u64::MAX, 1_700_000_000, 1_600_000_000),
                file("ñandú \u{FFFD}.txt", 0, 0, 0, 0, i64::MAX),
                file("", u32::MAX, 1, 7, i64::MIN, -5),
            ],
        };
        assert_eq!(round_trip(&folder)[0], ENC_ZSTD);
    }

    #[test]
    fn small_folders_stay_raw() {
        let totals_only = PackedFolder { totals: vec![totals(1, 0, 37, 120_000)], files: vec![] };
        let blob = round_trip(&totals_only);
        assert_eq!(blob[0], ENC_RAW);
        assert!(blob.len() < 20, "{} bytes", blob.len());
        assert_eq!(round_trip(&PackedFolder::default())[0], ENC_RAW);
    }

    #[test]
    fn packer_is_reusable() {
        let mut p = Packer::new().unwrap();
        let a = PackedFolder { totals: vec![totals(1, 0, 1, 10)], files: vec![file("a", 1, 0, 10, 1, 1)] };
        let b = PackedFolder {
            totals: vec![totals(2, 1, 2, 25)],
            files: vec![file("b", 2, 1, 20, 2, 2), file("c", 2, 1, 5, 3, 3)],
        };
        let (ba, bb) = (p.pack(&a).unwrap(), p.pack(&b).unwrap());
        assert_eq!(unpack(&ba).unwrap(), a);
        assert_eq!(unpack(&bb).unwrap(), b);
    }

    #[test]
    fn corrupt_blobs_are_errors_not_panics() {
        assert!(unpack(b"").is_err());
        assert!(unpack(b"\x07abc").is_err());
        assert!(unpack(b"\x01not zstd").is_err());
        // Huge counts are rejected before allocating.
        assert!(unpack(&[ENC_RAW, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F]).is_err());
        let folder = PackedFolder { totals: vec![totals(1, 0, 1, 1)], files: vec![file("x", 1, 0, 1, 1, 1)] };
        let mut blob = Packer::new().unwrap().pack(&folder).unwrap();
        assert_eq!(blob[0], ENC_RAW);
        blob.pop();
        assert!(unpack(&blob).is_err());
        blob.extend_from_slice(&[0, 0, 0]);
        assert!(unpack(&blob).is_err());
    }
}
