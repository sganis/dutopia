// rs/src/db.rs
use crate::filepack;
use crate::item::{FilesOut, FsItemOut};
use anyhow::{anyhow, Context, Result};
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{OpenFlags, OptionalExtension, ToSql};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

pub type DbPool = r2d2::Pool<SqliteConnectionManager>;

pub const SUPPORTED_SCHEMA_VERSION: &str = "4";

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

/// True when the DB carries per-file rows (`dudb --raw`), i.e. /api/files can
/// be served from the `files` table instead of the live filesystem.
pub fn has_files(pool: &DbPool) -> Result<bool> {
    let conn = pool.get().context("acquiring connection")?;
    let v: Option<String> = conn
        .query_row(
            "SELECT value FROM metadata WHERE key = 'has_files'",
            [],
            |r| r.get(0),
        )
        .ok();
    Ok(v.as_deref() == Some("1"))
}

/// The `dudb --max-files` the file index was built with; 0 = uncapped (also
/// for DBs without a file index). Listings beyond it would no longer be the
/// exact largest files, so callers clamp their page size to it.
pub fn files_cap(pool: &DbPool) -> Result<usize> {
    let conn = pool.get().context("acquiring connection")?;
    let v: Option<String> = conn
        .query_row("SELECT value FROM metadata WHERE key = 'files_cap'", [], |r| r.get(0))
        .optional()?;
    Ok(v.and_then(|s| s.parse().ok()).unwrap_or(0))
}

/// Files directly inside `dir_path` from the scan snapshot (regular files
/// only), with the same user/age filters as `item::get_items`: the `limit`
/// largest of those dudb listed, plus per-owner totals of every matching
/// file, listed or not. Nonexistent folders and folders without files return
/// an empty result, matching `list_children`.
pub fn list_files(
    pool: &DbPool,
    dir_path: &str,
    user_filter: &[String],
    age_filter: Option<u8>,
    limit: usize,
) -> Result<FilesOut> {
    let conn = pool.get().context("acquiring connection")?;
    let blob: Option<Vec<u8>> = conn
        .query_row(
            "SELECT ff.data FROM paths p JOIN folder_files ff ON ff.folder_id = p.id
             WHERE p.full_path = ?1",
            [dir_path],
            |r| r.get(0),
        )
        .optional()?;
    let Some(blob) = blob else { return Ok(FilesOut::default()) };

    let mut by_name = conn.prepare_cached("SELECT id FROM users WHERE name = ?1")?;
    let mut names: HashMap<u32, String> = HashMap::new();
    for u in user_filter {
        if let Some(id) = by_name.query_row([u], |r| r.get::<_, u32>(0)).optional()? {
            names.insert(id, u.clone());
        }
    }
    if !user_filter.is_empty() && names.is_empty() {
        return Ok(FilesOut::default());
    }
    let filter_ids: HashSet<u32> = names.keys().copied().collect();
    let wanted = |user_id: u32, age: u8| {
        age_filter.is_none_or(|a| a == age)
            && (user_filter.is_empty() || filter_ids.contains(&user_id))
    };
    let folder = filepack::unpack(&blob)?;
    let mut by_id = conn.prepare_cached("SELECT name FROM users WHERE id = ?1")?;
    let mut name_of = |id: u32| -> Result<String> {
        if let Some(n) = names.get(&id) {
            return Ok(n.clone());
        }
        let n: String = by_id.query_row([id], |r| r.get(0))?;
        names.insert(id, n.clone());
        Ok(n)
    };

    let mut out = FilesOut::default();
    for t in folder.totals.iter().filter(|t| wanted(t.user_id, t.age)) {
        let owner = name_of(t.user_id)?;
        out.totals.entry(owner).or_default().add(t.count, t.bytes, t.atime, t.mtime);
    }
    for f in folder.files.iter().filter(|f| wanted(f.user_id, f.age)).take(limit) {
        out.files.push(FsItemOut {
            path: join_child_path(dir_path, &f.name),
            owner: name_of(f.user_id)?,
            size: f.size,
            accessed: f.atime,
            modified: f.mtime,
        });
    }
    Ok(out)
}

/// True when the DB was built with `dudb --no-files`: folders only on
/// purpose, so callers list no files instead of reading the live filesystem.
pub fn no_files(pool: &DbPool) -> Result<bool> {
    let conn = pool.get().context("acquiring connection")?;
    let v: Option<String> = conn
        .query_row("SELECT value FROM metadata WHERE key = 'no_files'", [], |r| r.get(0))
        .optional()?;
    Ok(v.as_deref() == Some("1"))
}

