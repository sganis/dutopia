// src/bin/dudb/extsort.rs
//
// External merge sort for the per-file rows of `dudb --raw`. duscan emits a
// folder's files scattered across the CSV; sorting a billion rows by
// (folder_id, name) in bounded memory brings each folder's files together so
// `folder_pack` can write one row per folder, in ascending folder_id order.
//
// Runs are zstd-compressed record streams. Equal keys keep input order (stable
// sort within a run, lower run index first across runs), so the merge keeps
// the first occurrence of a duplicated path in CSV order.
use anyhow::{bail, Context, Result};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::fs::File;
use std::io::{BufReader, BufWriter, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

/// folder_id(8) user_id(4) age(1) size(8) atime(8) mtime(8) name_len(4)
const HDR: usize = 41;
const ZSTD_LEVEL: i32 = 1;
const IO_BUF: usize = 1 << 20;

/// Non-key columns of a `files` row.
#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub struct FileMeta {
    pub user_id: u32,
    pub age: u8,
    pub size: u64,
    pub atime: i64,
    pub mtime: i64,
}

/// One `files` row. `name` is already valid UTF-8 (lossy-converted on parse).
#[derive(Default, Debug, Clone, PartialEq)]
pub struct FileRec {
    pub folder_id: i64,
    pub name: Vec<u8>,
    pub meta: FileMeta,
}

/// In-memory run: records encoded back to back in `arena`, plus a
/// (folder_id, offset) index that is what actually gets sorted.
#[derive(Default)]
pub struct RunBuffer {
    arena: Vec<u8>,
    index: Vec<(i64, usize)>,
}

impl RunBuffer {
    /// Pre-size for a `bytes` budget (~3/4 arena, ~1/4 index at typical
    /// record sizes) so the buffers don't double past the budget.
    pub fn with_capacity(bytes: usize) -> Self {
        let entries = bytes / 4 / std::mem::size_of::<(i64, usize)>();
        Self {
            arena: Vec::with_capacity(bytes - bytes / 4),
            index: Vec::with_capacity(entries),
        }
    }

    pub fn push(&mut self, folder_id: i64, name: &[u8], m: FileMeta) {
        self.index.push((folder_id, self.arena.len()));
        let a = &mut self.arena;
        a.extend_from_slice(&folder_id.to_le_bytes());
        a.extend_from_slice(&m.user_id.to_le_bytes());
        a.push(m.age);
        a.extend_from_slice(&m.size.to_le_bytes());
        a.extend_from_slice(&m.atime.to_le_bytes());
        a.extend_from_slice(&m.mtime.to_le_bytes());
        a.extend_from_slice(&(name.len() as u32).to_le_bytes());
        a.extend_from_slice(name);
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// Approximate heap footprint, used to cut runs at the memory budget.
    pub fn mem_bytes(&self) -> usize {
        self.arena.len() + self.index.len() * std::mem::size_of::<(i64, usize)>()
    }

    /// Sort by (folder_id, name) and write the run to `path`.
    pub fn sort_and_write(mut self, path: &Path) -> Result<u64> {
        let arena = &self.arena;
        let name_at = |off: usize| {
            let len = u32::from_le_bytes(arena[off + HDR - 4..off + HDR].try_into().unwrap());
            &arena[off + HDR..off + HDR + len as usize]
        };
        self.index
            .sort_by(|a, b| a.0.cmp(&b.0).then_with(|| name_at(a.1).cmp(name_at(b.1))));

        let file = File::create(path).with_context(|| format!("creating run {}", path.display()))?;
        let mut enc = zstd::stream::write::Encoder::new(BufWriter::with_capacity(IO_BUF, file), ZSTD_LEVEL)?;
        for &(_, off) in &self.index {
            let end = off + HDR + name_at(off).len();
            enc.write_all(&arena[off..end])?;
        }
        enc.finish()?.flush()?;
        Ok(self.index.len() as u64)
    }
}

struct RunReader {
    dec: zstd::stream::read::Decoder<'static, BufReader<File>>,
}

impl RunReader {
    fn open(path: &Path) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("opening run {}", path.display()))?;
        let dec = zstd::stream::read::Decoder::with_buffer(BufReader::with_capacity(IO_BUF, file))?;
        Ok(Self { dec })
    }

    /// Read the next record into `rec` (reusing its name buffer). Ok(false)
    /// on a clean end of run; a partial record is an error.
    fn next(&mut self, rec: &mut FileRec) -> Result<bool> {
        let mut h = [0u8; HDR];
        let got = read_full(&mut self.dec, &mut h)?;
        if got == 0 {
            return Ok(false);
        }
        if got < HDR {
            bail!("truncated sort run (header)");
        }
        let le8 = |r: std::ops::Range<usize>| -> [u8; 8] { h[r].try_into().unwrap() };
        rec.folder_id = i64::from_le_bytes(le8(0..8));
        rec.meta = FileMeta {
            user_id: u32::from_le_bytes(h[8..12].try_into().unwrap()),
            age: h[12],
            size: u64::from_le_bytes(le8(13..21)),
            atime: i64::from_le_bytes(le8(21..29)),
            mtime: i64::from_le_bytes(le8(29..37)),
        };
        let len = u32::from_le_bytes(h[37..41].try_into().unwrap()) as usize;
        rec.name.resize(len, 0);
        self.dec.read_exact(&mut rec.name).context("truncated sort run (name)")?;
        Ok(true)
    }
}

