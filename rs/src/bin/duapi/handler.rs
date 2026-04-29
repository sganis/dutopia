// rs/src/bin/duapi/handler.rs
use axum::{
    extract::Query,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use jsonwebtoken::{encode, Header};
use std::collections::HashSet;
use std::time::SystemTime;

use dutopia::auth::{keys, AuthBody, AuthError, AuthPayload, Claims};

use dutopia::db;
use dutopia::item::get_items;
use crate::email;
use crate::query::{parse_users_csv, FilesQuery, FolderQuery};
use crate::{get_db, get_users};

/// GET /api/health
///
/// `smtp_configured` tells the frontend whether the Notify button in the
/// cleanup panel should be enabled — without this probe the button would
/// always render and then fail on click with a 501.
pub async fn health_handler() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "smtp_configured": email::is_configured(),
    }))
}

/// POST /api/login
pub async fn login_handler(Json(payload): Json<AuthPayload>) -> Result<Json<AuthBody>, AuthError> {
    if payload.username.is_empty() || payload.password.is_empty() {
        tracing::warn!("login rejected: missing credentials");
        return Err(AuthError::MissingCredentials);
    }

    let verified = dutopia::auth::verify_credentials(&payload.username, &payload.password);
    if !verified.authenticated {
        tracing::warn!(user = %payload.username, "login rejected: wrong credentials");
        return Err(AuthError::WrongCredentials);
    }

    const TTL_SECONDS: u64 = 24 * 60 * 60;
    let exp_secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| AuthError::TokenCreation)?
        .as_secs()
        + TTL_SECONDS;
    let exp: usize = exp_secs.try_into().map_err(|_| AuthError::TokenCreation)?;

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
        exp,
    };
    tracing::info!(user = %claims.sub, is_admin = claims.is_admin, "login success");

    let token = encode(&Header::default(), &claims, &keys().encoding)
        .map_err(|_| AuthError::TokenCreation)?;

    Ok(Json(AuthBody::new(token)))
}

/// GET /api/users
pub async fn users_handler(claims: Claims) -> Response {
    if claims.is_admin {
        let users = get_users();
        tracing::info!(count = users.len(), "200 OK /api/users");
        Json(users).into_response()
    } else {
        tracing::info!(user = %claims.sub, "200 OK /api/users (self)");
        Json([claims.sub]).into_response()
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
#[path = "handler_tests.rs"]
mod tests;
