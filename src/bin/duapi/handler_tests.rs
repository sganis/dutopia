// rs/src/bin/duapi/handler_tests.rs
use super::*;
use axum::body::to_bytes;
use axum::extract::Query;
use serial_test::serial;

use crate::{DB_POOL, DB_VERSION, FILES_INDEXED, TEST_DB, USERS};
use dutopia::db::FolderOut;
use dutopia::item::FilesOut;

const TEST_BODY_LIMIT: usize = 2 * 1024 * 1024;

pub(crate) fn init_db_once() {
    if DB_POOL.get().is_some() {
        return;
    }
    let temp_db = dutopia::db::test_support::build_test_db();
    let pool = dutopia::db::open_pool(&temp_db.path).expect("open_pool");
    let users = dutopia::db::list_users(&pool).expect("list_users");
    let files_indexed = dutopia::db::has_files(&pool).expect("has_files");
    let db_version = dutopia::db::built_at(&pool).expect("built_at");
    // Keep the TempDb alive for the entire test run so the file is not
    // removed while the pool is still using it.
    let _ = TEST_DB.set(temp_db);
    let _ = DB_POOL.set(Some(pool));
    let _ = USERS.set(users);
    let _ = FILES_INDEXED.set(files_indexed);
    let _ = DB_VERSION.set(db_version);
}

#[tokio::test]
#[serial]
async fn test_users_handler_admin_and_user() {
    init_db_once();
    let admin = Claims {
        sub: "root".to_string(),
        is_admin: true,
        roots: vec![],
            exp: 9_999_999_999usize,
    };
    let resp_admin = users_handler(admin).await.into_response();
    assert_eq!(resp_admin.status(), StatusCode::OK);
    let body = to_bytes(resp_admin.into_body(), TEST_BODY_LIMIT)
        .await
        .unwrap();
    let list: Vec<String> = serde_json::from_slice(&body).unwrap();
    assert!(list.contains(&"alice".to_string()));
    assert!(list.contains(&"bob".to_string()));

    let user = Claims {
        sub: "alice".to_string(),
        is_admin: false,
        roots: vec![],
            exp: 9_999_999_999usize,
    };
    let resp_user = users_handler(user).await.into_response();
    let body = to_bytes(resp_user.into_body(), TEST_BODY_LIMIT)
        .await
        .unwrap();
    let list: Vec<String> = serde_json::from_slice(&body).unwrap();
    assert_eq!(list, vec!["alice".to_string()]);
}

#[tokio::test]
#[serial]
async fn test_get_folders_handler_authz_and_filters() {
    init_db_once();

    let non_admin = Claims {
        sub: "alice".into(),
        is_admin: false,
        roots: vec![],
            exp: 9_999_999_999usize,
    };
    let q_all = FolderQuery {
        path: Some("/".into()),
        users: None,
        age: None,
    };
    let resp = get_folders_handler(non_admin.clone(), Query(q_all))
        .await
        .into_response();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let q_self = FolderQuery {
        path: Some("/".into()),
        users: Some("alice".into()),
        age: None,
    };
    let resp = get_folders_handler(non_admin, Query(q_self))
        .await
        .into_response();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), TEST_BODY_LIMIT).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(v.is_array());

    let admin = Claims {
        sub: "root".into(),
        is_admin: true,
        roots: vec![],
            exp: 9_999_999_999usize,
    };
    let q_admin_all = FolderQuery {
        path: Some("/".into()),
        users: None,
        age: None,
    };
    let resp = get_folders_handler(admin, Query(q_admin_all))
        .await
        .into_response();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), TEST_BODY_LIMIT).await.unwrap();
    let arr: Vec<FolderOut> = serde_json::from_slice(&body).unwrap();
    assert!(arr.iter().any(|it| it.path == "/docs"));
    let docs = arr.into_iter().find(|it| it.path == "/docs").unwrap();
    assert!(!docs.users.is_empty());
}

