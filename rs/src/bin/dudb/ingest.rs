// rs/src/bin/dudb/ingest.rs
use anyhow::{Context, Result};
use csv::ReaderBuilder;
use dutopia::util::dusum_parent;
use memchr::memchr_iter;
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

#[derive(Default, Debug)]
pub struct IngestStats {
    pub rows_inserted: u64,
    pub paths_inserted: u64,
    pub users_inserted: u64,
}

/// Synthetic-root sentinel: `dusum` does not emit this path. We insert one
/// row with `full_path = ""` so that platform roots (`/`, `C:\`, `\\srv`)
/// have a single shared parent and can be enumerated by querying for
/// children of the empty path.
pub const SYNTHETIC_ROOT: &str = "";

pub fn count_lines(path: &Path) -> Result<usize> {
    let mut file =
        File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut buf = [0u8; 128 * 1024];
    let mut count = 0usize;
    let mut last: Option<u8> = None;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        count += memchr_iter(b'\n', &buf[..n]).count();
        last = Some(buf[n - 1]);
    }
    if let Some(b) = last {
        if b != b'\n' {
            count += 1;
        }
    }
    Ok(count)
}

pub fn ingest_csv<F: FnMut(u64)>(
    conn: &mut Connection,
    csv_path: &Path,
    total_data_lines: usize,
    mut on_progress: F,
) -> Result<IngestStats> {
    let mut user_cache: HashMap<String, i64> = HashMap::new();
    let mut path_cache: HashMap<String, i64> = HashMap::new();
    let mut stats = IngestStats::default();

    let tx = conn.transaction()?;
    {
        // The synthetic root has no parent and no stats. Every CSV path whose
        // dusum_parent() is None links here.
        let synth_root_id: i64 = tx.query_row(
            "INSERT INTO paths(full_path, parent_id) VALUES(?1, NULL) RETURNING id",
            params![SYNTHETIC_ROOT],
            |r| r.get(0),
        )?;
        path_cache.insert(SYNTHETIC_ROOT.to_string(), synth_root_id);
        stats.paths_inserted += 1;

        let mut insert_user =
            tx.prepare("INSERT INTO users(name) VALUES(?1) RETURNING id")?;
        let mut insert_path =
            tx.prepare("INSERT INTO paths(full_path, parent_id) VALUES(?1, ?2) RETURNING id")?;
        let mut insert_stat = tx.prepare(
            "INSERT INTO stats
              (path_id, user_id, age, file_count, file_size, disk_bytes, linked_size, atime, mtime)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )?;

        let mut rdr = ReaderBuilder::new()
            .has_headers(true)
            .flexible(true)
            .from_path(csv_path)
            .with_context(|| format!("opening CSV {}", csv_path.display()))?;

        let progress_step = if total_data_lines >= 100 {
            total_data_lines / 100
        } else {
            0
        };

        for (lineno, rec) in rdr.records().enumerate() {
            let rec = match rec {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("warn: skipping malformed CSV row {}: {}", lineno + 2, e);
                    continue;
                }
            };
            if rec.len() < 9 {
                continue;
            }
            // Path is stored byte-for-byte from dusum. No canonicalization
            // here — dusum::aggregate::get_folder_ancestors is the single
            // source of truth for the on-disk format.
            let path = rec.get(0).unwrap_or("");
            let user = rec.get(1).unwrap_or("").trim();
            if path.is_empty() || user.is_empty() {
                continue;
            }
            let age: u8 = rec.get(2).and_then(|s| s.trim().parse().ok()).unwrap_or(0);
            let file_count: u64 = rec.get(3).and_then(|s| s.trim().parse().ok()).unwrap_or(0);
            let file_size: u64 = rec.get(4).and_then(|s| s.trim().parse().ok()).unwrap_or(0);
            let disk_bytes: u64 = rec.get(5).and_then(|s| s.trim().parse().ok()).unwrap_or(0);
            let linked_size: u64 = rec.get(6).and_then(|s| s.trim().parse().ok()).unwrap_or(0);
            let atime: i64 = rec.get(7).and_then(|s| s.trim().parse().ok()).unwrap_or(0);
            let mtime: i64 = rec.get(8).and_then(|s| s.trim().parse().ok()).unwrap_or(0);

            let user_id = match user_cache.get(user) {
                Some(&id) => id,
                None => {
                    let id: i64 = insert_user.query_row(params![user], |r| r.get(0))?;
                    user_cache.insert(user.to_string(), id);
                    stats.users_inserted += 1;
                    id
                }
            };

            let path_id = match path_cache.get(path) {
                Some(&id) => id,
                None => {
                    let parent_id = match dusum_parent(path) {
                        Some(pp) => path_cache.get(&pp).copied().unwrap_or(synth_root_id),
                        None => synth_root_id,
                    };
                    let id: i64 = insert_path.query_row(params![path, parent_id], |r| r.get(0))?;
                    path_cache.insert(path.to_string(), id);
                    stats.paths_inserted += 1;
                    id
                }
            };

            let inserted = insert_stat.execute(params![
                path_id,
                user_id,
                age,
                file_count,
                file_size,
                disk_bytes,
                linked_size,
                atime,
                mtime,
            ]);
            match inserted {
                Ok(_) => stats.rows_inserted += 1,
                Err(e) => {
                    if let rusqlite::Error::SqliteFailure(err, _) = &e {
                        if err.code == rusqlite::ErrorCode::ConstraintViolation {
                            eprintln!(
                                "warn: duplicate stats row at line {} (path_id={}, user={}, age={}); skipped",
                                lineno + 2, path_id, user, age
                            );
                            continue;
                        }
                    }
                    return Err(e.into());
                }
            }

            if progress_step > 0 && (lineno + 1) % progress_step == 0 {
                on_progress(lineno as u64 + 1);
            }
        }
    }
    tx.commit()?;

    backfill_missing_parents(conn, &path_cache)?;
    Ok(stats)
}

