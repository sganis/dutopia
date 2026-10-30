// src/bin/dudb/ingest_raw_tests.rs
use super::*;
use crate::{ingest, schema};
use crate::folder_pack::ListRule;
use dutopia::filepack::{unpack, PackedFile};
use std::io::Write;
use tempfile::NamedTempFile;

const REG: u32 = 0o100644; // regular file rw-r--r--
const DIR: u32 = 0o040755; // directory

fn fresh_conn() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    schema::apply_ingest_pragmas(&c).unwrap();
    schema::create_tables(&c).unwrap();
    c
}

/// Folder skeleton the raw rows below hang off. Mirrors what dusum would
/// emit for /a and /a/b plus roots.
fn sum_csv() -> NamedTempFile {
    let mut f = NamedTempFile::new().unwrap();
    writeln!(
        f,
        "path,user,age,files,size,disk,linked,accessed,modified\n\
         /,alice,0,3,300,300,0,1,1\n\
         /a,alice,0,2,200,200,0,1,1\n\
         /a/b,alice,0,1,100,100,0,1,1"
    )
    .unwrap();
    f
}

fn raw_csv(rows: &[String]) -> NamedTempFile {
    let mut f = NamedTempFile::new().unwrap();
    writeln!(f, "INODE,ATIME,MTIME,UID,GID,MODE,SIZE,DISK,PATH").unwrap();
    for r in rows {
        writeln!(f, "{}", r).unwrap();
    }
    f
}

/// Both passes with a tiny run budget so multi-run merging is exercised.
fn ingest_with(
    sum: &NamedTempFile,
    raw: &NamedTempFile,
    run_bytes: usize,
    lines: usize,
    ticks: &mut Vec<u64>,
) -> (Connection, RawIngestStats, usize) {
    let mut c = fresh_conn();
    let (_, path_cache) = ingest::ingest_csv(&mut c, sum.path(), 3, |_| {}).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let opts = SortOpts { tmp_dir: tmp.path().to_path_buf(), run_bytes };
    let sorted = sort_raw_csv(&c, raw.path(), &path_cache, AgeCfg::default(), &opts, lines, |p| {
        ticks.push(p)
    })
    .unwrap();
    let runs = sorted.run_count();
    let rule = ListRule { cap: 0, min_size: 0 };
    let stats = load_sorted_runs(&c, sorted, rule, |_| {}).unwrap();
    // Run files are removed once loaded.
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
    (c, stats, runs)
}

fn ingest_both(sum: &NamedTempFile, raw: &NamedTempFile) -> (Connection, RawIngestStats) {
    let (c, s, _) = ingest_with(sum, raw, 1 << 20, 0, &mut Vec::new());
    (c, s)
}

/// Files packed for one folder, largest first; empty if it has no row.
fn files_in(c: &Connection, folder: &str) -> Vec<PackedFile> {
    c.query_row(
        "SELECT ff.data FROM paths p JOIN folder_files ff ON ff.folder_id = p.id
         WHERE p.full_path = ?1",
        params![folder],
        |r| r.get::<_, Vec<u8>>(0),
    )
    .map(|blob| unpack(&blob).unwrap().files)
    .unwrap_or_default()
}

fn all_files(c: &Connection) -> Vec<PackedFile> {
    let mut stmt = c.prepare("SELECT data FROM folder_files").unwrap();
    let blobs = stmt.query_map([], |r| r.get::<_, Vec<u8>>(0)).unwrap();
    blobs.flat_map(|b| unpack(&b.unwrap()).unwrap().files).collect()
}

fn file_named(c: &Connection, name: &str) -> PackedFile {
    all_files(c).into_iter().find(|f| f.name == name).unwrap()
}

#[test]
fn regular_files_land_in_matching_folders() {
    let now = chrono::Utc::now().timestamp();
    let sum = sum_csv();
    let raw = raw_csv(&[
        format!("1-1,{now},{now},501,20,{REG},10,10,/a/x.txt"),
        format!("1-2,{now},{now},501,20,{REG},20,20,/a/b/y.txt"),
        format!("1-3,{now},{now},501,20,{DIR},0,0,/a/b"),
    ]);
    let (c, s) = ingest_both(&sum, &raw);
    assert_eq!((s.files_seen, s.files_listed, s.folders), (2, 2, 2));
    assert_eq!(s.skipped_non_regular, 1);
    assert_eq!(s.unmatched_folders, 0);
    let a = files_in(&c, "/a");
    assert_eq!(a.len(), 1);
    assert_eq!((a[0].name.as_str(), a[0].size), ("x.txt", 10));
    let b = files_in(&c, "/a/b");
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].name, "y.txt");
    // Folders without files get no row.
    assert!(files_in(&c, "/").is_empty());
}

