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

use crate::item::get_items;
use crate::query::{parse_users_csv, FilesQuery, FolderQuery};
use crate::{get_fs_index, get_users};

/// POST /api/login
pub async fn login_handler(Json(payload): Json<AuthPayload>) -> Result<Json<AuthBody>, AuthError> {
    if payload.username.is_empty() || payload.password.is_empty() {
        return Err(AuthError::MissingCredentials);
    }

    if !dutopia::auth::platform::verify_user(&payload.username, &payload.password) {
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

    let claims = Claims {
        sub: payload.username.to_owned(),
        is_admin: admins.contains(&payload.username.trim().to_ascii_lowercase()),
        exp: exp.try_into().unwrap(),
    };
    println!("login success: {:?}", &claims);

    let token = encode(&Header::default(), &claims, &keys().encoding)
        .map_err(|_| AuthError::TokenCreation)?;

    Ok(Json(AuthBody::new(token)))
}

/// GET /api/users
pub async fn users_handler(claims: Claims) -> impl IntoResponse {
    if claims.is_admin {
        let users = get_users().clone();
        println!("200 OK /api/users count={}", users.len());
        Json(users)
    } else {
        let me = vec![claims.sub.clone()];
        println!("200 OK /api/users self={:?}", me);
        Json(me)
    }
}

/// GET /api/folders?path=/some/dir&users=alice,bob&age=1
pub async fn get_folders_handler(
    claims: Claims,
    Query(q): Query<FolderQuery>,
) -> impl IntoResponse {
    let mut path = q.path.unwrap_or_else(|| "/".to_string());
    if path.is_empty() {
        path = "/".to_string();
    }
    if !path.starts_with('/') {
        path = format!("/{}", path);
    }

    let requested: Vec<String> = match q.users.as_deref() {
        Some(s) if !s.trim().is_empty() => parse_users_csv(s),
        _ => Vec::new(),
    };

    if !claims.is_admin {
        if requested.is_empty() || requested.len() != 1 || requested[0] != claims.sub {
            println!(
                "403 Forbidden /api/folders path={} requested_users={:?}",
                path, requested
            );
            return AuthError::Forbidden.into_response();
        }
    }

    let index = get_fs_index();
    let items = match index.list_children(&path, &requested, q.age) {
        Ok(v) => {
            println!("200 OK /api/folders path={} items={}", path, v.len());
            v
        }
        Err(e) => {
            println!(
                "list_children ERROR /api/folders path={} err={}",
                path, e
            );
            Vec::new()
        }
    };

    Json(items).into_response()
}

/// GET /api/files?path=/some/dir&users=alice,bob&age=1
pub async fn get_files_handler(claims: Claims, Query(q): Query<FilesQuery>) -> impl IntoResponse {
    let folder = match q.path.as_deref() {
        Some(p) if !p.is_empty() => p.to_string(),
        _ => {
            println!("400 Bad Request /api/files missing 'path'");
            return (StatusCode::BAD_REQUEST, "missing 'path' query parameter").into_response();
        }
    };

    let requested: Vec<String> = match q.users.as_deref() {
        Some(s) if !s.trim().is_empty() => parse_users_csv(s),
        _ => Vec::new(),
    };

    if !claims.is_admin {
        if requested.is_empty() || requested.len() != 1 || requested[0] != claims.sub {
            println!(
                "403 Forbidden /api/files path={} requested_users={:?}",
                folder, requested
            );
            return AuthError::Forbidden.into_response();
        }
    }

    let age = q.age;

    let fut = tokio::task::spawn_blocking(move || get_items(folder, &requested, age));

    match fut.await {
        Err(join_err) => {
            println!("500 Task Join Error /api/files err={join_err}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("task error: {join_err}"),
            )
                .into_response()
        }
        Ok(Err(e)) => {
            #[cfg(not(unix))]
            {
                println!("501 Not Implemented /api/files err={}", e);
                (StatusCode::NOT_IMPLEMENTED, e.to_string()).into_response()
            }
            #[cfg(unix)]
            {
                println!("400 Bad Request /api/files err={}", e);
                (StatusCode::BAD_REQUEST, e.to_string()).into_response()
            }
        }
        Ok(Ok(items)) => {
            println!("200 OK /api/files items={}", items.len());
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
    use std::io::Write;
    use tempfile::{tempdir, NamedTempFile};

    use crate::index::{FolderOut, InMemoryFSIndex};
    use crate::item::FsItemOut;
    use crate::{FS_INDEX, USERS};

    const TEST_BODY_LIMIT: usize = 2 * 1024 * 1024;

    fn init_index_once() {
        if FS_INDEX.get().is_some() {
            return;
        }

        let mut f = NamedTempFile::new().expect("tmp csv");
        writeln!(
            f,
            "path,user,age,files,size,disk,linked,atime,mtime\n\
             /,alice,0,2,200,100,0,1700000000,1700000100\n\
             /,bob,1,1,50,50,0,1600000000,1600000100\n\
             /docs,alice,2,3,600,300,300,1500000000,1500000050"
        )
        .unwrap();
        let p = f.into_temp_path();

        let mut idx = InMemoryFSIndex::new();
        let users = idx.load_from_csv(p.as_ref()).expect("load_from_csv");
        FS_INDEX.set(idx).expect("FS_INDEX set once");
        USERS.set(users).expect("USERS set once");
    }

    #[tokio::test]
    #[serial]
    async fn test_users_handler_admin_and_user() {
        init_index_once();
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
        init_index_once();

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
        init_index_once();
        let idx = FS_INDEX.get().unwrap();

        let items = idx.list_children("/", &Vec::new(), None).unwrap();
        assert!(items.iter().any(|it| it.path == "/docs"));

        let items_alice = idx
            .list_children("/", &vec!["alice".into()], None)
            .unwrap();
        assert!(items_alice.iter().any(|it| it.path == "/docs"));
        let docs = items_alice
            .into_iter()
            .find(|it| it.path == "/docs")
            .unwrap();
        assert!(docs.users.contains_key("alice"));

        let items_age2 = idx.list_children("/", &Vec::new(), Some(2)).unwrap();
        let docs2 = items_age2
            .into_iter()
            .find(|it| it.path == "/docs")
            .unwrap();
        let alice_ages = docs2.users.get("alice").unwrap();
        assert!(alice_ages.contains_key("2"));
    }
}
