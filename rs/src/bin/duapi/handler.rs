// rs/src/bin/duapi/handler.rs
use axum::{
    extract::Query,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use jsonwebtoken::{encode, Header};
use std::collections::HashSet;
use std::time::SystemTime;

use dutopia::auth::{keys, AuthBody, AuthError, AuthPayload, Claims};

use dutopia::db;
use dutopia::item::get_items;
use crate::query::{parse_users_csv, FilesQuery, FolderQuery};
use crate::{get_db, get_users};

/// GET /api/health
pub async fn health_handler() -> impl IntoResponse {
    Json(serde_json::json!({"status": "ok"}))
}

/// POST /api/login
pub async fn login_handler(Json(payload): Json<AuthPayload>) -> Result<Json<AuthBody>, AuthError> {
    if payload.username.is_empty() || payload.password.is_empty() {
        return Err(AuthError::MissingCredentials);
    }

    let verified = dutopia::auth::verify_credentials(&payload.username, &payload.password);
    if !verified.authenticated {
        return Err(AuthError::WrongCredentials);
    }

    const TTL_SECONDS: u64 = 24 * 60 * 60;
    let exp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + TTL_SECONDS;

    let admins: HashSet<String> = std::env::var("ADMIN_GROUP")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();

    let is_admin =
        verified.admin_override || admins.contains(&payload.username.trim().to_ascii_lowercase());

    let claims = Claims {
        sub: payload.username.to_owned(),
        is_admin,
        exp: exp.try_into().unwrap(),
    };
    tracing::info!(user = %claims.sub, is_admin = claims.is_admin, "login success");

    let token = encode(&Header::default(), &claims, &keys().encoding)
        .map_err(|_| AuthError::TokenCreation)?;

    Ok(Json(AuthBody::new(token)))
}

/// GET /api/users
pub async fn users_handler(claims: Claims) -> impl IntoResponse {
    if claims.is_admin {
        let users = get_users().clone();
        tracing::info!(count = users.len(), "200 OK /api/users");
        Json(users)
    } else {
        let me = vec![claims.sub.clone()];
        tracing::info!(user = %claims.sub, "200 OK /api/users (self)");
        Json(me)
    }
}

/// GET /api/folders?path=/some/dir&users=alice,bob&age=1
pub async fn get_folders_handler(
    claims: Claims,
    Query(q): Query<FolderQuery>,
) -> impl IntoResponse {
    let raw_path = q.path.unwrap_or_default();
    let path = match crate::query::normalize_path(&raw_path) {
        Some(p) => p,
        None => {
            tracing::warn!(input = %raw_path, "400 Bad Request /api/folders rejected path");
            return (StatusCode::BAD_REQUEST, "invalid path").into_response();
        }
    };

    let requested: Vec<String> = match q.users.as_deref() {
        Some(s) if !s.trim().is_empty() => parse_users_csv(s),
        _ => Vec::new(),
    };

    if !claims.is_admin {
        if requested.is_empty() || requested.len() != 1 || requested[0] != claims.sub {
            tracing::warn!(
                path = %path,
                requested_users = ?requested,
                "403 Forbidden /api/folders"
            );
            return AuthError::Forbidden.into_response();
        }
    }

    let pool = get_db().clone();
    let path_for_task = path.clone();
    let age_filter = q.age;
    let fut = tokio::task::spawn_blocking(move || {
        db::list_children(&pool, &path_for_task, &requested, age_filter)
    });

    let items = match fut.await {
        Err(join_err) => {
            tracing::error!(err = %join_err, "500 Task Join Error /api/folders");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("task error: {join_err}"),
            )
                .into_response();
        }
        Ok(Ok(mut v)) => {
            let cap = crate::query::max_page_size();
            if v.len() > cap {
                tracing::warn!(path = %path, total = v.len(), cap, "/api/folders truncated");
                v.truncate(cap);
            }
            tracing::info!(path = %path, items = v.len(), "200 OK /api/folders");
            v
        }
        Ok(Err(e)) => {
            tracing::warn!(path = %path, err = %e, "list_children ERROR /api/folders");
            Vec::new()
        }
    };

    Json(items).into_response()
}

/// GET /api/files?path=/some/dir&users=alice,bob&age=1
pub async fn get_files_handler(claims: Claims, Query(q): Query<FilesQuery>) -> impl IntoResponse {
    let folder = match q.path.as_deref() {
        None => {
            tracing::warn!("400 Bad Request /api/files missing 'path'");
            return (StatusCode::BAD_REQUEST, "missing 'path' query parameter").into_response();
        }
        Some(raw) => match crate::query::normalize_path(raw) {
            None => {
                tracing::warn!(input = %raw, "400 Bad Request /api/files rejected path");
                return (StatusCode::BAD_REQUEST, "invalid path").into_response();
            }
            Some(p) if p == "/" => {
                tracing::warn!("400 Bad Request /api/files path '/' not allowed");
                return (
                    StatusCode::BAD_REQUEST,
                    "path '/' not allowed for /api/files",
                )
                    .into_response();
            }
            Some(p) => p,
        },
    };

    let requested: Vec<String> = match q.users.as_deref() {
        Some(s) if !s.trim().is_empty() => parse_users_csv(s),
        _ => Vec::new(),
    };

    if !claims.is_admin {
        if requested.is_empty() || requested.len() != 1 || requested[0] != claims.sub {
            tracing::warn!(
                path = %folder,
                requested_users = ?requested,
                "403 Forbidden /api/files"
            );
            return AuthError::Forbidden.into_response();
        }
    }

    let age = q.age;

    let fut = tokio::task::spawn_blocking(move || get_items(folder, &requested, age));

    match fut.await {
        Err(join_err) => {
            tracing::error!(err = %join_err, "500 Task Join Error /api/files");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("task error: {join_err}"),
            )
                .into_response()
        }
        Ok(Err(e)) => {
            #[cfg(not(unix))]
            {
                tracing::warn!(err = %e, "501 Not Implemented /api/files");
                (StatusCode::NOT_IMPLEMENTED, e.to_string()).into_response()
            }
            #[cfg(unix)]
            {
                tracing::warn!(err = %e, "400 Bad Request /api/files");
                (StatusCode::BAD_REQUEST, e.to_string()).into_response()
            }
        }
        Ok(Ok(mut items)) => {
            let cap = crate::query::max_page_size();
            if items.len() > cap {
                tracing::warn!(total = items.len(), cap, "/api/files truncated");
                items.truncate(cap);
            }
            tracing::info!(items = items.len(), "200 OK /api/files");
            Json(items).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::extract::Query;
    use serial_test::serial;
    #[cfg(unix)]
    use tempfile::tempdir;

    use dutopia::db::FolderOut;
    #[cfg(unix)]
    use dutopia::item::FsItemOut;
    use crate::{DB_POOL, TEST_DB, USERS};

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
}
