// src/bin/duxray/stats.rs
//
// Single-pass scan and counter state. All aggregation is fixed-size arrays
// or bounded HashMaps so a 200 GB CSV runs in constant memory.

use anyhow::{Context, Result};
use memchr::{memchr, memrchr};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// One record cannot exceed this (runaway-corruption cap).
const MAX_RECORD: usize = 64 * 1024 * 1024;
/// Physical lines beyond the first a quoted record may span before the
/// reader declares it corrupt (dangling quote) and replays them.
const MAX_EXTRA_LINES: usize = 8;

// POSIX file-type masks as encoded by duscan in MODE (same on all platforms).
const S_IFMT: u32 = 0o170000;
const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;

const DAY: i64 = 86_400;
const MB: u64 = 1024 * 1024;

// Size band edges (bytes). Band i covers [EDGES[i], EDGES[i+1]).
pub const BAND_EDGES: [u64; 10] = [
    0,
    1024,
    10 * 1024,
    100 * 1024,
    MB,
    4 * MB,
    16 * MB,
    64 * MB,
    256 * MB,
    1024 * MB,
];
pub const NBAND: usize = BAND_EDGES.len(); // last band is ">top edge"

// Cumulative "< threshold" thresholds.
pub const THRESHOLDS: [(&str, u64); 11] = [
    ("1K", 1024),
    ("10K", 10 * 1024),
    ("100K", 100 * 1024),
    ("1M", MB),
    ("2M", 2 * MB),
    ("5M", 5 * MB),
    ("10M", 10 * MB),
    ("20M", 20 * MB),
    ("50M", 50 * MB),
    ("100M", 100 * MB),
    ("1G", 1024 * MB),
];

// Age buckets by mtime/atime (dusum convention): 0=<7d 1=7-60d 2=60-600d 3=>600d
pub const AGE_EDGES: [i64; 3] = [7 * DAY, 60 * DAY, 600 * DAY];
pub const AGE_LABELS: [&str; 4] = ["<7d", "7-60d", "60-600d", ">600d"];

// Abandonment buckets by last-touched = max(atime, mtime):
// 0=<1y 1=1-2y 2=2-5y 3=>5y
pub const ABANDON_EDGES: [i64; 3] = [365 * DAY, 2 * 365 * DAY, 5 * 365 * DAY];
pub const ABANDON_LABELS: [&str; 4] = ["<1y", "1-2y", "2-5y", ">5y"];

#[inline]
pub fn age_bucket(now: i64, t: i64) -> usize {
    let age = now.saturating_sub(t);
    if age < AGE_EDGES[0] {
        0
    } else if age < AGE_EDGES[1] {
        1
    } else if age < AGE_EDGES[2] {
        2
    } else {
        3
    }
}

#[inline]
pub fn abandon_bucket(now: i64, t: i64) -> usize {
    let age = now.saturating_sub(t);
    if age < ABANDON_EDGES[0] {
        0
    } else if age < ABANDON_EDGES[1] {
        1
    } else if age < ABANDON_EDGES[2] {
        2
    } else {
        3
    }
}

#[inline]
fn band_of(size: u64) -> usize {
    for (i, &edge) in BAND_EDGES.iter().enumerate().skip(1) {
        if size < edge {
            return i - 1;
        }
    }
    NBAND - 1
}

#[derive(Default)]
pub struct OwnerAgg {
    pub files: u64,
    pub bytes: u64,
    pub abandon_files: u64,
    pub abandon_bytes: u64,
}

#[derive(Default)]
pub struct ProjAgg {
    pub files: u64,
    pub bytes: u64,
    pub abandon_files: u64,
    pub abandon_bytes: u64,
}

/// Hardlink dedup accounting. Inode fields come in two shapes: plain POSIX
/// inodes (`12345`) and GPFS `fsid-inode` pairs (`157-1986374`), so the
/// dedup key is the full (fsid, inode) pair — parsing just the leading
/// number would collapse a whole filesystem onto one id. Inodes are still
/// only unique per filesystem, and a combined CSV spans many, so `vols`
/// holds one key set per volume (the first two path components, e.g.
/// `red/cmt3`). Unique size/disk counts the first sighting of a
/// (volume, key) pair; duplicates fall out of the difference with the
/// per-file totals.
#[derive(Default)]
pub struct Hardlinks {
    vols: Vec<HashSet<(u64, u64)>>,
    pub unique_files: u64,
    pub unique_size: u64,
    pub unique_disk: u64,
    pub linked_files: u64,
    pub linked_size: u64,
    pub linked_disk: u64,
}

