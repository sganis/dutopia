// src/bin/dudb/ingest_raw.rs
//
// Second ingest pass: stream the raw duscan CSV
// (INODE,ATIME,MTIME,UID,GID,MODE,SIZE,DISK,PATH) into the per-file `files`
// table so duapi can serve /api/files from the DB instead of a live `ls`.
// Only regular files are kept — directories already live in `paths`, and the
// live-filesystem fallback (`item::get_items`) skips non-files too.
use anyhow::{Context, Result};
use csv::ReaderBuilder;
use dutopia::util::{age_bucket, parse_int, sanitize_mtime, username_from_uid, AgeCfg};
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::path::Path;

// POSIX file-type masks as encoded by duscan in MODE (same on all platforms).
const S_IFMT: u32 = 0o170000;
const S_IFREG: u32 = 0o100000;

/// Commit granularity: keeps the WAL bounded and progress durable on
/// billion-row ingests without per-row transaction overhead.
const COMMIT_EVERY: u64 = 1_000_000;

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

pub fn ingest_raw_csv<F: FnMut(u64)>(
    conn: &Connection,
    csv_path: &Path,
    path_cache: &HashMap<String, i64>,
    age_cfg: AgeCfg,
    total_data_lines: usize,
    mut on_progress: F,
) -> Result<RawIngestStats> {
    let mut stats = RawIngestStats::default();
    // Ownership must resolve exactly like dusum's folder pass did, so run
    // dudb on a host in the same identity domain (normally the same host,
    // right after dusum in the pipeline).
    let mut uid_cache: HashMap<u32, String> = HashMap::new();
    let mut user_cache: HashMap<String, i64> = load_users(conn)?;

    let mut insert_user = conn.prepare("INSERT INTO users(name) VALUES(?1) RETURNING id")?;
    let mut insert_file = conn.prepare(
        "INSERT OR IGNORE INTO files(folder_id, name, user_id, age, size, atime, mtime)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;

    let mut rdr = ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_path(csv_path)
        .with_context(|| format!("opening raw CSV {}", csv_path.display()))?;

    let progress_step = if total_data_lines >= 100 {
        total_data_lines / 100
    } else {
        0
    };
    // The `age` column is frozen relative to ingest time. dusum bucketed with
    // its own `now` minutes earlier in the same pipeline run, so drift is
    // limited to files whose mtime sits within that gap of a cutoff.
    let now_ts = chrono::Utc::now().timestamp();

    conn.execute_batch("BEGIN IMMEDIATE;")?;
    for (lineno, rec) in rdr.byte_records().enumerate() {
        let rec = match rec {
            Ok(r) => r,
            Err(e) => {
                eprintln!("warn: skipping malformed raw CSV row {}: {}", lineno + 2, e);
                continue;
            }
        };
        if rec.len() < 9 {
            continue;
        }
        let mode = parse_int::<u32>(rec.get(5));
        if mode & S_IFMT != S_IFREG {
            stats.skipped_non_regular += 1;
            continue;
        }
        let path_bytes = rec.get(8).unwrap_or(b"");
        let Some((folder, name)) = split_folder_name(path_bytes) else {
            stats.unmatched_folders += 1;
            continue;
        };
        let folder_id = match path_cache.get(folder.as_str()) {
            Some(&id) => id,
            None => {
                stats.unmatched_folders += 1;
                if stats.unmatched_folders <= MAX_UNMATCHED_WARNINGS {
                    eprintln!(
                        "warn: raw CSV line {}: folder {:?} not in dusum CSV; \
                         are the two files from the same scan run?",
                        lineno + 2,
                        folder
                    );
                }
                continue;
            }
        };

        let uid = parse_int::<u32>(rec.get(3));
        let user = match uid_cache.get(&uid) {
            Some(u) => u.clone(),
            None => {
                let u = username_from_uid(uid);
                uid_cache.insert(uid, u.clone());
                u
            }
        };
        let user_id = match user_cache.get(&user) {
            Some(&id) => id,
            None => {
                let id: i64 = insert_user.query_row(params![user], |r| r.get(0))?;
                user_cache.insert(user, id);
                stats.users_inserted += 1;
                id
            }
        };

        let atime = sanitize_mtime(now_ts, parse_int::<i64>(rec.get(1)));
        let mtime = sanitize_mtime(now_ts, parse_int::<i64>(rec.get(2)));
        let age = age_bucket(now_ts, mtime, age_cfg);
        let size = parse_int::<u64>(rec.get(6));

        let changed =
            insert_file.execute(params![folder_id, name, user_id, age, size, atime, mtime])?;
        if changed == 0 {
            stats.duplicates += 1;
            continue;
        }
        stats.files_inserted += 1;
        if stats.files_inserted % COMMIT_EVERY == 0 {
            conn.execute_batch("COMMIT; BEGIN IMMEDIATE;")?;
        }

        if progress_step > 0 && (lineno + 1) % progress_step == 0 {
            on_progress(lineno as u64 + 1);
        }
    }
    conn.execute_batch("COMMIT;")?;

    if stats.unmatched_folders > 0 {
        eprintln!(
            "warn: {} raw rows referenced folders missing from the dusum CSV; skipped",
            stats.unmatched_folders
        );
    }
    Ok(stats)
}

fn load_users(conn: &Connection) -> Result<HashMap<String, i64>> {
    let mut stmt = conn.prepare("SELECT name, id FROM users")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    let mut map = HashMap::new();
    for row in rows {
        let (name, id) = row?;
        map.insert(name, id);
    }
    Ok(map)
}

/// Split a raw file path into (parent folder, filename), with the folder in
/// exactly the form `dusum::aggregate::get_folder_ancestors` stores it (its
/// deepest ancestor): native separators collapsed and trimmed, drive roots
/// kept as `C:\`, invalid UTF-8 replaced with U+FFFD like dusum output.
/// Returns None when dusum would emit no ancestor for the path (no separator,
/// trailing separator, or a folder part that normalizes to nothing).
fn split_folder_name(path: &[u8]) -> Option<(String, String)> {
    let sep = separator(path);
    let pos = path.iter().rposition(|&b| b == sep)?;
    let name = &path[pos + 1..];
    if name.is_empty() {
        return None;
    }
    let folder: Vec<u8> = if pos == 0 {
        vec![sep]
    } else {
        let mut f = path[..pos].to_vec();
        while f.len() > 1 && f.last() == Some(&sep) {
            f.pop();
        }
        // Anchor prefix, then rejoin non-empty segments so repeated
        // separators collapse exactly as get_folder_ancestors does.
        let (mut current, rest): (Vec<u8>, &[u8]) = if sep == b'/' && f.starts_with(b"/") {
            (b"/".to_vec(), &f[1..])
        } else if sep == b'\\' && f.starts_with(b"\\\\") {
            (b"\\\\".to_vec(), &f[2..])
        } else if sep == b'\\' && f.len() >= 2 && f[0].is_ascii_alphabetic() && f[1] == b':' {
            let rest = if f.len() >= 3 && f[2] == b'\\' {
                &f[3..]
            } else {
                &f[2..]
            };
            (vec![f[0], b':', b'\\'], rest)
        } else {
            (Vec::new(), &f[..])
        };
        for segment in rest.split(|&b| b == sep).filter(|s| !s.is_empty()) {
            if !current.is_empty() && current.last() != Some(&sep) {
                current.push(sep);
            }
            current.extend_from_slice(segment);
        }
        // A bare UNC prefix ("\\") with no segments never appears in dusum
        // output; treat it as no-ancestor like get_folder_ancestors does.
        if current.is_empty() || current == b"\\\\" {
            return None;
        }
        current
    };
    Some((
        String::from_utf8_lossy(&folder).into_owned(),
        String::from_utf8_lossy(name).into_owned(),
    ))
}

/// Must match `dusum::aggregate::separator`: a backslash anywhere (or a drive
/// prefix) means Windows-native; otherwise Unix.
fn separator(path: &[u8]) -> u8 {
    if path.contains(&b'\\') {
        return b'\\';
    }
    if path.len() >= 2 && path[0].is_ascii_alphabetic() && path[1] == b':' {
        return b'\\';
    }
    b'/'
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ingest, schema};
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

    fn ingest_both(sum: &NamedTempFile, raw: &NamedTempFile) -> (Connection, RawIngestStats) {
        let mut c = fresh_conn();
        let (_, path_cache) = ingest::ingest_csv(&mut c, sum.path(), 3, |_| {}).unwrap();
        let stats = ingest_raw_csv(
            &c,
            raw.path(),
            &path_cache,
            AgeCfg::default(),
            0,
            |_| {},
        )
        .unwrap();
        (c, stats)
    }

    fn count(c: &Connection, sql: &str) -> i64 {
        c.query_row(sql, [], |r| r.get(0)).unwrap()
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
        assert_eq!(s.files_inserted, 2);
        assert_eq!(s.skipped_non_regular, 1);
        assert_eq!(s.unmatched_folders, 0);
        let n: i64 = count(
            &c,
            "SELECT COUNT(*) FROM files f
             JOIN paths p ON p.id = f.folder_id WHERE p.full_path = '/a'",
        );
        assert_eq!(n, 1);
        let name: String = c
            .query_row(
                "SELECT f.name FROM files f
                 JOIN paths p ON p.id = f.folder_id WHERE p.full_path = '/a/b'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(name, "y.txt");
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
        assert_eq!(s.files_inserted, 3);
        let age_of = |name: &str| -> i64 {
            c.query_row(
                "SELECT age FROM files WHERE name = ?1",
                params![name],
                |r| r.get(0),
            )
            .unwrap()
        };
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
        assert_eq!(s.files_inserted, 1);
        assert_eq!(s.unmatched_folders, 2); // /nope + relative
        assert_eq!(s.duplicates, 1);
        assert_eq!(count(&c, "SELECT COUNT(*) FROM files"), 1);
    }

    #[test]
    fn new_uid_creates_user_row() {
        let now = chrono::Utc::now().timestamp();
        let sum = sum_csv();
        // uid 0 resolves to root (unix) or USERNAME (windows) — either way a
        // name that is not in the dusum fixture (alice only).
        let raw = raw_csv(&[format!("4-1,{now},{now},0,0,{REG},1,1,/a/rooted")]);
        let (c, s) = ingest_both(&sum, &raw);
        assert_eq!(s.files_inserted, 1);
        assert_eq!(s.users_inserted, 1);
        assert!(count(&c, "SELECT COUNT(*) FROM users") >= 2);
    }

    #[test]
    fn split_folder_name_native_forms() {
        let ok = |p: &[u8]| split_folder_name(p).unwrap();
        assert_eq!(ok(b"/f.txt"), ("/".into(), "f.txt".into()));
        assert_eq!(ok(b"/a/b/f.txt"), ("/a/b".into(), "f.txt".into()));
        // Repeated separators collapse, matching get_folder_ancestors.
        assert_eq!(ok(b"/a//b//f.txt"), ("/a/b".into(), "f.txt".into()));
        assert_eq!(ok(b"C:\\f.txt"), ("C:\\".into(), "f.txt".into()));
        assert_eq!(
            ok(b"C:\\Users\\San\\f.txt"),
            ("C:\\Users\\San".into(), "f.txt".into())
        );
        assert_eq!(
            ok(b"\\\\srv\\shr\\f.txt"),
            ("\\\\srv\\shr".into(), "f.txt".into())
        );
        assert_eq!(split_folder_name(b"relative.txt"), None);
        assert_eq!(split_folder_name(b"/a/trailing/"), None);
    }

    #[test]
    fn split_folder_name_lossy_utf8_matches_dusum() {
        // dusum writes folder paths through from_utf8_lossy; the raw pass must
        // produce the same lossy string or the folder lookup misses.
        let raw = [b'/', 0xFFu8, b'a', b'/', b'f'];
        let (folder, name) = split_folder_name(&raw).unwrap();
        assert_eq!(folder, String::from_utf8_lossy(&[b'/', 0xFFu8, b'a']));
        assert_eq!(name, "f");
    }
}