#[tokio::test]
async fn test_get_files_handler_bad_path() {
    let claims = Claims {
        sub: "any".into(),
        is_admin: true,
        roots: vec![],
            exp: 9_999_999_999usize,
    };
    let q = FilesQuery {
        path: None,
        users: None,
        age: None,
    };
    let resp = get_files_handler(claims, Query(q)).await.into_response();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_get_folders_handler_rejects_traversal() {
    let claims = Claims {
        sub: "root".into(),
        is_admin: true,
        roots: vec![],
            exp: 9_999_999_999usize,
    };
    let q = FolderQuery {
        path: Some("/var/../etc/passwd".into()),
        users: None,
        age: None,
    };
    let resp = get_folders_handler(claims, Query(q)).await.into_response();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_get_files_handler_rejects_traversal() {
    let claims = Claims {
        sub: "root".into(),
        is_admin: true,
        roots: vec![],
            exp: 9_999_999_999usize,
    };
    let q = FilesQuery {
        path: Some("/var/../etc/passwd".into()),
        users: None,
        age: None,
    };
    let resp = get_files_handler(claims, Query(q)).await.into_response();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[serial]
async fn test_get_files_handler_db_admin_ok() {
    init_db_once();
    let claims = Claims {
        sub: "root".into(),
        is_admin: true,
        roots: vec![],
            exp: 9_999_999_999usize,
    };
    let q = FilesQuery {
        path: Some("/docs".into()),
        users: None,
        age: None,
    };

    let resp = get_files_handler(claims, Query(q)).await.into_response();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), TEST_BODY_LIMIT).await.unwrap();
    let items = serde_json::from_slice::<FilesOut>(&body).unwrap().files;
    let paths: Vec<&str> = items.iter().map(|i| i.path.as_str()).collect();
    assert_eq!(paths, vec!["/docs/a.txt", "/docs/b.txt", "/docs/c.txt"]);
}

#[tokio::test]
#[serial]
async fn test_get_files_handler_db_filters() {
    init_db_once();
    let claims = Claims {
        sub: "alice".into(),
        is_admin: false,
        roots: vec![],
            exp: 9_999_999_999usize,
    };
    // Non-admin restricted to self: only alice's files come back.
    let q = FilesQuery {
        path: Some("/docs".into()),
        users: Some("alice".into()),
        age: Some(2),
    };
    let resp = get_files_handler(claims, Query(q)).await.into_response();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), TEST_BODY_LIMIT).await.unwrap();
    let items = serde_json::from_slice::<FilesOut>(&body).unwrap().files;
    assert_eq!(items.len(), 2);
    assert!(items.iter().all(|i| i.owner == "alice"));
}

#[tokio::test]
#[serial]
async fn test_get_files_handler_clamps_to_max_page_size() {
    init_db_once();
    // SAFETY: we set then restore for test isolation.
    unsafe { std::env::set_var("MAX_PAGE_SIZE", "2") };
    let claims = Claims {
        sub: "root".into(),
        is_admin: true,
        roots: vec![],
            exp: 9_999_999_999usize,
    };
    let q = FilesQuery {
        path: Some("/docs".into()),
        users: None,
        age: None,
    };
    let resp = get_files_handler(claims, Query(q)).await.into_response();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), TEST_BODY_LIMIT).await.unwrap();
    let items = serde_json::from_slice::<FilesOut>(&body).unwrap().files;
    assert_eq!(items.len(), 2);
    unsafe { std::env::remove_var("MAX_PAGE_SIZE") };
}

