// rs/src/bin/duapi/trash.rs
//
// The trash cart, kept on the server so the nightly cron can see it.
//
//   UI trash icon → POST   /api/trash          adds an entry to queue.json
//   UI Keep       → DELETE /api/trash?path=    removes it (the 1-day undo)
//   UI panel      → GET    /api/trash          entries the caller may manage
//   cron 02:00    → POST   /api/trash/claim    (Bearer $TRASH_TOKEN) entries
//                    queued over 24h ago, removed from the queue; the cron
//                    moves them to <fs>/.trash/<date>/… and purges later.
//
// The queue is `$TRASH_DIR/queue.json` (default /data/trash, the PVC), one
// JSON array guarded by a process-wide mutex and replaced by tmp + rename.

use anyhow::Context;
use axum::{
    extract::Query,
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use dutopia::auth::{AuthError, Claims};
use dutopia::authz::path_in_roots;
use dutopia::filesystems::fs_of;

use crate::cleanup::{validate, PathItem};
use crate::query::normalize_path;

/// Default undo window: entries wait this long before the cron may claim them.
pub const DEFAULT_UNDO_HOURS: i64 = 24;
pub const CLAIM_AFTER_SECS: i64 = DEFAULT_UNDO_HOURS * 60 * 60;

/// The undo window in hours: `TRASH_UNDO_HOURS` (0 claims at once, for local
/// testing), default 24. Read on every call so tests can vary it.
pub fn undo_hours() -> i64 {
    std::env::var("TRASH_UNDO_HOURS")
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|h| *h >= 0)
        .unwrap_or(DEFAULT_UNDO_HOURS)
}
/// Bound on queue.json so a runaway client cannot grow it without limit.
const MAX_ENTRIES: usize = 20_000;
const QUEUE_FILE: &str = "queue.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub path: String,
    pub size: u64,
    /// Owning username: the cron skips the item if it no longer owns it.
    pub owner: String,
    /// Who queued it (`claims.sub`).
    pub actor: String,
    /// The FILESYSTEMS entry holding `path`: the trash is `<fs>/.trash`.
    pub fs: String,
    /// Unix seconds.
    pub queued_at: i64,
}

#[derive(Debug, Deserialize)]
pub struct AddReq {
    pub path: String,
    #[serde(default)]
    pub size: u64,
    pub owner: String,
}

#[derive(Debug, Deserialize)]
pub struct KeepQuery {
    pub path: Option<String>,
}

/// May `claims` add, keep or see an item of `owner` at `path`? Same tiers as
/// `authz`: global admins any item; storage admins items inside their roots
/// plus their own; regular users only their own.
pub fn can_manage(claims: &Claims, owner: &str, path: &str) -> bool {
    claims.is_admin
        || (!claims.roots.is_empty() && path_in_roots(&claims.roots, path))
        || owner == claims.sub
}

/// Check a normalized `path` can be trashed and return its filesystem: it
/// must be inside a FILESYSTEMS entry, not the filesystem root itself, and
/// not inside `<fs>/.trash` or `<fs>/.stats`.
pub fn check_target(path: &str, filesystems: &[String]) -> Result<String, String> {
    let fs = fs_of(filesystems, path)
        .ok_or_else(|| format!("not inside a filesystem listed in FILESYSTEMS: {path}"))?;
    if path == fs {
        return Err(format!("cannot trash a filesystem root: {path}"));
    }
    for reserved in [".trash", ".stats"] {
        let dir = if fs == "/" { format!("/{reserved}") } else { format!("{fs}/{reserved}") };
        if path_in_roots(std::slice::from_ref(&dir), path) {
            return Err(format!("cannot trash inside {dir}"));
        }
    }
    Ok(fs.to_string())
}

/// Owner used by dusum/dudb for a UID with no account: the cron cannot
/// check such an owner, so it is never queued.
const UNKNOWN_OWNER: &str = "UNK";

/// `rows` is `db::folder_stats` of the item: when the item is a folder in
/// the DB, it must hold only `owner`'s files. Returns the reason otherwise.
/// No row (a file, or a folder the scan has not seen) is left to the cron.
pub fn owner_conflict(rows: &[dutopia::db::FolderOut], owner: &str) -> Option<String> {
    let row = rows.first()?;
    if row.users.len() == 1 && row.users.contains_key(owner) {
        return None;
    }
    let mut others: Vec<&str> =
        row.users.keys().map(String::as_str).filter(|u| *u != owner).collect();
    others.sort_unstable();
    Some(format!(
        "folder holds files of other users ({}): only single-owner folders can be trashed",
        others.join(", ")
    ))
}

