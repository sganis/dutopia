// src/bin/dudb/ingest_raw.rs
//
// Second ingest pass: the raw duscan CSV
// (INODE,ATIME,MTIME,UID,GID,MODE,SIZE,DISK,PATH) becomes the per-file `files`
// table so duapi can serve /api/files from the DB instead of a live `ls`.
// Only regular files are kept — directories already live in `paths`, and the
// live-filesystem fallback (`item::get_items`) skips non-files too.
//
// Two steps keep a billion-row ingest linear (see extsort.rs for why):
//   1. sort_raw_csv     — parse, resolve folder/user, spill sorted runs
//   2. load_sorted_runs — merge the runs into `files` in primary-key order
use crate::extsort::{merge_runs, FileMeta, RunBuffer};
use crate::raw_path::split_folder_name;
use anyhow::{anyhow, Context, Result};
use csv::{ByteRecord, ReaderBuilder};
use dutopia::util::{age_bucket, parse_int, sanitize_mtime, username_from_uid, AgeCfg};
use rusqlite::types::{ToSqlOutput, ValueRef};
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

// POSIX file-type masks as encoded by duscan in MODE (same on all platforms).
const S_IFMT: u32 = 0o170000;
const S_IFREG: u32 = 0o100000;

/// Cap on per-row warnings so a systematically mismatched CSV pair doesn't
/// flood stderr; the total lands in `RawIngestStats.unmatched_folders`.
const MAX_UNMATCHED_WARNINGS: u64 = 20;

#[derive(Default, Debug)]
pub struct RawIngestStats {
    pub files_inserted: u64,
    pub users_inserted: u64,
    pub skipped_non_regular: u64,
    pub unmatched_folders: u64,
    pub duplicates: u64,
}

pub struct SortOpts {
    /// Parent directory for the run files (a private subdirectory is created).
    pub tmp_dir: PathBuf,
    /// Memory for one in-memory run; two are alive at once (fill + sort).
    pub run_bytes: usize,
}

/// Output of pass 1. Dropping it deletes the run files.
pub struct SortedRuns {
    dir: TempDir,
    runs: Vec<PathBuf>,
    rows: u64,
    stats: RawIngestStats,
}

impl SortedRuns {
    pub fn rows(&self) -> u64 {
        self.rows
    }
    pub fn run_count(&self) -> usize {
        self.runs.len()
    }
    pub fn dir(&self) -> &Path {
        self.dir.path()
    }
    pub fn disk_bytes(&self) -> u64 {
        self.runs.iter().filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len()).sum()
    }
}

/// Pass 1: stream the raw CSV into (folder_id, name)-sorted runs. Parsing
/// stays on this thread; a helper thread sorts and compresses the previous
/// run meanwhile. Users first seen here are inserted into `users`.
pub fn sort_raw_csv<F: FnMut(u64)>(
    conn: &Connection,
    csv_path: &Path,
    path_cache: &HashMap<String, i64>,
    age_cfg: AgeCfg,
    opts: &SortOpts,
    total_data_lines: usize,
    mut on_progress: F,
) -> Result<SortedRuns> {
    let mut stats = RawIngestStats::default();
    // Ownership must resolve exactly like dusum's folder pass did, so run
    // dudb on a host in the same identity domain (normally the same host,
    // right after dusum in the pipeline).
    let mut users = UserResolver::new(conn)?;

    let dir = tempfile::Builder::new()
        .prefix("dudb-sort-")
        .tempdir_in(&opts.tmp_dir)
        .with_context(|| format!("creating sort dir in {}", opts.tmp_dir.display()))?;
    let (tx, rx) = crossbeam::channel::bounded::<(RunBuffer, PathBuf)>(0);
    let sorter = std::thread::spawn(move || -> Result<u64> {
        let mut rows = 0;
        for (buf, path) in rx {
            rows += buf.sort_and_write(&path)?;
        }
        Ok(rows)
    });

    let mut rdr = ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .buffer_capacity(1 << 20)
        .from_path(csv_path)
        .with_context(|| format!("opening raw CSV {}", csv_path.display()))?;

    let progress_step = (total_data_lines / 100) as u64;
    // The `age` column is frozen relative to ingest time. dusum bucketed with
    // its own `now` minutes earlier in the same pipeline run, so drift is
    // limited to files whose mtime sits within that gap of a cutoff.
    let now_ts = chrono::Utc::now().timestamp();

    let mut runs: Vec<PathBuf> = Vec::new();
    let mut buf = RunBuffer::with_capacity(opts.run_bytes);
    let mut rec = ByteRecord::new();
    let mut lineno: u64 = 0;
    loop {
        match rdr.read_byte_record(&mut rec) {
            Ok(true) => {}
            Ok(false) => break,
            // A persistent read error would otherwise repeat forever.
            Err(e) if e.is_io_error() => return Err(e).context("reading raw CSV"),
            Err(e) => {
                eprintln!("warn: skipping malformed raw CSV row {}: {}", lineno + 2, e);
                continue;
            }
        }
        lineno += 1;
        // Before any `continue`, so skipped rows never swallow a tick.
        if progress_step > 0 && lineno.is_multiple_of(progress_step) {
            on_progress(lineno);
        }
        if rec.len() < 9 {
            continue;
        }
        let mode = parse_int::<u32>(rec.get(5));
        if mode & S_IFMT != S_IFREG {
            stats.skipped_non_regular += 1;
            continue;
        }
        let Some((folder, name)) = split_folder_name(rec.get(8).unwrap_or(b"")) else {
            stats.unmatched_folders += 1;
            continue;
        };
        let Some(&folder_id) = path_cache.get(folder.as_ref()) else {
            stats.unmatched_folders += 1;
            if stats.unmatched_folders <= MAX_UNMATCHED_WARNINGS {
                eprintln!(
                    "warn: raw CSV line {}: folder {:?} not in dusum CSV; \
                     are the two files from the same scan run?",
                    lineno + 1,
                    folder
                );
            }
            continue;
        };

        let mtime = sanitize_mtime(now_ts, parse_int::<i64>(rec.get(2)));
        let meta = FileMeta {
            user_id: users.id_for_uid(parse_int::<u32>(rec.get(3)), &mut stats)?,
            age: age_bucket(now_ts, mtime, age_cfg),
            size: parse_int::<u64>(rec.get(6)),
            atime: sanitize_mtime(now_ts, parse_int::<i64>(rec.get(1))),
            mtime,
        };
        buf.push(folder_id, name.as_bytes(), meta);

        if buf.mem_bytes() >= opts.run_bytes {
            let full = std::mem::replace(&mut buf, RunBuffer::with_capacity(opts.run_bytes));
            hand_off(&tx, full, dir.path(), &mut runs)?;
        }
    }
    if !buf.is_empty() {
        hand_off(&tx, buf, dir.path(), &mut runs)?;
    }
    drop(tx);
    let rows = sorter
        .join()
        .map_err(|_| anyhow!("sort thread panicked"))??;
    on_progress(total_data_lines as u64);

    if stats.unmatched_folders > 0 {
        eprintln!(
            "warn: {} raw rows referenced folders missing from the dusum CSV; skipped",
            stats.unmatched_folders
        );
    }
    Ok(SortedRuns { dir, runs, rows, stats })
}

