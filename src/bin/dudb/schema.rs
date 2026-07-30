// rs/src/bin/dudb/schema.rs
use anyhow::Result;
use dutopia::util::AgeCfg;
use rusqlite::{params, Connection};

/// `3` adds the per-file `files` table (populated from the raw duscan CSV via
/// `--raw`) plus the `has_files` metadata flag. `2` stored paths byte-for-byte
/// from `dusum` with a synthetic `full_path = ""` root (v1 was Unix-canonical).
pub const SCHEMA_VERSION: &str = "3";

/// Pragmas tuned for bulk ingest. `synchronous=OFF` is safe here because the
/// DB is rebuildable from the source CSV. `page_size` must come first: it only
/// takes effect before the database file is initialized, and the larger pages
/// cut B-tree depth for the billion-row `files` table.
pub fn apply_ingest_pragmas(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA page_size    = 8192;
         PRAGMA journal_mode = WAL;
         PRAGMA synchronous  = OFF;
         PRAGMA temp_store   = MEMORY;
         PRAGMA cache_size   = -262144;
         PRAGMA foreign_keys = OFF;",
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
         -- Per-file rows from the raw duscan CSV (regular files only).
         -- Clustered on (folder_id, name): one folder's files live in
         -- contiguous pages, so listing a folder needs no secondary index.
         CREATE TABLE IF NOT EXISTS files (
            folder_id INTEGER NOT NULL,
            name      TEXT    NOT NULL,
            user_id   INTEGER NOT NULL,
            age       INTEGER NOT NULL,
            size      INTEGER NOT NULL,
            atime     INTEGER NOT NULL,
            mtime     INTEGER NOT NULL,
            PRIMARY KEY (folder_id, name)
         ) WITHOUT ROWID;",
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

/// Recorded after a successful `--raw` ingest. `has_files = 1` is what makes
/// duapi serve /api/files from the DB instead of the live filesystem; the age
/// cutoffs document which thresholds froze the per-file `age` column.
pub fn write_files_metadata(
    conn: &Connection,
    raw_csv: &str,
    raw_mtime: i64,
    file_count: u64,
    age_cfg: AgeCfg,
) -> Result<()> {
    let pairs: [(&str, String); 6] = [
        ("has_files", "1".to_string()),
        ("files_source_csv", raw_csv.to_string()),
        ("files_source_csv_mtime", raw_mtime.to_string()),
        ("files_row_count", file_count.to_string()),
        ("age_young_days", age_cfg.young.to_string()),
        ("age_old_days", age_cfg.old.to_string()),
    ];
    upsert_pairs(conn, &pairs)
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
        assert!(names.contains(&"files".to_string()));
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
        write_files_metadata(&c, "raw.csv", 5678, 1000, AgeCfg::default()).unwrap();
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
        assert_eq!(get("files_row_count"), "1000");
        assert_eq!(get("age_young_days"), "60");
        assert_eq!(get("age_old_days"), "600");
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