fn read_full(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// Heap entry ordered as a min-heap on (folder_id, name, run).
struct Head {
    rec: FileRec,
    run: usize,
}

impl Head {
    fn key(&self) -> (i64, &[u8], usize) {
        (self.rec.folder_id, &self.rec.name, self.run)
    }
}

impl Ord for Head {
    fn cmp(&self, other: &Self) -> Ordering {
        other.key().cmp(&self.key())
    }
}
impl PartialOrd for Head {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl PartialEq for Head {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}
impl Eq for Head {}

/// K-way merge of sorted runs. `emit` sees each distinct (folder_id, name)
/// once, in key order; returns the number of duplicate keys dropped.
pub fn merge_runs<F: FnMut(&FileRec) -> Result<()>>(runs: &[PathBuf], mut emit: F) -> Result<u64> {
    let mut readers = runs.iter().map(|p| RunReader::open(p)).collect::<Result<Vec<_>>>()?;
    let mut heap = BinaryHeap::with_capacity(readers.len());
    for (run, r) in readers.iter_mut().enumerate() {
        let mut rec = FileRec::default();
        if r.next(&mut rec)? {
            heap.push(Head { rec, run });
        }
    }

    let mut duplicates = 0u64;
    let mut last: Option<(i64, Vec<u8>)> = None;
    while let Some(mut head) = heap.pop() {
        let dup = matches!(&last, Some((f, n)) if *f == head.rec.folder_id && *n == head.rec.name);
        if dup {
            duplicates += 1;
        } else {
            emit(&head.rec)?;
            let (f, n) = last.get_or_insert_with(|| (0, Vec::new()));
            *f = head.rec.folder_id;
            n.clear();
            n.extend_from_slice(&head.rec.name);
        }
        if readers[head.run].next(&mut head.rec)? {
            heap.push(head);
        }
    }
    Ok(duplicates)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(folder_id: i64, name: &str, size: u64) -> FileRec {
        let meta = FileMeta { user_id: 7, age: 1, size, atime: -5, mtime: 9 };
        FileRec { folder_id, name: name.into(), meta }
    }

    fn write_run(dir: &Path, i: usize, recs: &[FileRec]) -> PathBuf {
        let mut b = RunBuffer::default();
        for r in recs {
            b.push(r.folder_id, &r.name, r.meta);
        }
        let p = dir.join(format!("run{i}"));
        assert_eq!(b.sort_and_write(&p).unwrap(), recs.len() as u64);
        p
    }

    fn merged(runs: &[PathBuf]) -> (Vec<FileRec>, u64) {
        let mut out = Vec::new();
        let dups = merge_runs(runs, |r| {
            out.push(r.clone());
            Ok(())
        })
        .unwrap();
        (out, dups)
    }

    #[test]
    fn sorts_within_and_across_runs_by_folder_then_name_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let a = write_run(dir.path(), 0, &[rec(10, "b", 1), rec(2, "z", 2), rec(10, "a", 3)]);
        let b = write_run(dir.path(), 1, &[rec(3, "m", 4), rec(10, "B", 5), rec(2, "y", 6)]);
        let (out, dups) = merged(&[a, b]);
        let keys: Vec<(i64, String)> = out
            .iter()
            .map(|r| (r.folder_id, String::from_utf8(r.name.clone()).unwrap()))
            .collect();
        // Folder ids numeric, names in byte order ("B" < "a") like SQLite BINARY.
        let want = [(2, "y"), (2, "z"), (3, "m"), (10, "B"), (10, "a"), (10, "b")];
        assert_eq!(keys, want.map(|(f, n)| (f, n.to_string())));
        assert_eq!(dups, 0);
        // Payload survives the round trip.
        assert_eq!(out[0], rec(2, "y", 6));
    }

    #[test]
    fn duplicates_keep_first_occurrence_in_input_order() {
        let dir = tempfile::tempdir().unwrap();
        let a = write_run(dir.path(), 0, &[rec(1, "dup", 100), rec(1, "dup", 101)]);
        let b = write_run(dir.path(), 1, &[rec(1, "dup", 200)]);
        let (out, dups) = merged(&[a, b]);
        assert_eq!(out, vec![rec(1, "dup", 100)]);
        assert_eq!(dups, 2);
    }

    #[test]
    fn empty_and_utf8_names_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let a = write_run(dir.path(), 0, &[rec(4, "ñandú \u{FFFD}.txt", 1)]);
        let b = write_run(dir.path(), 1, &[]);
        let (out, _) = merged(&[a, b]);
        assert_eq!(out, vec![rec(4, "ñandú \u{FFFD}.txt", 1)]);
    }
}