impl Hardlinks {
    /// Number of distinct volumes seen (for the report).
    pub fn volumes(&self) -> usize {
        self.vols.len()
    }

    /// Record a regular-file row; returns true the first time its inode is
    /// seen within its volume (i.e. the row counts towards unique data).
    #[inline]
    pub fn record(&mut self, vol: u32, inode: (u64, u64), size: u64, disk: u64) -> bool {
        if self.vols.len() <= vol as usize {
            self.vols.resize_with(vol as usize + 1, HashSet::default);
        }
        if self.vols[vol as usize].insert(inode) {
            self.unique_files += 1;
            self.unique_size += size;
            self.unique_disk += disk;
            true
        } else {
            self.linked_files += 1;
            self.linked_size += size;
            self.linked_disk += disk;
            false
        }
    }
}

/// Parse an INODE field: `fsid-inode` (GPFS) or plain (POSIX). None if
/// malformed. A plain inode maps to key (0, n).
#[inline]
fn parse_inode_key(b: &[u8]) -> Option<(u64, u64)> {
    match memchr(b'-', b) {
        Some(i) => {
            let fsid = atoi::atoi::<u64>(&b[..i])?;
            let ino = atoi::atoi::<u64>(&b[i + 1..])?;
            Some((fsid, ino))
        }
        None => atoi::atoi::<u64>(b).map(|n| (0, n)),
    }
}

#[derive(Default)]
pub struct Stats {
    pub files: u64,
    pub dirs: u64,
    pub other: u64,
    pub empty: u64,
    pub parse_errors: u64,
    pub total_size: u64,
    pub total_disk: u64,
    pub largest: (String, u64),

    pub band_files: [u64; NBAND],
    pub band_size: [u64; NBAND],
    pub band_disk: [u64; NBAND],
    pub thr_lt: [u64; THRESHOLDS.len()],
    pub thr_size: [u64; THRESHOLDS.len()],

    pub mtime_bucket: [u64; 4],
    pub atime_bucket: [u64; 4],
    pub mtime_bytes: [u64; 4],
    pub atime_bytes: [u64; 4],
    pub touched_bucket: [u64; 4],
    pub touched_bytes: [u64; 4],

    pub never_read: u64,
    pub never_read_size: u64,
    pub write_once: u64,
    pub atime_eq_mtime: u64,

    pub abandon_files: [u64; 4],
    pub abandon_size: [u64; 4],

    pub sparse: u64,
    pub sparse_saved: u64,
    pub hardlinks: Option<Hardlinks>,
    pub junk: u64,
    pub junk_size: u64,
    pub world_writable: u64,
    pub world_writable_size: u64,
    pub execs: u64,
    pub no_perm: u64,
    pub future_mtime: u64,
    pub epoch_mtime: u64,
    pub min_mtime: Option<i64>,
    pub max_mtime: Option<i64>,
    pub min_atime: Option<i64>,
    pub max_atime: Option<i64>,

    pub depth_hist: [u64; 32],
    pub max_depth: usize,
    pub long_paths: u64,
    pub max_path_len: usize,
    pub longest_path: String,

    pub owners: HashMap<u32, OwnerAgg>,
    pub ext_counts: HashMap<String, (u64, u64)>, // ext -> (files, bytes)
    pub projects: HashMap<String, ProjAgg>,
}

/// Parse a signed decimal int from bytes, None on malformed/empty.
#[inline]
fn parse_i64(b: &[u8]) -> Option<i64> {
    if b.is_empty() {
        return None;
    }
    atoi::atoi::<i64>(b)
}

/// Parse a u32 uid; None on malformed.
#[inline]
fn parse_u32(b: &[u8]) -> Option<u32> {
    atoi::atoi::<u32>(b)
}