/// `owner_conflict` against the DB, when one is loaded (else the cron's own
/// ownership walk is the only check). Err is a ready 409/500 response.
async fn check_db_owner(path: &str, owner: &str, actor: &str) -> Result<(), Response> {
    let Some(pool) = crate::get_db().cloned() else { return Ok(()) };
    let p = path.to_string();
    let rows = tokio::task::spawn_blocking(move || dutopia::db::folder_stats(&pool, &[p], &[], None))
        .await;
    match rows {
        Ok(Ok(rows)) => match owner_conflict(&rows, owner) {
            None => Ok(()),
            Some(msg) => {
                tracing::warn!(actor = %actor, %path, %msg, "409 POST /api/trash");
                Err((StatusCode::CONFLICT, msg).into_response())
            }
        },
        Ok(Err(e)) => {
            tracing::error!(err = %e, "500 folder_stats ERROR POST /api/trash");
            Err((StatusCode::INTERNAL_SERVER_ERROR, format!("folder_stats error: {e}")).into_response())
        }
        Err(join_err) => {
            tracing::error!(err = %join_err, "500 Task Join Error POST /api/trash");
            Err((StatusCode::INTERNAL_SERVER_ERROR, "task error").into_response())
        }
    }
}

/// Split `entries` into (claimed, kept): claimed are those queued at least
/// `after_secs` before `now`.
pub fn split_due(entries: Vec<Entry>, now: i64, after_secs: i64) -> (Vec<Entry>, Vec<Entry>) {
    entries
        .into_iter()
        .partition(|e| now - e.queued_at >= after_secs)
}

/// Load the queue (missing file = empty).
pub fn load(dir: &Path) -> anyhow::Result<Vec<Entry>> {
    let file = dir.join(QUEUE_FILE);
    match std::fs::read_to_string(&file) {
        Ok(text) => serde_json::from_str(&text)
            .with_context(|| format!("parsing {}", file.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", file.display())),
    }
}

/// Replace the queue atomically (write tmp, fsync, rename).
pub fn save(dir: &Path, entries: &[Entry]) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let file = dir.join(QUEUE_FILE);
    let tmp = dir.join(format!("{QUEUE_FILE}.tmp"));
    {
        let mut f = std::fs::File::create(&tmp)
            .with_context(|| format!("creating {}", tmp.display()))?;
        serde_json::to_writer_pretty(&mut f, entries)?;
        f.write_all(b"\n")?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &file)
        .with_context(|| format!("renaming {} to {}", tmp.display(), file.display()))
}

pub fn trash_dir() -> PathBuf {
    std::env::var("TRASH_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/data/trash"))
}

/// One writer at a time on queue.json (one duapi pod).
static LOCK: Mutex<()> = Mutex::new(());

/// Run `f` on the loaded queue under the lock, in a blocking task; save the
/// queue when `f` reports a change. IO errors become a 500 response.
async fn with_queue<T, F>(f: F) -> Result<T, Response>
where
    F: FnOnce(&mut Vec<Entry>) -> (T, bool) + Send + 'static,
    T: Send + 'static,
{
    let dir = trash_dir();
    let res = tokio::task::spawn_blocking(move || -> anyhow::Result<T> {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut queue = load(&dir)?;
        let (out, changed) = f(&mut queue);
        if changed {
            save(&dir, &queue)?;
        }
        Ok(out)
    })
    .await;
    match res {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => {
            tracing::error!(err = %format!("{e:#}"), "500 trash queue error");
            Err((StatusCode::INTERNAL_SERVER_ERROR, format!("trash queue error: {e:#}"))
                .into_response())
        }
        Err(join_err) => {
            tracing::error!(err = %join_err, "500 Task Join Error /api/trash");
            Err((StatusCode::INTERNAL_SERVER_ERROR, "task error").into_response())
        }
    }
}

fn bad_request(actor: &str, msg: String) -> Response {
    tracing::warn!(actor = %actor, %msg, "400 Bad Request /api/trash");
    (StatusCode::BAD_REQUEST, msg).into_response()
}

/// GET /api/trash — the entries the caller may manage.
pub async fn list_handler(claims: Claims) -> Response {
    let actor = claims.sub.clone();
    let visible = with_queue(move |q| {
        let v: Vec<Entry> = q
            .iter()
            .filter(|e| can_manage(&claims, &e.owner, &e.path))
            .cloned()
            .collect();
        (v, false)
    })
    .await;
    match visible {
        Ok(v) => {
            tracing::info!(actor = %actor, items = v.len(), "200 OK GET /api/trash");
            Json(v).into_response()
        }
        Err(resp) => resp,
    }
}