#[tokio::test]
async fn test_get_files_handler_non_admin_forbidden() {
    let claims = Claims {
        sub: "alice".into(),
        is_admin: false,
        roots: vec![],
            exp: 9_999_999_999usize,
    };
    // No user filter: rejected before any DB or filesystem access.
    let q = FilesQuery {
        path: Some("/docs".into()),
        users: None,
        age: None,
    };
    let resp = get_files_handler(claims, Query(q)).await.into_response();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_login_missing_credentials() {
    let bad1 = AuthPayload {
        username: "".into(),
        password: "x".into(),
    };
    let err1 = login_handler(Json(bad1)).await.unwrap_err();
    assert!(matches!(err1, AuthError::MissingCredentials));

    let bad2 = AuthPayload {
        username: "x".into(),
        password: "".into(),
    };
    let err2 = login_handler(Json(bad2)).await.unwrap_err();
    assert!(matches!(err2, AuthError::MissingCredentials));
}

#[tokio::test]
#[serial]
async fn test_list_children_filters_and_ages() {
    init_db_once();
    let pool = DB_POOL.get().unwrap().as_ref().unwrap();

    let items = dutopia::db::list_children(pool, "/", &[], None).unwrap();
    assert!(items.iter().any(|it| it.path == "/docs"));

    let items_alice =
        dutopia::db::list_children(pool, "/", &["alice".to_string()], None).unwrap();
    assert!(items_alice.iter().any(|it| it.path == "/docs"));
    let docs = items_alice
        .into_iter()
        .find(|it| it.path == "/docs")
        .unwrap();
    assert!(docs.users.contains_key("alice"));

    let items_age2 = dutopia::db::list_children(pool, "/", &[], Some(2)).unwrap();
    let docs2 = items_age2
        .into_iter()
        .find(|it| it.path == "/docs")
        .unwrap();
    let alice_ages = docs2.users.get("alice").unwrap();
    assert!(alice_ages.contains_key("2"));
}

#[tokio::test]
#[serial]
async fn test_health_handler() {
    init_db_once();
    let resp = health_handler().await.into_response();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), TEST_BODY_LIMIT).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["status"], "ok");
    assert!(v["smtp_configured"].is_boolean());
    // Present (non-null) once the test DB is loaded: dudb writes built_at,
    // so the browser has a version to key its cache on.
    assert!(v["db_version"].is_string());
}

#[tokio::test]
#[serial]
async fn test_get_folders_handler_clamps_to_max_page_size() {
    init_db_once();
    // SAFETY: we set then restore for test isolation.
    unsafe { std::env::set_var("MAX_PAGE_SIZE", "1") };
    let admin = Claims {
        sub: "root".into(),
        is_admin: true,
        roots: vec![],
            exp: 9_999_999_999usize,
    };
    let q = FolderQuery {
        path: Some("/".into()),
        users: None,
        age: None,
    };
    let resp = get_folders_handler(admin, Query(q)).await.into_response();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), TEST_BODY_LIMIT).await.unwrap();
    let arr: Vec<FolderOut> = serde_json::from_slice(&body).unwrap();
    assert!(arr.len() <= 1);
    unsafe { std::env::remove_var("MAX_PAGE_SIZE") };
}

#[tokio::test]
#[serial]
async fn test_get_folders_handler_storage_admin_scoped() {
    init_db_once();

    let sa = Claims {
        sub: "user1".into(),
        is_admin: false,
        roots: vec!["/docs".into()],
        exp: 9_999_999_999usize,
    };

    // Inside root: no users filter needed (full admin view).
    let q = FolderQuery {
        path: Some("/docs".into()),
        users: None,
        age: None,
    };
    let resp = get_folders_handler(sa.clone(), Query(q)).await.into_response();
    assert_eq!(resp.status(), StatusCode::OK);

    // Ancestor of root (SPA navigation): allowed, children filtered to the
    // route into the roots.
    let q = FolderQuery {
        path: Some("/".into()),
        users: None,
        age: None,
    };
    let resp = get_folders_handler(sa.clone(), Query(q)).await.into_response();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), TEST_BODY_LIMIT).await.unwrap();
    let arr: Vec<FolderOut> = serde_json::from_slice(&body).unwrap();
    // Only the next hop towards the root survives; unrelated top-level
    // folders are dropped.
    assert!(arr.iter().all(|it| it.path == "/docs" || it.path.starts_with("/docs/")));

    // Off-route sibling: forbidden.
    let q = FolderQuery {
        path: Some("/opt".into()),
        users: None,
        age: None,
    };
    let resp = get_folders_handler(sa.clone(), Query(q)).await.into_response();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // …but users=self is allowed anywhere (regular-user fallback).
    let q = FolderQuery {
        path: Some("/opt".into()),
        users: Some("user1".into()),
        age: None,
    };
    let resp = get_folders_handler(sa.clone(), Query(q)).await.into_response();
    assert_eq!(resp.status(), StatusCode::OK);

    // Prefix trap: /docs-sub is NOT inside root /docs.
    let q = FolderQuery {
        path: Some("/docs-sub".into()),
        users: None,
        age: None,
    };
    let resp = get_folders_handler(sa, Query(q)).await.into_response();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
