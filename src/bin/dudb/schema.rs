// rs/src/bin/dudb/schema.rs
use anyhow::Result;
use dutopia::util::AgeCfg;
use rusqlite::{params, Connection};

/// `4` replaces v3's one-row-per-file `files` table with `folder_files`: one
/// `filepack` blob per folder holding its file totals and a short list of
/// its largest files (v3 grew a 1.1B-file DB from 8 GB to 72 GB). `3` added the per-file index (`--raw`) and the
/// `has_files` flag. `2` stored paths byte-for-byte from `dusum` with a
/// synthetic `full_path = ""` root (v1 was Unix-canonical).
pub const SCHEMA_VERSION: &str = "4";

/// Pragmas tuned for bulk ingest into a scratch file that is renamed into
/// place only after a successful build, so durability is irrelevant:
/// `synchronous=OFF`, and an in-memory rollback journal instead of WAL. Every
/// page written during the build is new, and new pages are never journaled,
/// so the journal stays tiny; WAL would instead copy the whole DB through the
/// `-wal` file and back again on checkpoint. `page_size` must come first: it
/// only takes effect before the database file is initialized.
pub fn apply_ingest_pragmas(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA page_size    = 8192;
         PRAGMA journal_mode = MEMORY;
         PRAGMA synchronous  = OFF;
         PRAGMA locking_mode = EXCLUSIVE;
         PRAGMA temp_store   = MEMORY;
         PRAGMA cache_size   = -262144;
         PRAGMA foreign_keys = OFF;",
    )?;
    Ok(())
}

/// Planner statistics plus the on-disk mode duapi expects. `analysis_limit`
/// bounds ANALYZE to a sample per index: a full ANALYZE walks every row of
/// every index (a billion-row `files` scan) for statistics the duapi queries
/// don't need precisely. DELETE journal mode lets a read-only open work
/// without creating a `-shm` file next to the DB (e.g. on an NFS share).
pub fn finalize(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA analysis_limit = 1000;
         ANALYZE;
         PRAGMA journal_mode = DELETE;",
    )?;
    Ok(())
}

pub fn create_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS users (
            id   INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE
         );
         CREATE TABLE IF NOT EXISTS paths (
            id        INTEGER PRIMARY KEY,
            parent_id INTEGER,
            full_path TEXT NOT NULL UNIQUE
         );
         CREATE TABLE IF NOT EXISTS stats (
            path_id     INTEGER NOT NULL,
            user_id     INTEGER NOT NULL,
            age         INTEGER NOT NULL,
            file_count  INTEGER NOT NULL,
            file_size   INTEGER NOT NULL,
            disk_bytes  INTEGER NOT NULL,
            linked_size INTEGER NOT NULL,
            atime       INTEGER NOT NULL,
            mtime       INTEGER NOT NULL,
            PRIMARY KEY (path_id, user_id, age)
         ) WITHOUT ROWID;
         CREATE TABLE IF NOT EXISTS metadata (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
         );
         -- Regular files directly inside each folder (from the raw duscan
         -- CSV): totals per (user, age) plus the largest few, packed by
         -- dutopia::filepack. Listing a folder is one rowid lookup.
         CREATE TABLE IF NOT EXISTS folder_files (
            folder_id INTEGER PRIMARY KEY,
            data      BLOB    NOT NULL
         );",
    )?;
    Ok(())
}

pub fn create_indexes(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_paths_parent ON paths(parent_id);")?;
    Ok(())
}

pub fn write_metadata(
    conn: &Connection,
    source_csv: &str,
    source_mtime: i64,
    row_count: u64,
) -> Result<()> {
    let loader = format!("dudb {}", env!("CARGO_PKG_VERSION"));
    let now = chrono::Utc::now().timestamp();
    let pairs: [(&str, String); 6] = [
        ("schema_version", SCHEMA_VERSION.to_string()),
        ("source_csv", source_csv.to_string()),
        ("source_csv_mtime", source_mtime.to_string()),
        ("row_count", row_count.to_string()),
        ("loader_version", loader),
        ("built_at", now.to_string()),
    ];
    upsert_pairs(conn, &pairs)?;
    // Default only: keeps the "1" written by write_files_metadata regardless
    // of which metadata pass runs first.
    conn.execute(
        "INSERT INTO metadata(key, value) VALUES('has_files', '0')
         ON CONFLICT(key) DO NOTHING",
        [],
    )?;
    Ok(())
}

/// What a `--raw` ingest stored, recorded in `metadata`.
pub struct FilesMeta<'a> {
    pub raw_csv: &'a str,
    pub raw_mtime: i64,
    /// Distinct regular files, all counted in the folder totals.
    pub total: u64,
    /// Files in the per-folder lists.
    pub listed: u64,
    /// `--max-files`; 0 = no limit.
    pub cap: usize,
    /// `--min-file-size` in bytes.
    pub min_size: u64,
    pub age_cfg: AgeCfg,
}

