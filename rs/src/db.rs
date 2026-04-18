// rs/src/db.rs
use anyhow::{anyhow, Context, Result};
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{OpenFlags, ToSql};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

pub type DbPool = r2d2::Pool<SqliteConnectionManager>;

pub const SUPPORTED_SCHEMA_VERSION: &str = "2";

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Age {
    pub count: u64,
    pub size: u64,
    pub disk: u64,
    pub linked: u64,
    pub atime: i64,
    pub mtime: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FolderOut {
    pub path: String,
    pub users: HashMap<String, HashMap<String, Age>>,
}

/// Open a read-only connection pool against the given DB and validate schema.
pub fn open_pool(db: &Path) -> Result<DbPool> {
    if !db.exists() {
        return Err(anyhow!("DB file not found: {}", db.display()));
    }
    let manager = SqliteConnectionManager::file(db)
        .with_flags(OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .with_init(|c| {
            c.execute_batch(
                "PRAGMA query_only = ON;
                 PRAGMA mmap_size  = 30000000000;
                 PRAGMA cache_size = -65536;
                 PRAGMA temp_store = MEMORY;",
            )
        });
    let pool_size = std::cmp::max(num_cpus::get(), 4) as u32;
    let pool = r2d2::Pool::builder()
        .max_size(pool_size)
        .build(manager)
        .with_context(|| format!("opening pool for {}", db.display()))?;

    let conn = pool.get().context("acquiring connection")?;
    let v: Option<String> = conn
        .query_row(
            "SELECT value FROM metadata WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .ok();
    match v.as_deref() {
        Some(s) if s == SUPPORTED_SCHEMA_VERSION => {}
        Some(other) => {
            return Err(anyhow!(
                "DB schema_version = {} but duapi expects {}; rebuild with newer dudb",
                other,
                SUPPORTED_SCHEMA_VERSION
            ));
        }
        None => {
            return Err(anyhow!(
                "DB has no metadata.schema_version; not a dudb-built database"
            ));
        }
    }
    Ok(pool)
}

/// Return all usernames sorted ascending.
pub fn list_users(pool: &DbPool) -> Result<Vec<String>> {
    let conn = pool.get().context("acquiring connection")?;
    let mut stmt = conn.prepare("SELECT name FROM users ORDER BY name")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Children of `dir_path`, with per-user / per-age stats. Filters mirror the
/// previous in-memory implementation: empty `user_filter` means all users;
/// `age_filter` of `None` means all three age buckets.
///
/// `dir_path` is matched verbatim against `paths.full_path`, which stores the
/// exact OS-native form `dusum::aggregate::get_folder_ancestors` produced.
/// `query::normalize_path` is responsible for putting the request into that
/// form. The empty string maps to the synthetic root above all platform roots
/// (so a Linux DB returns `/`; a Windows DB returns `C:\`, `D:\`, `\\srv`).
///
/// A folder with zero matching (user, age) rows does not appear in the result.
/// A nonexistent `dir_path` simply returns an empty Vec.
pub fn list_children(
    pool: &DbPool,
    dir_path: &str,
    user_filter: &[String],
    age_filter: Option<u8>,
) -> Result<Vec<FolderOut>> {
    let conn = pool.get().context("acquiring connection")?;

    let mut sql = String::from(
        "SELECT p.full_path, u.name, s.age,
                s.file_count, s.file_size, s.disk_bytes, s.linked_size,
                s.atime, s.mtime
         FROM   paths parent
         JOIN   paths p ON p.parent_id = parent.id
         JOIN   stats s ON s.path_id   = p.id
         JOIN   users u ON u.id        = s.user_id
         WHERE  parent.full_path = ?1",
    );

    // Boxed params so we can grow the list dynamically.
    let mut params: Vec<Box<dyn ToSql>> = vec![Box::new(dir_path.to_string())];

    if let Some(a) = age_filter {
        sql.push_str(&format!(" AND s.age = ?{}", params.len() + 1));
        params.push(Box::new(a as i64));
    }

    if !user_filter.is_empty() {
        sql.push_str(" AND u.name IN (");
        for (i, u) in user_filter.iter().enumerate() {
            if i > 0 {
                sql.push(',');
            }
            sql.push_str(&format!("?{}", params.len() + 1));
            params.push(Box::new(u.clone()));
        }
        sql.push(')');
    }

    sql.push_str(" ORDER BY p.full_path");

    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn ToSql> = params.iter().map(|b| b.as_ref()).collect();

    let rows = stmt.query_map(param_refs.as_slice(), |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, u8>(2)?,
            r.get::<_, u64>(3)?,
            r.get::<_, u64>(4)?,
            r.get::<_, u64>(5)?,
            r.get::<_, u64>(6)?,
            r.get::<_, i64>(7)?,
            r.get::<_, i64>(8)?,
        ))
    })?;

    // Group rows: path -> user -> age -> Age. BTreeMap on path keeps output
    // sorted to match the legacy implementation's `items.sort_by(path)`.
    let mut grouped: BTreeMap<String, HashMap<String, HashMap<String, Age>>> = BTreeMap::new();
    for row in rows {
        let (path, user, age, count, size, disk, linked, atime, mtime) = row?;
        let users_map = grouped.entry(path).or_default();
        let ages_map = users_map.entry(user).or_default();
        ages_map.insert(
            age.to_string(),
            Age {
                count,
                size,
                disk,
                linked,
                atime,
                mtime,
            },
        );
    }

    Ok(grouped
        .into_iter()
        .map(|(path, users)| FolderOut { path, users })
        .collect())
}

pub mod test_support {
    //! Shared helpers for building a temp SQLite DB so that handler tests and
    //! db tests don't duplicate fixture code.
    use rusqlite::Connection;
    use std::path::{Path, PathBuf};

    /// Owns a SQLite DB file and removes it (plus -wal/-shm) on drop.
    pub struct TempDb {
        pub path: PathBuf,
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
            for ext in ["-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{}", self.path.display(), ext));
            }
        }
    }

    pub fn build_test_db() -> TempDb {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "duapi_test_{}_{}.db",
            std::process::id(),
            uniq()
        ));
        populate(&path);
        TempDb { path }
    }

    fn uniq() -> u128 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }

    fn populate(path: &Path) {
        let _ = std::fs::remove_file(path);
        let conn = Connection::open(path).unwrap();
        // Schema v2: synthetic root has full_path='' (empty); platform roots
        // (here `/`) parent to it. Mirrors what dudb produces from a Linux
        // dusum CSV.
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=OFF;
             CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
             CREATE TABLE paths (id INTEGER PRIMARY KEY, parent_id INTEGER, full_path TEXT NOT NULL UNIQUE);
             CREATE TABLE stats (
                path_id INTEGER NOT NULL, user_id INTEGER NOT NULL, age INTEGER NOT NULL,
                file_count INTEGER NOT NULL, file_size INTEGER NOT NULL,
                disk_bytes INTEGER NOT NULL, linked_size INTEGER NOT NULL,
                atime INTEGER NOT NULL, mtime INTEGER NOT NULL,
                PRIMARY KEY (path_id, user_id, age)
             ) WITHOUT ROWID;
             CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE INDEX idx_paths_parent ON paths(parent_id);
             INSERT INTO metadata(key,value) VALUES('schema_version','2');
             INSERT INTO users(name) VALUES('alice'),('bob');
             INSERT INTO paths(full_path, parent_id) VALUES('', NULL);
             INSERT INTO paths(full_path, parent_id) VALUES(
                 '/', (SELECT id FROM paths WHERE full_path=''));
             INSERT INTO paths(full_path, parent_id) VALUES(
                 '/docs', (SELECT id FROM paths WHERE full_path='/'));
             INSERT INTO stats VALUES
               ((SELECT id FROM paths WHERE full_path='/'),
                (SELECT id FROM users WHERE name='alice'),
                0, 2, 200, 100, 0, 1700000000, 1700000100),
               ((SELECT id FROM paths WHERE full_path='/'),
                (SELECT id FROM users WHERE name='bob'),
                1, 1, 50, 50, 0, 1600000000, 1600000100),
               ((SELECT id FROM paths WHERE full_path='/docs'),
                (SELECT id FROM users WHERE name='alice'),
                2, 3, 600, 300, 300, 1500000000, 1500000050);",
        )
        .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_pool() -> (test_support::TempDb, DbPool) {
        let db = test_support::build_test_db();
        let pool = open_pool(&db.path).unwrap();
        (db, pool)
    }

    #[test]
    fn list_users_returns_sorted() {
        let (_db, pool) = build_pool();
        let users = list_users(&pool).unwrap();
        assert_eq!(users, vec!["alice".to_string(), "bob".to_string()]);
    }

    #[test]
    fn list_children_under_unix_root_returns_docs() {
        let (_db, pool) = build_pool();
        let items = list_children(&pool, "/", &[], None).unwrap();
        assert!(items.iter().any(|i| i.path == "/docs"));
    }

    #[test]
    fn list_children_user_filter_drops_folders_with_no_match() {
        let (_db, pool) = build_pool();
        let items = list_children(&pool, "/", &["alice".to_string()], None).unwrap();
        assert!(items.iter().any(|i| i.path == "/docs"));
        let docs = items.iter().find(|i| i.path == "/docs").unwrap();
        assert!(docs.users.contains_key("alice"));
        assert!(!docs.users.contains_key("bob"));
    }

    #[test]
    fn list_children_age_filter() {
        let (_db, pool) = build_pool();
        let items = list_children(&pool, "/", &[], Some(2)).unwrap();
        let docs = items.iter().find(|i| i.path == "/docs").unwrap();
        let alice = docs.users.get("alice").unwrap();
        assert!(alice.contains_key("2"));
        assert!(!alice.contains_key("0"));
    }

    #[test]
    fn list_children_unknown_path_is_empty() {
        let (_db, pool) = build_pool();
        let items = list_children(&pool, "/nope", &[], None).unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn list_children_empty_path_returns_platform_roots() {
        // Frontend's synthetic-root marker. On the Linux fixture, this is `/`.
        let (_db, pool) = build_pool();
        let items = list_children(&pool, "", &[], None).unwrap();
        assert_eq!(items.len(), 1, "expected exactly one platform root");
        assert_eq!(items[0].path, "/");
    }

    /// Windows fixture: paths stored with backslashes, drive roots and UNC.
    /// Verifies that the request paths the frontend would send (which come
    /// out of `query::normalize_path` in OS-native form) match the DB
    /// byte-for-byte without any extra translation layer.
    #[test]
    fn list_children_windows_native_paths() {
        use rusqlite::Connection;
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "duapi_winfix_{}_{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
             CREATE TABLE paths (id INTEGER PRIMARY KEY, parent_id INTEGER, full_path TEXT NOT NULL UNIQUE);
             CREATE TABLE stats (
                path_id INTEGER NOT NULL, user_id INTEGER NOT NULL, age INTEGER NOT NULL,
                file_count INTEGER NOT NULL, file_size INTEGER NOT NULL,
                disk_bytes INTEGER NOT NULL, linked_size INTEGER NOT NULL,
                atime INTEGER NOT NULL, mtime INTEGER NOT NULL,
                PRIMARY KEY (path_id, user_id, age)
             ) WITHOUT ROWID;
             CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE INDEX idx_paths_parent ON paths(parent_id);
             INSERT INTO metadata(key,value) VALUES('schema_version','2');
             INSERT INTO users(name) VALUES('San');
             INSERT INTO paths(full_path, parent_id) VALUES('', NULL);
             INSERT INTO paths(full_path, parent_id) VALUES('C:\\',
                 (SELECT id FROM paths WHERE full_path=''));
             INSERT INTO paths(full_path, parent_id) VALUES('C:\\Users',
                 (SELECT id FROM paths WHERE full_path='C:\\'));
             INSERT INTO paths(full_path, parent_id) VALUES('\\\\srv',
                 (SELECT id FROM paths WHERE full_path=''));
             INSERT INTO stats VALUES
               ((SELECT id FROM paths WHERE full_path='C:\\'),
                (SELECT id FROM users WHERE name='San'),
                0, 10, 1000, 1000, 0, 1, 1),
               ((SELECT id FROM paths WHERE full_path='C:\\Users'),
                (SELECT id FROM users WHERE name='San'),
                0, 5, 500, 500, 0, 1, 1),
               ((SELECT id FROM paths WHERE full_path='\\\\srv'),
                (SELECT id FROM users WHERE name='San'),
                0, 2, 200, 200, 0, 1, 1);",
        )
        .unwrap();
        drop(conn);

        let pool = open_pool(&path).unwrap();
        let cleanup = test_support::TempDb { path: path.clone() };

        // Empty path → both platform roots.
        let roots = list_children(&pool, "", &[], None).unwrap();
        let root_paths: Vec<&str> = roots.iter().map(|i| i.path.as_str()).collect();
        assert!(root_paths.contains(&"C:\\"));
        assert!(root_paths.contains(&"\\\\srv"));

        // C:\ → C:\Users (sent verbatim, no translation).
        let drive_children = list_children(&pool, "C:\\", &[], None).unwrap();
        assert_eq!(drive_children.len(), 1);
        assert_eq!(drive_children[0].path, "C:\\Users");

        drop(cleanup);
    }

    #[test]
    fn open_pool_rejects_missing_metadata() {
        let path = std::env::temp_dir().join(format!(
            "duapi_bad_meta_{}_{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE x(y INT);").unwrap();
        drop(conn);
        let err = open_pool(&path).unwrap_err().to_string();
        let _ = std::fs::remove_file(&path);
        assert!(err.contains("metadata") || err.contains("schema"));
    }
}