#[serial]
async fn test_get_files_handler_storage_admin_scoped() {
    init_db_once();

    let sa = Claims {
        sub: "user1".into(),
        is_admin: false,
        roots: vec!["/docs".into()],
        exp: 9_999_999_999usize,
    };

    let q = FilesQuery {
        path: Some("/docs".into()),
        users: None,
        age: None,
    };
    let resp = get_files_handler(sa.clone(), Query(q)).await.into_response();
    if crate::files_disabled() {
        // --no-files DB: handler short-circuits after the authz check.
        assert_eq!(resp.status(), StatusCode::OK);
    } else {
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // The SPA never calls /api/files for the root ('' or '/'), and the
    // handler rejects those paths before authz — unchanged behavior. The
    // navigable-files branch (empty listing for a real intermediate folder
    // above the root) is covered by check_access unit tests plus the
    // /api/folders test above, since the test DB has no deeper nesting.

    // Off-route sibling: forbidden.
    let q = FilesQuery {
        path: Some("/opt".into()),
        users: None,
        age: None,
    };
    let resp = get_files_handler(sa.clone(), Query(q)).await.into_response();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let q = FilesQuery {
        path: Some("/".into()),
        users: None,
        age: None,
    };
    let resp = get_files_handler(sa, Query(q)).await.into_response();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST); // '/' rejected before authz
}

#[tokio::test]
#[serial]
async fn test_users_handler_storage_admin_gets_full_list() {
    init_db_once();
    let sa = Claims {
        sub: "user1".into(),
        is_admin: false,
        roots: vec!["/docs".into()],
        exp: 9_999_999_999usize,
    };
    let resp = users_handler(sa).await.into_response();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), TEST_BODY_LIMIT).await.unwrap();
    let list: Vec<String> = serde_json::from_slice(&body).unwrap();
    assert!(list.contains(&"alice".to_string()));
    assert!(list.contains(&"bob".to_string()));
}

async fn home_paths(claims: Claims, users: Option<&str>) -> (StatusCode, Vec<FolderOut>) {
    let q = FolderQuery { path: Some("".into()), users: users.map(Into::into), age: None };
    let resp = get_folders_handler(claims, Query(q)).await.into_response();
    let status = resp.status();
    let body = to_bytes(resp.into_body(), TEST_BODY_LIMIT).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap_or_default())
}

#[tokio::test]
#[serial]
async fn test_home_lists_filesystems_when_set() {
    init_db_once();
    let admin = Claims { sub: "root".into(), is_admin: true, roots: vec![], exp: 9_999_999_999 };
    let alice = Claims { sub: "alice".into(), is_admin: false, roots: vec![], exp: 9_999_999_999 };

    // Without FILESYSTEMS the home page is the path tree: `/`.
    unsafe { std::env::remove_var("FILESYSTEMS") };
    let (_, rows) = home_paths(admin.clone(), None).await;
    assert_eq!(rows.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(), vec!["/"]);

    unsafe { std::env::set_var("FILESYSTEMS", "/docs,/missing") };
    let (status, rows) = home_paths(admin, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rows.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(), vec!["/docs"]);

    // Regular user: own usage on each filesystem; all users is forbidden.
    let (status, rows) = home_paths(alice.clone(), Some("alice")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].users.keys().all(|u| u == "alice"));
    let (status, _) = home_paths(alice, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    unsafe { std::env::remove_var("FILESYSTEMS") };
}