/// Recorded after a successful `--raw` ingest. `has_files = 1` is what makes
/// duapi serve /api/files from the DB instead of the live filesystem;
/// `files_cap` bounds its page size so a listing is always the exact top-N;
/// the age cutoffs document which thresholds froze the per-file `age`.
pub fn write_files_metadata(conn: &Connection, m: &FilesMeta) -> Result<()> {
    let pairs: [(&str, String); 9] = [
        ("has_files", "1".to_string()),
        ("files_source_csv", m.raw_csv.to_string()),
        ("files_source_csv_mtime", m.raw_mtime.to_string()),
        ("files_total", m.total.to_string()),
        ("files_listed", m.listed.to_string()),
        ("files_cap", m.cap.to_string()),
        ("files_min_size", m.min_size.to_string()),
        ("age_young_days", m.age_cfg.young.to_string()),
        ("age_old_days", m.age_cfg.old.to_string()),
    ];
    upsert_pairs(conn, &pairs)
}

/// `dudb --no-files`: a folders-only DB on purpose. duapi then lists no
/// files instead of falling back to reading the live filesystem.
pub fn write_no_files_metadata(conn: &Connection) -> Result<()> {
    upsert_pairs(conn, &[("no_files", "1".to_string())])
}

fn upsert_pairs(conn: &Connection, pairs: &[(&str, String)]) -> Result<()> {
    let mut stmt = conn.prepare(
        "INSERT INTO metadata(key, value) VALUES(?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )?;
    for (k, v) in pairs {
        stmt.execute(params![k, v])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        apply_ingest_pragmas(&c).unwrap();
        create_tables(&c).unwrap();
        c
    }

    #[test]
    fn tables_created() {
        let c = fresh();
        let names: Vec<String> = c
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(names.contains(&"users".to_string()));
        assert!(names.contains(&"paths".to_string()));
        assert!(names.contains(&"stats".to_string()));
        assert!(names.contains(&"metadata".to_string()));
        assert!(names.contains(&"folder_files".to_string()));
    }

    #[test]
    fn metadata_round_trips() {
        let c = fresh();
        write_metadata(&c, "x.csv", 1234, 99).unwrap();
        let v: String = c
            .query_row(
                "SELECT value FROM metadata WHERE key='schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        let src: String = c
            .query_row(
                "SELECT value FROM metadata WHERE key='source_csv'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(src, "x.csv");
        let hf: String = c
            .query_row(
                "SELECT value FROM metadata WHERE key='has_files'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hf, "0");
    }

    #[test]
    fn files_metadata_marks_has_files() {
        let c = fresh();
        write_metadata(&c, "x.csv", 1234, 99).unwrap();
        let m = FilesMeta {
            raw_csv: "raw.csv",
            raw_mtime: 5678,
            total: 1500,
            listed: 40,
            cap: 100,
            min_size: 1_000_000,
            age_cfg: AgeCfg::default(),
        };
        write_files_metadata(&c, &m).unwrap();
        let get = |k: &str| -> String {
            c.query_row(
                "SELECT value FROM metadata WHERE key=?1",
                params![k],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(get("has_files"), "1");
        assert_eq!(get("files_source_csv"), "raw.csv");
        assert_eq!(get("files_total"), "1500");
        assert_eq!(get("files_listed"), "40");
        assert_eq!(get("files_cap"), "100");
        assert_eq!(get("files_min_size"), "1000000");
        assert_eq!(get("age_young_days"), "60");
        assert_eq!(get("age_old_days"), "600");
    }

    #[test]
    fn no_files_metadata_marks_db() {
        let c = fresh();
        write_metadata(&c, "x.csv", 1234, 99).unwrap();
        write_no_files_metadata(&c).unwrap();
        let get = |k: &str| -> String {
            c.query_row("SELECT value FROM metadata WHERE key=?1", params![k], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(get("no_files"), "1");
        assert_eq!(get("has_files"), "0");
    }

    #[test]
    fn finalize_leaves_rollback_journal_and_stats() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.db");
        let c = Connection::open(&p).unwrap();
        apply_ingest_pragmas(&c).unwrap();
        create_tables(&c).unwrap();
        c.execute("INSERT INTO paths(full_path, parent_id) VALUES('/', NULL)", [])
            .unwrap();
        create_indexes(&c).unwrap();
        finalize(&c).unwrap();
        let mode: String = c.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
        assert_eq!(mode, "delete");
        let n: i64 = c
            .query_row("SELECT COUNT(*) FROM sqlite_stat1", [], |r| r.get(0))
            .unwrap();
        assert!(n > 0);
    }

    #[test]
    fn indexes_built() {
        let c = fresh();
        create_indexes(&c).unwrap();
        let n: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_paths_parent'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }
}