/// Defensive pass: if the CSV was not sorted in dusum order, some paths may
/// have been inserted before their parent and end up linked to the synthetic
/// root. Re-link them to their real parent if it now exists.
fn backfill_missing_parents(
    conn: &mut Connection,
    path_cache: &HashMap<String, i64>,
) -> Result<()> {
    let synth_root_id = match path_cache.get(SYNTHETIC_ROOT) {
        Some(&id) => id,
        None => return Ok(()),
    };
    let suspects: Vec<(i64, String)> = {
        let mut stmt = conn.prepare(
            "SELECT id, full_path FROM paths
             WHERE parent_id = ?1 AND full_path != ?2",
        )?;
        let rows = stmt.query_map(params![synth_root_id, SYNTHETIC_ROOT], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let mut rebound = 0usize;
    let tx = conn.transaction()?;
    {
        let mut upd = tx.prepare("UPDATE paths SET parent_id = ?1 WHERE id = ?2")?;
        for (id, full) in &suspects {
            if let Some(pp) = dusum_parent(full) {
                if let Some(&pid) = path_cache.get(&pp) {
                    if pid != synth_root_id {
                        upd.execute(params![pid, id])?;
                        rebound += 1;
                    }
                }
            }
        }
    }
    tx.commit()?;
    if rebound > 0 {
        eprintln!("warn: re-linked {} mis-parented paths after ingest", rebound);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn fresh_conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        schema::apply_ingest_pragmas(&c).unwrap();
        schema::create_tables(&c).unwrap();
        c
    }

    fn linux_csv() -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(
            f,
            "path,user,age,files,size,disk,linked,accessed,modified\n\
             /,alice,0,2,200,100,0,1700000000,1700000100\n\
             /,bob,1,1,50,50,0,1600000000,1600000100\n\
             /docs,alice,2,3,600,300,300,1500000000,1500000050"
        )
        .unwrap();
        f
    }

    fn windows_csv() -> NamedTempFile {
        // Mimics what dusum's get_folder_ancestors produces on Windows: paths
        // stay in OS-native form with backslashes.
        let mut f = NamedTempFile::new().unwrap();
        writeln!(
            f,
            "path,user,age,files,size,disk,linked,accessed,modified\n\
             C:\\,San,0,10,1000,1000,0,1,1\n\
             C:\\Users,San,0,5,500,500,0,1,1\n\
             C:\\Users\\San,San,0,3,300,300,0,1,1\n\
             D:\\,San,0,2,200,200,0,1,1"
        )
        .unwrap();
        f
    }

    fn unc_csv() -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(
            f,
            "path,user,age,files,size,disk,linked,accessed,modified\n\
             \\\\srv,San,0,10,1000,1000,0,1,1\n\
             \\\\srv\\shr,San,0,5,500,500,0,1,1\n\
             \\\\srv\\shr\\dir,San,0,3,300,300,0,1,1"
        )
        .unwrap();
        f
    }

    fn count(c: &Connection, sql: &str) -> i64 {
        c.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn path_exists(c: &Connection, p: &str) -> bool {
        let n: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM paths WHERE full_path = ?1",
                params![p],
                |r| r.get(0),
            )
            .unwrap();
        n == 1
    }

    fn parent_of<'a>(c: &'a Connection, path: &'a str) -> Option<String> {
        c.query_row(
            "SELECT parent.full_path FROM paths child JOIN paths parent ON parent.id = child.parent_id
             WHERE child.full_path = ?1",
            params![path],
            |r| r.get(0),
        )
        .ok()
    }

    #[test]
    fn count_lines_with_and_without_trailing_newline() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "a\nb\n").unwrap();
        assert_eq!(count_lines(f.path()).unwrap(), 2);
        let mut g = NamedTempFile::new().unwrap();
        write!(g, "a\nb").unwrap();
        assert_eq!(count_lines(g.path()).unwrap(), 2);
    }

    #[test]
    fn linux_ingest_preserves_paths_verbatim() {
        let f = linux_csv();
        let mut c = fresh_conn();
        let s = ingest_csv(&mut c, f.path(), 3, |_| {}).unwrap();
        assert_eq!(s.rows_inserted, 3);
        assert_eq!(s.users_inserted, 2);
        // synth root + "/" + "/docs"
        assert_eq!(s.paths_inserted, 3);

        // Paths stored verbatim — no Unix-style mangling injected.
        assert_eq!(count(&c, "SELECT COUNT(*) FROM paths WHERE full_path = '/'"), 1);
        assert_eq!(count(&c, "SELECT COUNT(*) FROM paths WHERE full_path = '/docs'"), 1);

        // Synth root has parent NULL; "/" parents to synth root; "/docs" parents to "/".
        let synth = count(&c, "SELECT COUNT(*) FROM paths WHERE full_path = '' AND parent_id IS NULL");
        assert_eq!(synth, 1);
        assert_eq!(parent_of(&c, "/").as_deref(), Some(""));
        assert_eq!(parent_of(&c, "/docs").as_deref(), Some("/"));
    }

    #[test]
    fn windows_ingest_preserves_native_separators() {
        let f = windows_csv();
        let mut c = fresh_conn();
        let s = ingest_csv(&mut c, f.path(), 4, |_| {}).unwrap();
        assert_eq!(s.rows_inserted, 4);
        // synth + C:\ + C:\Users + C:\Users\San + D:\
        assert_eq!(s.paths_inserted, 5);

        // Paths kept exactly as dusum wrote them.
        for p in ["C:\\", "C:\\Users", "C:\\Users\\San", "D:\\"] {
            assert!(path_exists(&c, p), "missing path {:?}", p);
        }

        // Both drive roots parent to synth; nested paths chain natively.
        assert_eq!(parent_of(&c, "C:\\").as_deref(), Some(""));
        assert_eq!(parent_of(&c, "D:\\").as_deref(), Some(""));
        assert_eq!(parent_of(&c, "C:\\Users").as_deref(), Some("C:\\"));
        assert_eq!(parent_of(&c, "C:\\Users\\San").as_deref(), Some("C:\\Users"));
    }

    #[test]
    fn unc_ingest_preserves_native_form() {
        let f = unc_csv();
        let mut c = fresh_conn();
        let s = ingest_csv(&mut c, f.path(), 3, |_| {}).unwrap();
        assert_eq!(s.rows_inserted, 3);
        // synth + \\srv + \\srv\shr + \\srv\shr\dir
        assert_eq!(s.paths_inserted, 4);

        assert_eq!(parent_of(&c, "\\\\srv").as_deref(), Some(""));
        assert_eq!(parent_of(&c, "\\\\srv\\shr").as_deref(), Some("\\\\srv"));
        assert_eq!(parent_of(&c, "\\\\srv\\shr\\dir").as_deref(), Some("\\\\srv\\shr"));
    }

    #[test]
    fn ingest_skips_blank_user_or_path() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(
            f,
            "path,user,age,files,size,disk,linked,accessed,modified\n\
             ,alice,0,1,100,100,0,1,1\n\
             /a,,0,1,100,100,0,1,1\n\
             /a,alice,0,1,100,100,0,1,1"
        )
        .unwrap();
        let mut c = fresh_conn();
        let s = ingest_csv(&mut c, f.path(), 3, |_| {}).unwrap();
        assert_eq!(s.rows_inserted, 1);
    }

    #[test]
    fn backfill_recovers_unsorted_csv() {
        // child appears before parent
        let mut f = NamedTempFile::new().unwrap();
        writeln!(
            f,
            "path,user,age,files,size,disk,linked,accessed,modified\n\
             /a/b,alice,0,1,100,100,0,1,1\n\
             /a,alice,0,1,100,100,0,1,1"
        )
        .unwrap();
        let mut c = fresh_conn();
        ingest_csv(&mut c, f.path(), 2, |_| {}).unwrap();
        assert_eq!(parent_of(&c, "/a/b").as_deref(), Some("/a"));
        assert_eq!(parent_of(&c, "/a").as_deref(), Some(""));
    }
}
