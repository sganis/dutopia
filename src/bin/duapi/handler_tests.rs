// rs/src/bin/duapi/handler_tests.rs
use super::*;
use axum::body::to_bytes;
use axum::extract::Query;
use serial_test::serial;
#[cfg(unix)]
use tempfile::tempdir;

use crate::{DB_POOL, TEST_DB, USERS};
use dutopia::db::FolderOut;
#[cfg(unix)]
use dutopia::item::FsItemOut;

const TEST_BODY_LIMIT: usize = 2 * 1024 * 1024;

fn init_db_once() {
    if DB_POOL.get().is_some() {
        return;
    }
    let temp_db = dutopia::db::test_support::build_test_db();
    let pool = dutopia::db::open_pool(&temp_db.path).expect("open_pool");
    let users = dutopia::db::list_users(&pool).expect("list_users");
    // Keep the TempDb alive for the entire test run so the file is not
    // removed while the pool is still using it.
    let _ = TEST_DB.set(temp_db);
    let _ = DB_POOL.set(pool);
    let _ = USERS.set(users);
}

#[tokio::test]
#[serial]
async fn test_users_handler_admin_and_user() {
    init_db_once();
    let admin = Claims {
        sub: "root".to_string(),
        is_admin: true,
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

#[cfg(unix)]
#[tokio::test]
async fn test_get_files_handler_unix_admin_ok() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("a.txt");
    std::fs::write(&file_path, b"hi").unwrap();

    let claims = Claims {
        sub: "root".into(),
        is_admin: true,
        exp: 9_999_999_999usize,
    };
    let q = FilesQuery {
        path: Some(dir.path().to_string_lossy().into()),
        users: None,
        age: None,
    };

    let resp = get_files_handler(claims, Query(q)).await.into_response();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), TEST_BODY_LIMIT).await.unwrap();
    let items: Vec<FsItemOut> = serde_json::from_slice(&body).unwrap();
    assert_eq!(items.len(), 1);
    assert!(items[0].path.ends_with("a.txt"));
}

#[cfg(unix)]
#[tokio::test]
async fn test_get_files_handler_unix_non_admin_forbidden() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("b.txt");
    std::fs::write(&file_path, b"hi").unwrap();

    let claims = Claims {
        sub: "alice".into(),
        is_admin: false,
        exp: 9_999_999_999usize,
    };
    let q = FilesQuery {
        path: Some(dir.path().to_string_lossy().into()),
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
    let pool = DB_POOL.get().unwrap();

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
async fn test_health_handler() {
    let resp = health_handler().await.into_response();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), TEST_BODY_LIMIT).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["status"], "ok");
    assert!(v["smtp_configured"].is_boolean());
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
