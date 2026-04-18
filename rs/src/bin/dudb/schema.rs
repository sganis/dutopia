// rs/src/bin/dudb/schema.rs
use anyhow::Result;
use rusqlite::{params, Connection};

/// `2` since paths are stored byte-for-byte from `dusum` and the synthetic
/// root row uses `full_path = ""` (v1 stored Unix-canonical and used `/`).
pub const SCHEMA_VERSION: &str = "2";

/// Pragmas tuned for bulk ingest. `synchronous=OFF` is safe here because the
/// DB is rebuildable from the source CSV.
pub fn apply_ingest_pragmas(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
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