/// POST /api/trash — queue `{path, size, owner}`. 201 with the new entry;
/// 200 with the existing one when the path is already queued (the first
/// `queued_at` stands).
pub async fn add_handler(claims: Claims, Json(req): Json<AddReq>) -> Response {
    let item = [PathItem { path: req.path.clone(), size: req.size }];
    if let Err((code, msg)) = validate(&req.owner, &item) {
        tracing::warn!(actor = %claims.sub, %msg, "{code} POST /api/trash invalid");
        return (code, msg).into_response();
    }
    let path = match normalize_path(&req.path) {
        Some(p) if p.starts_with('/') => p,
        _ => return bad_request(&claims.sub, format!("invalid path: {}", req.path)),
    };
    let fs = match check_target(&path, &crate::filesystems()) {
        Ok(fs) => fs,
        Err(msg) => return bad_request(&claims.sub, msg),
    };
    let owner = req.owner.trim().to_string();
    if owner == UNKNOWN_OWNER {
        return bad_request(
            &claims.sub,
            format!("owner {UNKNOWN_OWNER} is a UID with no account: it cannot be trashed"),
        );
    }
    if !can_manage(&claims, &owner, &path) {
        tracing::warn!(actor = %claims.sub, %owner, %path, "403 Forbidden POST /api/trash");
        return AuthError::Forbidden.into_response();
    }
    if let Err(resp) = check_db_owner(&path, &owner, &claims.sub).await {
        return resp;
    }
    let entry = Entry {
        path,
        size: req.size,
        owner,
        actor: claims.sub.clone(),
        fs,
        queued_at: Utc::now().timestamp(),
    };
    let actor = claims.sub.clone();
    let res = with_queue(move |q| {
        if let Some(e) = q.iter().find(|e| e.path == entry.path) {
            return if can_manage(&claims, &e.owner, &e.path) {
                ((StatusCode::OK, Some(e.clone())), false)
            } else {
                ((StatusCode::CONFLICT, None), false)
            };
        }
        if q.len() >= MAX_ENTRIES {
            return ((StatusCode::INSUFFICIENT_STORAGE, None), false);
        }
        q.push(entry.clone());
        ((StatusCode::CREATED, Some(entry)), true)
    })
    .await;
    match res {
        Ok((code, Some(e))) => {
            tracing::info!(actor = %actor, path = %e.path, owner = %e.owner, "{code} POST /api/trash");
            (code, Json(e)).into_response()
        }
        Ok((code, None)) => {
            tracing::warn!(actor = %actor, "{code} POST /api/trash");
            let msg = if code == StatusCode::CONFLICT {
                "already queued by someone else".to_string()
            } else {
                format!("trash queue is full ({MAX_ENTRIES} items)")
            };
            (code, msg).into_response()
        }
        Err(resp) => resp,
    }
}

enum KeepOutcome {
    Kept(Entry),
    NotFound,
    Forbidden,
}

/// DELETE /api/trash?path= — Keep: take the entry out of the queue.
pub async fn keep_handler(claims: Claims, Query(q): Query<KeepQuery>) -> Response {
    let Some(raw) = q.path.filter(|p| !p.trim().is_empty()) else {
        return bad_request(&claims.sub, "missing 'path' query parameter".into());
    };
    let Some(path) = normalize_path(&raw) else {
        return bad_request(&claims.sub, format!("invalid path: {raw}"));
    };
    let actor = claims.sub.clone();
    let res = with_queue(move |queue| match queue.iter().position(|e| e.path == path) {
        None => (KeepOutcome::NotFound, false),
        Some(i) if !can_manage(&claims, &queue[i].owner, &queue[i].path) => {
            (KeepOutcome::Forbidden, false)
        }
        Some(i) => (KeepOutcome::Kept(queue.remove(i)), true),
    })
    .await;
    match res {
        Ok(KeepOutcome::Kept(e)) => {
            tracing::info!(actor = %actor, path = %e.path, "200 OK DELETE /api/trash (kept)");
            Json(e).into_response()
        }
        Ok(KeepOutcome::NotFound) => (StatusCode::NOT_FOUND, "not in the trash queue").into_response(),
        Ok(KeepOutcome::Forbidden) => {
            tracing::warn!(actor = %actor, "403 Forbidden DELETE /api/trash");
            AuthError::Forbidden.into_response()
        }
        Err(resp) => resp,
    }
}

/// Constant-time byte comparison for the cron token.
fn same_token(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// POST /api/trash/claim — `Authorization: Bearer $TRASH_TOKEN` (not a user
/// JWT). Returns the entries queued over `undo_hours()` ago and removes them from the
/// queue: from here on they belong to the cron. 401 when TRASH_TOKEN is
/// unset or does not match.
pub async fn claim_handler(headers: HeaderMap) -> Response {
    let expected = std::env::var("TRASH_TOKEN")
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    let given = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim);
    match (expected, given) {
        (Some(e), Some(g)) if same_token(e.as_bytes(), g.as_bytes()) => {}
        (None, _) => {
            tracing::warn!("401 POST /api/trash/claim: TRASH_TOKEN is not set");
            return (StatusCode::UNAUTHORIZED, "trash claim disabled").into_response();
        }
        _ => {
            tracing::warn!("401 POST /api/trash/claim: bad token");
            return (StatusCode::UNAUTHORIZED, "invalid trash token").into_response();
        }
    }
    let now = Utc::now().timestamp();
    let after_secs = undo_hours() * 60 * 60;
    let res = with_queue(move |q| {
        let (due, kept) = split_due(std::mem::take(q), now, after_secs);
        *q = kept;
        let changed = !due.is_empty();
        (due, changed)
    })
    .await;
    match res {
        Ok(due) => {
            tracing::info!(items = due.len(), "200 OK POST /api/trash/claim");
            Json(due).into_response()
        }
        Err(resp) => resp,
    }
}

#[cfg(test)]
#[path = "trash_tests.rs"]
mod tests;
