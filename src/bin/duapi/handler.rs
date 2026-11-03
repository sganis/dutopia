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
use dutopia::item::{get_items, FilesOut};
use crate::email;
use crate::query::{parse_users_csv, FilesQuery, FolderQuery};
use crate::get_users;

/// GET /api/health
///
/// `smtp_configured` tells the frontend whether the Notify button in the
/// cleanup panel should be enabled — without this probe the button would
/// always render and then fail on click with a 501.
///
/// `db_version` is the DB's `built_at` (probed once at startup, so no
/// per-request query). The frontend uses it to invalidate its browser-side
/// cache when the DB is rebuilt; null means no usable DB is loaded and
/// clients fall back to TTL-only expiry.
///
/// `filesystems` is the `FILESYSTEMS` list: the SPA sends "up" from one of
/// them to the home page and only offers the trash icon inside them.
pub async fn health_handler() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "smtp_configured": email::is_configured(),
        "db_version": crate::db_version(),
        "filesystems": crate::filesystems(),
        "trash_undo_hours": crate::trash::undo_hours(),
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

    let admins: HashSet<String> = std::env::var("GLOBAL_ADMINS")
        .or_else(|_| std::env::var("ADMIN_GROUP")) // deprecated name
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();

    let is_admin =
        verified.admin_override || admins.contains(&payload.username.trim().to_ascii_lowercase());

    // Storage admins keep their roots even when they are also global admins
    // (roots are ignored while is_admin is set).
    let roots = if verified.admin_override {
        Vec::new()
    } else {
        dutopia::authz::roots_for(&payload.username)
    };

    let claims = Claims {
        sub: payload.username.to_owned(),
        is_admin,
        roots,
        exp,
    };
    tracing::info!(user = %claims.sub, is_admin = claims.is_admin, "login success");

    let token = encode(&Header::default(), &claims, &keys().encoding)
        .map_err(|_| AuthError::TokenCreation)?;

    Ok(Json(AuthBody::new(token)))
}

/// GET /api/users
pub async fn users_handler(claims: Claims) -> Response {
    // Storage admins get the full list too: the user picker is how they
    // filter within their assigned storage.
    if claims.is_admin || !claims.roots.is_empty() {
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

    let access = match dutopia::authz::check_access(&claims, &path, &requested) {
        Ok(a) => a,
        Err(_) => {
            tracing::warn!(
                user = %claims.sub,
                path = %path,
                requested_users = ?requested,
                "403 Forbidden /api/folders"
            );
            return AuthError::Forbidden.into_response();
        }
    };
    // Storage admin navigating an ancestor of their roots: drop the
    // requested users (they'd leak sibling-branch stats) and filter the
    // children below to the route into their roots.
    let navigable = access == dutopia::authz::AccessMode::Navigable;
    let requested = if navigable { Vec::new() } else { requested };

    let pool = match crate::get_db() {
        Some(p) => p.clone(),
        None => return crate::db_unavailable(),
    };
    // Home page with FILESYSTEMS set: one row per filesystem, not `/`.
    let filesystems = if path.is_empty() { crate::filesystems() } else { Vec::new() };
    let home = (!filesystems.is_empty())
        .then(|| crate::home::rows_for(&claims, &filesystems, &access, requested.clone()));
    let path_for_task = path.clone();
    let age_filter = q.age;
    let fut = tokio::task::spawn_blocking(move || match home {
        Some((rows, users)) => db::folder_stats(&pool, &rows, &users, age_filter),
        None => db::list_children(&pool, &path_for_task, &requested, age_filter),
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
            // Home rows are already scoped by home::rows_for.
            if navigable && filesystems.is_empty() {
                dutopia::authz::filter_navigable_children(&claims.roots, &mut v);
            }
            let cap = crate::query::max_page_size();
            if v.len() > cap {
                tracing::warn!(path = %path, total = v.len(), cap, "/api/folders truncated");
                v.truncate(cap);
            }
            tracing::info!(path = %path, items = v.len(), "200 OK /api/folders");
            v
        }
        Ok(Err(e)) => {
            tracing::error!(path = %path, err = %e, "500 list_children ERROR /api/folders");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("list_children error: {e}"),
            )
                .into_response();
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

    match dutopia::authz::check_access(&claims, &folder, &requested) {
        Ok(dutopia::authz::AccessMode::Navigable) => {
            // Storage admin walking an ancestor of their roots: there are no
            // files to manage at this level, return an empty listing rather
            // than a 403 so the SPA keeps working while navigating.
            tracing::info!(user = %claims.sub, path = %folder, "200 OK /api/files (navigable)");
            return Json(FilesOut::default()).into_response();
        }
        Ok(dutopia::authz::AccessMode::Full) => {}
        Err(_) => {
            tracing::warn!(
                user = %claims.sub,
                path = %folder,
                requested_users = ?requested,
                "403 Forbidden /api/files"
            );
            return AuthError::Forbidden.into_response();
        }
    }

    let age = q.age;
    let cap = crate::query::max_page_size();

    // `dudb --no-files`: a folders-only DB on purpose; never read the disk.
    if crate::files_disabled() {
        return Json(FilesOut::default()).into_response();
    }

    // Preferred path: the file index packed by `dudb --raw`. In production
    // the API host has no access to the scanned filesystem, so the live
    // read_dir below only serves dev setups / folder-only DBs.
    if crate::files_indexed() {
        let cap = crate::files_limit(cap);
        let pool = match crate::get_db() {
            Some(p) => p.clone(),
            None => return crate::db_unavailable(),
        };
        let folder_for_task = folder.clone();
        let fut = tokio::task::spawn_blocking(move || {
            db::list_files(&pool, &folder_for_task, &requested, age, cap)
        });
        return match fut.await {
            Err(join_err) => {
                tracing::error!(err = %join_err, "500 Task Join Error /api/files");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("task error: {join_err}"),
                )
                    .into_response()
            }
            Ok(Err(e)) => {
                tracing::error!(path = %folder, err = %e, "500 list_files ERROR /api/files");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("list_files error: {e}"),
                )
                    .into_response()
            }
            Ok(Ok(out)) => {
                tracing::info!(items = out.files.len(), "200 OK /api/files");
                Json(out).into_response()
            }
        };
    }

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
        Ok(Ok(items)) => {
            if items.len() > cap {
                tracing::warn!(total = items.len(), cap, "/api/files truncated");
            }
            let out = FilesOut::from_all(items, cap);
            tracing::info!(items = out.files.len(), "200 OK /api/files");
            Json(out).into_response()
        }
    }
}


#[cfg(test)]
#[path = "handler_tests.rs"]
pub(crate) mod tests;