/// True if the line needs RFC-4180 unquoting (contains a double quote).
#[inline]
fn needs_unquote(line: &[u8]) -> bool {
    line.contains(&b'"')
}

/// Minimal CSV field split: unquoted plain split on commas, quoted fields
/// honored per duscan's smart-quoting (quotes only when the path contains
/// `"` `,` `\n` or `\r`).
fn split_line<'a>(line: &'a [u8], out: &mut Vec<&'a [u8]>) {
    out.clear();
    if !needs_unquote(line) {
        let mut rest = line;
        while let Some(i) = memchr(b',', rest) {
            out.push(&rest[..i]);
            rest = &rest[i + 1..];
        }
        out.push(rest);
        return;
    }
    let mut in_q = false;
    let mut start = 0;
    let mut i = 0;
    while i < line.len() {
        match line[i] {
            b'"' => {
                if in_q && i + 1 < line.len() && line[i + 1] == b'"' {
                    i += 1; // escaped quote, stays inside field
                } else {
                    in_q = !in_q;
                }
            }
            b',' if !in_q => {
                out.push(&line[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    out.push(&line[start..]);
}

/// Strip one layer of smart quotes from a field if present.
#[inline]
fn unquote_field(f: &[u8]) -> &[u8] {
    if f.len() >= 2 && f[0] == b'"' && f[f.len() - 1] == b'"' {
        &f[1..f.len() - 1]
    } else {
        f
    }
}

pub fn scan(
    path: &Path,
    now: i64,
    limit: u64,
    progress: u64,
    hardlinks: bool,
) -> Result<(Stats, bool)> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut reader = BufReader::with_capacity(8 * 1024 * 1024, file);

    let mut header = String::new();
    reader.read_line(&mut header)?;
    let header_ok = header.trim_end_matches(['\r', '\n'])
        == "INODE,ATIME,MTIME,UID,GID,MODE,SIZE,DISK,PATH";

    let mut s = Stats::default();
    if hardlinks {
        s.hardlinks = Some(Hardlinks::default());
    }
    let mut raw: Vec<u8> = Vec::with_capacity(4096);
    let mut pending: VecDeque<Vec<u8>> = VecDeque::new();
    let mut vol_ids: HashMap<Vec<u8>, u32> = HashMap::new();
    // Scratch buffer for lowercased extensions (avoids per-file allocation).
    let mut ext_buf: Vec<u8> = Vec::with_capacity(17);

    loop {
        // Read a full record: paths may contain embedded newlines inside
        // quotes (duscan smart-quoting). Corrupted rows are isolated to a
        // single line instead of absorbing their neighbors (see read_record).
        let n = read_record(&mut reader, &mut raw, &mut pending)?;
        if n == 0 {
            break;
        }
        let line = trim_eol(&raw);

        if line.is_empty() {
            continue;
        }
        // `fields` borrows `raw` for the rest of the block; both are
        // re-created each iteration so no borrow crosses the clear() above.
        let mut fields: Vec<&[u8]> = Vec::with_capacity(9);
        split_line(line, &mut fields);
        if fields.len() < 9 {
            s.parse_errors += 1;
            continue;
        }

        let (atime, mtime) = match (parse_i64(fields[1]), parse_i64(fields[2])) {
            (Some(a), Some(m)) => (a, m),
            _ => {
                s.parse_errors += 1;
                continue;
            }
        };
        let inode_key = match parse_inode_key(fields[0]) {
            Some(k) => k,
            None => {
                s.parse_errors += 1;
                continue;
            }
        };
        let mode = match atoi::atoi::<u32>(fields[5]) {
            Some(m) => m,
            None => {
                s.parse_errors += 1;
                continue;
            }
        };
        let (size, disk) = match (parse_i64(fields[6]), parse_i64(fields[7])) {
            (Some(a), Some(d)) if a >= 0 && d >= 0 => (a as u64, d as u64),
            _ => {
                s.parse_errors += 1;
                continue;
            }
        };
        let path_bytes = unquote_field(fields[8]);

        let ftype = mode & S_IFMT;
        if ftype == S_IFDIR {
            s.dirs += 1;
            continue;
        }
        if ftype != S_IFREG {
            s.other += 1;
            continue;
        }

        if limit != 0 && s.files >= limit {
            break;
        }
        s.files += 1;

        // path base name and volume/project prefix, used below
        let base = match memrchr(b'/', path_bytes) {
            Some(i) => &path_bytes[i + 1..],
            None => path_bytes,
        };
        let proj = second_component(path_bytes);

        if let Some(h) = s.hardlinks.as_mut() {
            // GPFS `fsid-inode`: the fsid IS the filesystem id, so it keys
            // the volume bucket directly (mapped to a small index). Plain
            // POSIX inodes (fsid 0) fall back to the path-prefix volume.
            // The two key spaces cannot collide: path keys contain '/'.
            let vol_key: &[u8] = if inode_key.0 != 0 {
                &inode_key.0.to_be_bytes()
            } else {
                proj.unwrap_or(&[])
            };
            let vol = match vol_ids.get(vol_key) {
                Some(&v) => v,
                None => {
                    let v = vol_ids.len() as u32;
                    vol_ids.insert(vol_key.to_vec(), v);
                    v
                }
            };
            h.record(vol, inode_key, size, disk);
        }

        // ---- size accounting
        if size == 0 {
            s.empty += 1;
        }
        s.total_size += size;
        s.total_disk += disk;
        if size > s.largest.1 {
            s.largest.1 = size;
            s.largest.0.clear();
            s.largest.0.push_str(&String::from_utf8_lossy(path_bytes));
        }
        let b = band_of(size);
        s.band_files[b] += 1;
        s.band_size[b] += size;
        s.band_disk[b] += disk;
        for (i, &(_n, thr)) in THRESHOLDS.iter().enumerate() {
            if size < thr {
                s.thr_lt[i] += 1;
                s.thr_size[i] += size;
            }
        }

        // ---- sparse
        if disk < size {
            s.sparse += 1;
            s.sparse_saved += size - disk;
        }

        // ---- mtime
        let mb = age_bucket(now, mtime);
        s.mtime_bucket[mb] += 1;
        s.mtime_bytes[mb] += size;
        if s.min_mtime.is_none_or(|v| mtime < v) {
            s.min_mtime = Some(mtime);
        }
        if s.max_mtime.is_none_or(|v| mtime > v) {
            s.max_mtime = Some(mtime);
        }
        if mtime > now + DAY {
            s.future_mtime += 1;
        }
        if mtime <= 0 {
            s.epoch_mtime += 1;
        }

        // ---- atime + never-read
        let ab = age_bucket(now, atime);
        s.atime_bucket[ab] += 1;
        s.atime_bytes[ab] += size;
        if s.min_atime.is_none_or(|v| atime < v) {
            s.min_atime = Some(atime);
        }
        if s.max_atime.is_none_or(|v| atime > v) {
            s.max_atime = Some(atime);
        }
        if atime <= mtime {
            s.never_read += 1;
            s.never_read_size += size;
        }
        if atime == mtime {
            s.atime_eq_mtime += 1;
            if mb == 3 {
                s.write_once += 1;
            }
        }

        // ---- last touched = max(atime, mtime)
        let touched = atime.max(mtime);
        let tb = age_bucket(now, touched);
        s.touched_bucket[tb] += 1;
        s.touched_bytes[tb] += size;
        let aband = abandon_bucket(now, touched);
        s.abandon_files[aband] += 1;
        s.abandon_size[aband] += size;

        // ---- permissions
        if mode & 0o002 != 0 {
            s.world_writable += 1;
            s.world_writable_size += size;
        }
        if mode & 0o111 != 0 {
            s.execs += 1;
        }
        if mode & 0o777 == 0 {
            s.no_perm += 1;
        }

        // ---- junk
        if is_junk(base) {
            s.junk += 1;
            s.junk_size += size;
        }

        // ---- path stats
        let depth = path_bytes.iter().filter(|&&c| c == b'/').count();
        s.depth_hist[depth.min(31)] += 1;
        if depth > s.max_depth {
            s.max_depth = depth;
        }
        let plen = path_bytes.len();
        if plen > 255 {
            s.long_paths += 1;
        }
        if plen > s.max_path_len {
            s.max_path_len = plen;
            s.longest_path.clear();
            s.longest_path.push_str(&String::from_utf8_lossy(path_bytes));
        }

        // ---- owner aggregation
        let uid = parse_u32(fields[3]).unwrap_or(u32::MAX);
        let o = s.owners.entry(uid).or_default();
        o.files += 1;
        o.bytes += size;
        if aband >= 1 {
            o.abandon_files += 1;
            o.abandon_bytes += size;
        }

        // ---- extension
        if let Some(dot) = memrchr(b'.', base) {
            if dot > 0 && dot + 1 < base.len() {
                let ext_bytes = &base[dot + 1..];
                if ext_bytes.len() <= 16 && ext_bytes.is_ascii() {
                    // fast path: in-place lowercase into the scratch buffer,
                    // no allocation for the common (short, ascii) case
                    ext_buf.clear();
                    ext_buf.extend_from_slice(ext_bytes);
                    ext_buf.make_ascii_lowercase();
                    // entry() needs a String key: reuse one allocation
                    let key = String::from_utf8_lossy(&ext_buf).into_owned();
                    let e = s.ext_counts.entry(key).or_default();
                    e.0 += 1;
                    e.1 += size;
                } else {
                    let ext = String::from_utf8_lossy(ext_bytes).into_owned();
                    let ext = truncate(&ext, 16).to_lowercase();
                    let e = s.ext_counts.entry(ext).or_default();
                    e.0 += 1;
                    e.1 += size;
                }
            }
        }

        // ---- name / project
        // top-level project: /vol/proj... -> "vol/proj"
        // (third slash-delimited component is the volume, fourth is proj)
        if let Some(proj) = proj {
            let key = String::from_utf8_lossy(proj).into_owned();
            let p = s.projects.entry(key).or_default();
            p.files += 1;
            p.bytes += size;
            if aband >= 1 {
                p.abandon_files += 1;
                p.abandon_bytes += size;
            }
        }

        if progress != 0 && s.files % progress == 0 {
            eprintln!(
                "  ... {} files, {:.1} TiB",
                s.files,
                s.total_size as f64 / 1024f64.powi(4)
            );
        }
    }

    Ok((s, header_ok))
}

#[inline]
fn trim_eol(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    while end > 0 && (line[end - 1] == b'\n' || line[end - 1] == b'\r') {
        end -= 1;
    }
    &line[..end]
}

/// Read one full CSV record (which may span multiple physical lines when a
/// quoted path embeds newlines) into `out` (cleared first). Returns bytes
/// read; 0 at EOF.
///
/// A combined CSV can contain corrupted rows: a converter that split records
/// on newlines leaves a dangling opening quote (e.g. `...,"/path/half` with
/// the rest of the name on the next line, never re-quoted). Blind
/// quote-parity would then absorb every following row into one record —
/// the 67 MB "path" and 3M-deep directories in the first all.csv run. So:
///
/// - absorption only starts when the line opens a quoted last field (duscan
///   smart-quoting only ever quotes the trailing PATH field);
/// - absorption stops after `MAX_EXTRA_LINES` physical lines; if the record
///   still has unbalanced quotes it is corruption: the broken line is
///   yielded alone (a parse error downstream) and the absorbed lines are
///   pushed back so the following valid rows are not lost.
fn read_record<R: BufRead>(
    reader: &mut R,
    out: &mut Vec<u8>,
    pending: &mut VecDeque<Vec<u8>>,
) -> Result<usize> {
    out.clear();
    // First physical line: pushed-back lines take priority over the reader.
    if let Some(line) = pending.pop_front() {
        out.extend_from_slice(&line);
    } else if reader.read_until(b'\n', out)? == 0 {
        return Ok(0); // clean EOF
    }
    let first_len = out.len();
    if !opens_quoted_field(out) {
        return Ok(first_len);
    }

    // Rare path: multi-line quoted record (or a corrupt dangling quote).
    let mut absorbed: Vec<Vec<u8>> = Vec::new();
    let mut scratch: Vec<u8> = Vec::new();
    while !quotes_balanced(out)
        && absorbed.len() < MAX_EXTRA_LINES
        && out.len() < MAX_RECORD
    {
        let line = match pending.pop_front() {
            Some(l) => l,
            None => {
                scratch.clear();
                if reader.read_until(b'\n', &mut scratch)? == 0 {
                    break; // EOF inside an open quote
                }
                std::mem::replace(&mut scratch, Vec::new())
            }
        };
        out.extend_from_slice(&line);
        absorbed.push(line);
    }
    // Still unbalanced: the first line is a broken row. Yield it alone and
    // replay the absorbed lines as independent record candidates.
    if !quotes_balanced(out) && !absorbed.is_empty() {
        out.truncate(first_len);
        for line in absorbed.into_iter().rev() {
            pending.push_front(line);
        }
    }
    Ok(out.len())
}

/// True if a record's final field opens a quote that is not closed by the
/// end of the line — the duscan signal that the record continues.
fn opens_quoted_field(rec: &[u8]) -> bool {
    let body = &rec[..rec.len().saturating_sub(1)]; // ignore trailing \n
    let mut in_q = false;
    let mut last_field_start = 0usize;
    let mut i = 0usize;
    while i < body.len() {
        match body[i] {
            b'"' => in_q = !in_q,
            b',' if !in_q => last_field_start = i + 1,
            _ => {}
        }
        i += 1;
    }
    body.len() > last_field_start && body[last_field_start] == b'"' && in_q
}

fn quotes_balanced(rec: &[u8]) -> bool {
    let mut in_q = false;
    let mut i = 0usize;
    while i < rec.len() {
        if rec[i] == b'"' {
            if in_q && i + 1 < rec.len() && rec[i + 1] == b'"' {
                i += 1; // escaped quote
            } else {
                in_q = !in_q;
            }
        }
        i += 1;
    }
    !in_q
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Junk names: OS cruft and editor/temp backups. Case-insensitive without
/// allocating (ASCII-equivalent byte comparison).
fn is_junk(base: &[u8]) -> bool {
    #[inline]
    fn eq_ignore_case(a: &[u8], b: &[u8]) -> bool {
        a.len() == b.len()
            && a.iter()
                .zip(b)
                .all(|(x, y)| x.to_ascii_lowercase() == *y)
    }
    #[inline]
    fn ends_with_ignore_case(hay: &[u8], suf: &[u8]) -> bool {
        hay.len() >= suf.len() && eq_ignore_case(&hay[hay.len() - suf.len()..], suf)
    }
    #[inline]
    fn starts_with_ignore_case(hay: &[u8], pre: &[u8]) -> bool {
        hay.len() >= pre.len() && eq_ignore_case(&hay[..pre.len()], pre)
    }
    eq_ignore_case(base, b".ds_store")
        || eq_ignore_case(base, b"thumbs.db")
        || eq_ignore_case(base, b"desktop.ini")
        || ends_with_ignore_case(base, b".tmp")
        || ends_with_ignore_case(base, b".temp")
        || ends_with_ignore_case(base, b".bak")
        || ends_with_ignore_case(base, b".old")
        || ends_with_ignore_case(base, b".swp")
        || starts_with_ignore_case(base, b".nfs")
        || starts_with_ignore_case(base, b"~$")
}

/// Second path component pair: `/nas10/proj/x` -> Some("nas10/proj").
/// Skips the leading empty component of absolute paths.
fn second_component(path: &[u8]) -> Option<&[u8]> {
    let mut it = path.split(|&c| c == b'/');
    it.next(); // empty before leading '/'
    let vol = it.next()?;
    let proj = it.next()?;
    if vol.is_empty() || proj.is_empty() {
        return None;
    }
    let start = vol.as_ptr() as usize - path.as_ptr() as usize;
    let end = proj.as_ptr() as usize - path.as_ptr() as usize + proj.len();
    Some(&path[start..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_plain() {
        let mut out = Vec::new();
        split_line(b"1,2,3,x/y", &mut out);
        assert_eq!(out, vec![b"1".as_slice(), b"2", b"3", b"x/y"]);
    }

    #[test]
    fn test_split_quoted() {
        let mut out = Vec::new();
        split_line(b"1,2,\"a,b\",c", &mut out);
        assert_eq!(out.len(), 4);
        assert_eq!(unquote_field(out[2]), b"a,b");
    }

    #[test]
    fn test_band_of() {
        assert_eq!(band_of(0), 0);
        assert_eq!(band_of(512), 0);
        assert_eq!(band_of(1024), 1);
        assert_eq!(band_of(MB + 1), 4);
        assert_eq!(band_of(2 * 1024 * MB), NBAND - 1);
    }

    #[test]
    fn test_second_component() {
        assert_eq!(second_component(b"/nas10/proj/x"), Some(&b"nas10/proj"[..]));
        assert_eq!(second_component(b"/nas10"), None);
        assert_eq!(second_component(b"/"), None);
    }

    // ---- read_record: multi-line records and corruption isolation ----

    fn records_of(data: &[u8]) -> Vec<String> {
        let mut r = BufReader::new(data);
        let mut out = Vec::new();
        let mut pending: VecDeque<Vec<u8>> = VecDeque::new();
        let mut recs = Vec::new();
        loop {
            let n = read_record(&mut r, &mut out, &mut pending).unwrap();
            if n == 0 {
                break;
            }
            recs.push(String::from_utf8_lossy(&out).into_owned());
        }
        recs
    }

    #[test]
    fn test_read_record_plain_lines() {
        let recs = records_of(b"a,b,c\nd,e,f\n");
        assert_eq!(recs, vec!["a,b,c\n", "d,e,f\n"]);
    }

    #[test]
    fn test_read_record_multiline_quoted() {
        // path with embedded newline: properly quoted record spans 2 lines
        let data = b"1,2,3,\"/x/y\nz\"\n4,5,6,\"/plain\"\n";
        let recs = records_of(data);
        assert_eq!(recs.len(), 2);
        assert!(recs[0].contains("/x/y\nz"));
        assert!(recs[1].contains("/plain"));
    }

    #[test]
    fn test_read_record_dangling_quote_isolated() {
        // corrupted row: opening quote never closed, next row is normal.
        // The corrupt row must NOT absorb the following ones.
        let data = b"1,2,3,\"/dangling\nnext,row,normal\nand,another,row\n";
        let recs = records_of(data);
        // first record = dangling line only; the rest split as plain lines
        assert_eq!(recs.len(), 3);
        assert!(recs[0].contains("/dangling"));
        assert_eq!(recs[1], "next,row,normal\n");
        assert_eq!(recs[2], "and,another,row\n");
    }

    #[test]
    fn test_read_record_quote_inside_field() {
        // an escaped quote pair must not confuse parity
        let data = b"1,2,3,\"/a\"\"b,c\"\nnext\n";
        let recs = records_of(data);
        assert_eq!(recs.len(), 2);
        assert!(recs[0].contains("/a\"\"b,c"));
    }

    #[test]
    fn test_opens_quoted_field() {
        assert!(opens_quoted_field(b"1,2,3,\"/open\n"));
        assert!(!opens_quoted_field(b"1,2,3,\"/closed\"\n"));
        assert!(!opens_quoted_field(b"1,2,3,plain\n"));
        assert!(opens_quoted_field(b"1,\"a\",3,\"x\n"));
        // quote in a middle field, closed: not an open record
        assert!(!opens_quoted_field(b"1,\"a\",3,plain\n"));
    }

    #[test]
    fn test_quotes_balanced() {
        assert!(quotes_balanced(b"1,2,\"a\"\n"));
        assert!(!quotes_balanced(b"1,2,\"a\n"));
        assert!(quotes_balanced(b"1,2,\"a\"\"b\"\n"));
        assert!(quotes_balanced(b"no quotes"));
    }

    // ---- hardlink volume scoping ----

    #[test]
    fn test_hardlinks_per_volume() {
        let mut h = Hardlinks::default();
        // same inode on two volumes: both unique
        assert!(h.record(0, (0, 5), 100, 10));
        assert!(h.record(1, (0, 5), 100, 10));
        // same inode same volume: duplicate
        assert!(!h.record(0, (0, 5), 50, 5));
        assert_eq!(h.unique_files, 2);
        assert_eq!(h.linked_files, 1);
        assert_eq!(h.volumes(), 2);
    }

    #[test]
    fn test_parse_inode_key() {
        // GPFS fsid-inode
        assert_eq!(parse_inode_key(b"157-1986374"), Some((157, 1986374)));
        // plain POSIX inode
        assert_eq!(parse_inode_key(b"12345"), Some((0, 12345)));
        // malformed
        assert_eq!(parse_inode_key(b"abc"), None);
        assert_eq!(parse_inode_key(b"157-"), None);
        assert_eq!(parse_inode_key(b"-1986374"), None);
        assert_eq!(parse_inode_key(b""), None);
    }

    #[test]
    fn test_scan_hardlinks_gpfs_inode() {
        use std::io::Write;
        // same GPFS inode pair on two different fsids: both unique
        let mut data = String::from("INODE,ATIME,MTIME,UID,GID,MODE,SIZE,DISK,PATH\n");
        data.push_str("157-500,100,100,1,1,33204,10,10,/nas1/proj/a/f.bin\n");
        data.push_str("157-500,100,100,1,1,33204,10,10,/nas1/proj/b/g.bin\n");
        data.push_str("154-500,100,100,1,1,33204,10,10,/nas2/proj/a/f.bin\n");
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(data.as_bytes()).unwrap();
        let (s, _) = scan(f.path(), 1000, 0, 0, true).unwrap();
        let h = s.hardlinks.as_ref().unwrap();
        assert_eq!(h.unique_files, 2); // (157,500) + (154,500)
        assert_eq!(h.linked_files, 1); // second (157,500)
        assert_eq!(h.volumes(), 2); // fsid 157 and fsid 154
    }

    #[test]
    fn test_scan_hardlinks_across_volumes() {
        use std::io::Write;
        let mut data = String::from("INODE,ATIME,MTIME,UID,GID,MODE,SIZE,DISK,PATH\n");
        // two files, same inode, different volumes
        data.push_str("7,100,100,1,1,33204,10,10,/nas1/proj/a/f.bin\n");
        data.push_str("7,100,100,1,1,33204,10,10,/nas2/proj/b/f.bin\n");
        data.push_str("7,100,100,1,1,33204,10,10,/nas1/proj/c/f.bin\n");
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(data.as_bytes()).unwrap();
        let (s, header_ok) = scan(f.path(), 1000, 0, 0, true).unwrap();
        assert!(header_ok);
        assert_eq!(s.files, 3);
        let h = s.hardlinks.as_ref().unwrap();
        assert_eq!(h.unique_files, 2);
        assert_eq!(h.linked_files, 1);
        assert_eq!(h.volumes(), 2);
    }

    #[test]
    fn test_scan_dangling_quote_row() {
        use std::io::Write;
        // row 2 is a dangling-quote fragment (as produced by dumachine for
        // multi-line DDN names). It parses as one file with a truncated
        // path — bad, but far better than absorbing the following rows
        // (the 67 MB path / 3M-depth bug in the first all.csv run).
        let mut data = String::from("INODE,ATIME,MTIME,UID,GID,MODE,SIZE,DISK,PATH\n");
        data.push_str("1,100,100,1,1,33204,10,10,/nas1/proj/a/ok.bin\n");
        data.push_str("2,100,100,1,1,33204,10,10,\"/nas1/proj/b/half\n");
        data.push_str("3,100,100,1,1,33204,10,10,/nas1/proj/c/ok.bin\n");
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(data.as_bytes()).unwrap();
        let (s, header_ok) = scan(f.path(), 1000, 0, 0, false).unwrap();
        assert!(header_ok);
        // all three rows accounted for individually: no absorption
        assert_eq!(s.files, 3);
        assert_eq!(s.parse_errors, 0);
        assert!(s.max_path_len < 100);
        assert!(s.max_depth < 20);
        // the fragment kept its leading fields: size still counted
        assert_eq!(s.total_size, 30);
    }
}