/// Queue a full buffer for the sort thread. The channel is zero-capacity, so
/// this blocks while the previous run is still being written.
fn hand_off(
    tx: &crossbeam::channel::Sender<(RunBuffer, PathBuf)>,
    buf: RunBuffer,
    dir: &Path,
    runs: &mut Vec<PathBuf>,
) -> Result<()> {
    let path = dir.join(format!("run-{:05}.zst", runs.len()));
    runs.push(path.clone());
    tx.send((buf, path))
        .map_err(|_| anyhow!("sort thread stopped early (disk full in the sort dir?)"))
}

/// Pass 2: merge the runs into `files` in primary-key order. Every insert
/// appends to the right edge of the B-tree, so throughput stays flat no
/// matter how large the table grows.
pub fn load_sorted_runs<F: FnMut(u64)>(
    conn: &Connection,
    sorted: SortedRuns,
    mut on_progress: F,
) -> Result<RawIngestStats> {
    let SortedRuns { dir, runs, rows, mut stats } = sorted;
    let progress_step = (rows / 100).max(1);
    let mut insert_file = conn.prepare(
        "INSERT OR IGNORE INTO files(folder_id, name, user_id, age, size, atime, mtime)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;

    conn.execute_batch("BEGIN;")?;
    let mut seen = 0u64;
    let mut ignored = 0u64;
    let mut inserted = 0u64;
    let dropped = merge_runs(&runs, |r| {
        let name = ToSqlOutput::Borrowed(ValueRef::Text(&r.name));
        let m = &r.meta;
        let changed = insert_file.execute(params![
            r.folder_id, name, m.user_id, m.age, m.size, m.atime, m.mtime
        ])?;
        if changed == 0 {
            ignored += 1;
        } else {
            inserted += 1;
        }
        seen += 1;
        if seen.is_multiple_of(progress_step) {
            on_progress(seen);
        }
        Ok(())
    })?;
    conn.execute_batch("COMMIT;")?;
    drop(dir);
    on_progress(rows);

    stats.files_inserted = inserted;
    stats.duplicates = dropped + ignored;
    Ok(stats)
}

/// uid → users.id, resolving names through the local account database once
/// per uid and inserting users dusum never saw.
struct UserResolver<'c> {
    by_uid: HashMap<u32, u32>,
    by_name: HashMap<String, i64>,
    insert: rusqlite::Statement<'c>,
}

impl<'c> UserResolver<'c> {
    fn new(conn: &'c Connection) -> Result<Self> {
        let mut stmt = conn.prepare("SELECT name, id FROM users")?;
        let by_name = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<Result<HashMap<_, _>, _>>()?;
        let insert = conn.prepare("INSERT INTO users(name) VALUES(?1) RETURNING id")?;
        Ok(Self { by_uid: HashMap::new(), by_name, insert })
    }

    fn id_for_uid(&mut self, uid: u32, stats: &mut RawIngestStats) -> Result<u32> {
        if let Some(&id) = self.by_uid.get(&uid) {
            return Ok(id);
        }
        let name = username_from_uid(uid);
        let id = match self.by_name.get(&name) {
            Some(&id) => id,
            None => {
                let id: i64 = self.insert.query_row(params![name], |r| r.get(0))?;
                self.by_name.insert(name, id);
                stats.users_inserted += 1;
                id
            }
        };
        let id = u32::try_from(id).context("users.id out of range")?;
        self.by_uid.insert(uid, id);
        Ok(id)
    }
}

#[cfg(test)]
#[path = "ingest_raw_tests.rs"]
mod tests;