/// Join a stored folder path and a filename in the folder's native form
/// (mirrors `dusum`'s separator heuristic: any backslash or a drive prefix
/// means Windows). Roots like `/` and `C:\` already end with the separator.
fn join_child_path(folder: &str, name: &str) -> String {
    let bytes = folder.as_bytes();
    let windows = folder.contains('\\')
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':');
    let sep = if windows { '\\' } else { '/' };
    if folder.ends_with(sep) {
        format!("{folder}{name}")
    } else {
        format!("{folder}{sep}{name}")
    }
}

pub mod test_support {
    //! Shared helpers for building a temp SQLite DB so that handler tests and
    //! db tests don't duplicate fixture code.
    use crate::filepack::{GroupTotals, PackedFile, PackedFolder, Packer};
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

    fn uniq() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        // Process-wide counter guarantees uniqueness across parallel test
        // threads; wall-clock nanoseconds alone can collide on fast systems.
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        COUNTER.fetch_add(1, Ordering::Relaxed)
    }

    fn populate(path: &Path) {
        let _ = std::fs::remove_file(path);
        let conn = Connection::open(path).unwrap();
        // Schema v4: synthetic root has full_path='' (empty); platform roots
        // (here `/`) parent to it; each folder's files are one filepack blob
        // in `folder_files`. Mirrors what `dudb --raw` produces from a Linux
        // dusum + duscan CSV pair.
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
             CREATE TABLE folder_files (folder_id INTEGER PRIMARY KEY, data BLOB NOT NULL);
             CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE INDEX idx_paths_parent ON paths(parent_id);
             INSERT INTO metadata(key,value) VALUES('schema_version','4'),('has_files','1');
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
        // Alice also owns 5 small age-2 files that dudb counted but did not list.
        insert_folder_files(
            &conn,
            "/docs",
            &[
                ("a.txt", "alice", 2, 400, 1500000000, 1500000050),
                ("b.txt", "alice", 2, 200, 1500000000, 1500000050),
                ("c.txt", "bob", 0, 50, 1700000000, 1700000100),
            ],
            &[("alice", 2, 5, 10, 1500000000, 1500000099)],
        );
    }

    /// Pack `files` (name, owner, age, size, atime, mtime) as the
    /// `folder_files` row of `folder`, in the given (largest-first) order.
    /// Totals count every listed file plus the `unlisted` ones (owner, age,
    /// count, bytes, atime, mtime).
    pub fn insert_folder_files(
        conn: &Connection,
        folder: &str,
        files: &[(&str, &str, u8, u64, i64, i64)],
        unlisted: &[(&str, u8, u64, u64, i64, i64)],
    ) {
        let user_id = |name: &str| -> u32 {
            conn.query_row("SELECT id FROM users WHERE name = ?1", [name], |r| r.get(0))
                .unwrap()
        };
        let mut packed = PackedFolder::default();
        let mut add = |user_id: u32, age: u8, count: u64, bytes: u64, atime: i64, mtime: i64| {
            match packed.totals.iter_mut().find(|t| (t.user_id, t.age) == (user_id, age)) {
                Some(t) => {
                    t.count += count;
                    t.bytes += bytes;
                    t.atime = t.atime.max(atime);
                    t.mtime = t.mtime.max(mtime);
                }
                None => packed.totals.push(GroupTotals { user_id, age, count, bytes, atime, mtime }),
            }
        };
        for &(_, owner, age, size, atime, mtime) in files {
            add(user_id(owner), age, 1, size, atime, mtime);
        }
        for &(owner, age, count, bytes, atime, mtime) in unlisted {
            add(user_id(owner), age, count, bytes, atime, mtime);
        }
        packed.files = files
            .iter()
            .map(|&(name, owner, age, size, atime, mtime)| PackedFile {
                name: name.into(),
                user_id: user_id(owner),
                age,
                size,
                atime,
                mtime,
            })
            .collect();
        let blob = Packer::new().unwrap().pack(&packed).unwrap();
        conn.execute(
            "INSERT INTO folder_files(folder_id, data)
             VALUES ((SELECT id FROM paths WHERE full_path = ?1), ?2)",
            rusqlite::params![folder, blob],
        )
        .unwrap();
    }
}

#[cfg(test)]
#[path = "db_tests.rs"]
mod tests;