#[test]
fn age_buckets_frozen_from_mtime() {
    let now = chrono::Utc::now().timestamp();
    let recent = now - 10 * 86_400;
    let mid = now - 100 * 86_400;
    let old = now - 700 * 86_400;
    let sum = sum_csv();
    let raw = raw_csv(&[
        format!("2-1,{now},{recent},501,20,{REG},1,1,/a/recent"),
        format!("2-2,{now},{mid},501,20,{REG},1,1,/a/mid"),
        format!("2-3,{now},{old},501,20,{REG},1,1,/a/old"),
    ]);
    let (c, s) = ingest_both(&sum, &raw);
    assert_eq!(s.files_listed, 3);
    let age_of = |name: &str| file_named(&c, name).age;
    assert_eq!(age_of("recent"), 0);
    assert_eq!(age_of("mid"), 1);
    assert_eq!(age_of("old"), 2);
}

#[test]
fn unmatched_folder_and_duplicates_counted() {
    let now = chrono::Utc::now().timestamp();
    let sum = sum_csv();
    let raw = raw_csv(&[
        format!("3-1,{now},{now},501,20,{REG},1,1,/nope/z.txt"),
        format!("3-2,{now},{now},501,20,{REG},1,1,/a/dup.txt"),
        format!("3-3,{now},{now},501,20,{REG},2,2,/a/dup.txt"),
        format!("3-4,{now},{now},501,20,{REG},1,1,relative.txt"),
    ]);
    let (c, s) = ingest_both(&sum, &raw);
    assert_eq!(s.files_listed, 1);
    assert_eq!(s.unmatched_folders, 2); // /nope + relative
    assert_eq!(s.duplicates, 1);
    assert_eq!(all_files(&c).len(), 1);
    // First occurrence in CSV order wins.
    assert_eq!(file_named(&c, "dup.txt").size, 1);
}

#[test]
fn new_uid_creates_user_row() {
    let now = chrono::Utc::now().timestamp();
    let sum = sum_csv();
    // uid 0 resolves to root (unix) or USERNAME (windows) — either way a
    // name that is not in the dusum fixture (alice only).
    let raw = raw_csv(&[format!("4-1,{now},{now},0,0,{REG},1,1,/a/rooted")]);
    let (c, s) = ingest_both(&sum, &raw);
    assert_eq!(s.files_listed, 1);
    assert_eq!(s.users_inserted, 1);
    let users: i64 = c.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0)).unwrap();
    assert!(users >= 2);
}

#[test]
fn many_runs_merge_into_deduplicated_folders() {
    let now = chrono::Utc::now().timestamp();
    let sum = sum_csv();
    // Interleave folders and repeat every name once later in the file so
    // duplicates straddle run boundaries.
    let mut rows = Vec::new();
    for i in (0..300).rev() {
        let dir = if i % 2 == 0 { "/a" } else { "/a/b" };
        rows.push(format!("5-{i},{now},{now},501,20,{REG},{i},1,{dir}/f{i:04}"));
    }
    for i in 0..300 {
        let dir = if i % 2 == 0 { "/a" } else { "/a/b" };
        rows.push(format!("6-{i},{now},{now},501,20,{REG},999999,1,{dir}/f{i:04}"));
    }
    let raw = raw_csv(&rows);
    // ~2 KiB per run → dozens of runs.
    let (c, s, runs) = ingest_with(&sum, &raw, 2048, 0, &mut Vec::new());
    assert!(runs > 10, "expected many runs, got {runs}");
    assert_eq!((s.files_listed, s.folders), (300, 2));
    assert_eq!(s.duplicates, 300);
    let files = all_files(&c);
    assert_eq!(files.len(), 300);
    // Every kept file is the first occurrence (size = i, never 999999).
    assert!(files.iter().all(|f| f.size != 999999));
    // Each folder is packed largest first.
    let a = files_in(&c, "/a");
    assert!(a.windows(2).all(|w| w[0].size >= w[1].size));
}

#[test]
fn progress_reaches_total_even_when_last_rows_are_skipped() {
    let now = chrono::Utc::now().timestamp();
    let sum = sum_csv();
    let mut rows = Vec::new();
    for i in 0..150 {
        rows.push(format!("7-{i},{now},{now},501,20,{REG},1,1,/a/g{i}"));
    }
    // Tail of directories: the old loop never ticked on skipped rows.
    for i in 0..50 {
        rows.push(format!("8-{i},{now},{now},501,20,{DIR},0,0,/a/d{i}"));
    }
    let raw = raw_csv(&rows);
    let mut ticks = Vec::new();
    ingest_with(&sum, &raw, 1 << 20, 200, &mut ticks);
    assert_eq!(ticks.last(), Some(&200));
    assert_eq!(ticks.iter().filter(|&&t| t > 150).count() >= 2, true);
}