#[tokio::test]
#[serial]
async fn test_home_storage_admin_sees_filesystems_of_their_roots() {
    init_db_once();
    unsafe { std::env::set_var("FILESYSTEMS", "/,/docs") };
    // Root /docs is a filesystem; `/` holds it too (longest-prefix aside,
    // both rows contain the root).
    let sa = Claims { sub: "user1".into(), is_admin: false, roots: vec!["/docs".into()], exp: 9_999_999_999 };
    let (status, rows) = home_paths(sa, Some("bob")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rows.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(), vec!["/", "/docs"]);
    // Users filter dropped (navigable): bob's and alice's usage both show.
    assert!(rows[0].users.contains_key("alice"));

    let other = Claims { sub: "user2".into(), is_admin: false, roots: vec!["/opt".into()], exp: 9_999_999_999 };
    unsafe { std::env::set_var("FILESYSTEMS", "/docs") };
    let (status, rows) = home_paths(other, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(rows.is_empty());
    unsafe { std::env::remove_var("FILESYSTEMS") };
}

#[tokio::test]
#[serial]
async fn test_health_reports_filesystems() {
    init_db_once();
    unsafe { std::env::set_var("FILESYSTEMS", "/red/nas1/,/red/nas2") };
    let resp = health_handler().await.into_response();
    let body = to_bytes(resp.into_body(), TEST_BODY_LIMIT).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["filesystems"], serde_json::json!(["/red/nas1", "/red/nas2"]));
    unsafe { std::env::remove_var("FILESYSTEMS") };
}

#[tokio::test]
#[serial]
async fn test_health_reports_trash_keep_days() {
    init_db_once();
    let keep_days = || async {
        let resp = health_handler().await.into_response();
        let body = to_bytes(resp.into_body(), TEST_BODY_LIMIT).await.unwrap();
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["trash_keep_days"].clone()
    };
    unsafe { std::env::remove_var("TRASH_KEEP_DAYS") };
    assert_eq!(keep_days().await, 30);
    unsafe { std::env::set_var("TRASH_KEEP_DAYS", "7") };
    assert_eq!(keep_days().await, 7);
    unsafe { std::env::set_var("TRASH_KEEP_DAYS", "nope") };
    assert_eq!(keep_days().await, 30);
    unsafe { std::env::remove_var("TRASH_KEEP_DAYS") };
}

#[tokio::test]
#[serial]
async fn test_health_reports_many_users() {
    init_db_once();
    let many = || async {
        let resp = health_handler().await.into_response();
        let body = to_bytes(resp.into_body(), TEST_BODY_LIMIT).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        (v["many_users_px"].clone(), v["many_users_color"].clone(), v["bar_empty_color"].clone())
    };
    unsafe { std::env::remove_var("MANY_USERS_PX") };
    unsafe { std::env::remove_var("MANY_USERS_COLOR") };
    unsafe { std::env::remove_var("BAR_EMPTY_COLOR") };
    let (px, color, empty) = many().await;
    assert_eq!(px, 2.0);
    assert_eq!(color, "#D2CCBC");
    assert_eq!(empty, "#262626");
    unsafe { std::env::set_var("MANY_USERS_PX", "6") };
    unsafe { std::env::set_var("MANY_USERS_COLOR", "#FFFFFF") };
    unsafe { std::env::set_var("BAR_EMPTY_COLOR", "#112233") };
    let (px, color, empty) = many().await;
    assert_eq!(px, 6.0);
    assert_eq!(color, "#FFFFFF");
    assert_eq!(empty, "#112233");
    unsafe { std::env::set_var("MANY_USERS_PX", "nope") };
    unsafe { std::env::set_var("MANY_USERS_COLOR", "red;x:y") };
    unsafe { std::env::set_var("BAR_EMPTY_COLOR", "gray") };
    let (px, color, empty) = many().await;
    assert_eq!(px, 2.0);
    assert_eq!(color, "#D2CCBC");
    assert_eq!(empty, "#262626");
    unsafe { std::env::remove_var("MANY_USERS_PX") };
    unsafe { std::env::remove_var("MANY_USERS_COLOR") };
    unsafe { std::env::remove_var("BAR_EMPTY_COLOR") };
}
