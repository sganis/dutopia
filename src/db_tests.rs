// src/db_tests.rs
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
fn has_files_true_on_fixture() {
    let (_db, pool) = build_pool();
    assert!(has_files(&pool).unwrap());
}

#[test]
fn list_files_returns_largest_first_full_paths() {
    let (_db, pool) = build_pool();
    let items = list_files(&pool, "/docs", &[], None, 100).unwrap().files;
    let paths: Vec<&str> = items.iter().map(|i| i.path.as_str()).collect();
    assert_eq!(paths, vec!["/docs/a.txt", "/docs/b.txt", "/docs/c.txt"]);
    assert_eq!(items[0].owner, "alice");
    assert_eq!(items[0].size, 400);
    assert_eq!(items[2].owner, "bob");
}

#[test]
fn list_files_totals_include_unlisted_files() {
    let (_db, pool) = build_pool();
    let out = list_files(&pool, "/docs", &[], None, 100).unwrap();
    let alice = out.totals["alice"];
    // a.txt + b.txt listed, plus 5 small files counted but not listed.
    assert_eq!((alice.count, alice.size), (7, 610));
    assert_eq!(alice.modified, 1500000099);
    let bob = out.totals["bob"];
    assert_eq!((bob.count, bob.size, bob.modified), (1, 50, 1700000100));
    // A short page still reports every file.
    let one = list_files(&pool, "/docs", &[], None, 1).unwrap();
    assert_eq!(one.files.len(), 1);
    assert_eq!(one.totals, out.totals);
}

#[test]
fn list_files_user_and_age_filters() {
    let (_db, pool) = build_pool();
    let alice = list_files(&pool, "/docs", &["alice".to_string()], None, 100).unwrap();
    assert_eq!(alice.files.len(), 2);
    assert!(alice.files.iter().all(|i| i.owner == "alice"));
    assert_eq!(alice.totals.keys().collect::<Vec<_>>(), ["alice"]);

    let age0 = list_files(&pool, "/docs", &[], Some(0), 100).unwrap();
    assert_eq!(age0.files.len(), 1);
    assert_eq!(age0.files[0].path, "/docs/c.txt");
    assert_eq!(age0.totals.keys().collect::<Vec<_>>(), ["bob"]);
}

#[test]
fn list_files_limit_keeps_largest_matches() {
    let (_db, pool) = build_pool();
    let items = list_files(&pool, "/docs", &[], None, 2).unwrap().files;
    let paths: Vec<&str> = items.iter().map(|i| i.path.as_str()).collect();
    assert_eq!(paths, vec!["/docs/a.txt", "/docs/b.txt"]);
    // The limit counts matches, not stored files: bob's only file is last.
    let bob = list_files(&pool, "/docs", &["bob".to_string()], None, 1).unwrap().files;
    assert_eq!(bob.len(), 1);
    assert_eq!(bob[0].path, "/docs/c.txt");
}

#[test]
fn list_files_unknown_user_is_empty() {
    let (_db, pool) = build_pool();
    let out = list_files(&pool, "/docs", &["nobody".to_string()], None, 100).unwrap();
    assert!(out.files.is_empty() && out.totals.is_empty());
}

#[test]
fn files_cap_and_no_files_read_metadata() {
    let db = test_support::build_test_db();
    let pool = open_pool(&db.path).unwrap();
    assert_eq!(files_cap(&pool).unwrap(), 0);
    assert!(!no_files(&pool).unwrap());
    rusqlite::Connection::open(&db.path)
        .unwrap()
        .execute_batch(
            "INSERT INTO metadata(key, value) VALUES('files_cap', '100'), ('no_files', '1')",
        )
        .unwrap();
    let pool = open_pool(&db.path).unwrap();
    assert_eq!(files_cap(&pool).unwrap(), 100);
    assert!(no_files(&pool).unwrap());
}

#[test]
fn list_files_unknown_folder_is_empty() {
    let (_db, pool) = build_pool();
    assert!(list_files(&pool, "/nope", &[], None, 100).unwrap().files.is_empty());
    // Folders without a folder_files row are empty too, not an error.
    let root = list_files(&pool, "/", &[], None, 100).unwrap();
    assert!(root.files.is_empty() && root.totals.is_empty());
}

#[test]
fn join_child_path_native_forms() {
    assert_eq!(join_child_path("/", "f"), "/f");
    assert_eq!(join_child_path("/a/b", "f"), "/a/b/f");
    assert_eq!(join_child_path("C:\\", "f"), "C:\\f");
    assert_eq!(join_child_path("C:\\Users", "f"), "C:\\Users\\f");
    assert_eq!(join_child_path("\\\\srv\\shr", "f"), "\\\\srv\\shr\\f");
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
         CREATE TABLE folder_files (folder_id INTEGER PRIMARY KEY, data BLOB NOT NULL);
         CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE INDEX idx_paths_parent ON paths(parent_id);
         INSERT INTO metadata(key,value) VALUES('schema_version','4'),('has_files','1');
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
    test_support::insert_folder_files(&conn, "C:\\Users", &[("notes.txt", "San", 0, 10, 1, 1)], &[]);
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

    // File paths joined with the folder's native separator.
    let files = list_files(&pool, "C:\\Users", &[], None, 10).unwrap().files;
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "C:\\Users\\notes.txt");

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
