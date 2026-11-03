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

use axum::{
    extract::Query,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use dutopia::auth::{AuthError, Claims};
use dutopia::authz::path_in_roots;
use dutopia::filesystems::fs_of;

use crate::cleanup::{validate, PathItem};
use crate::query::normalize_path;
use crate::trash_history::{keep_too_late, latest_for, readd_blocked, Outcome};
#[cfg(test)]
pub use crate::trash_store::save;
pub use crate::trash_store::{load, trash_dir};
use crate::trash_store::{with_both, with_queue, Changed};

/// Default days a moved item stays in `<fs>/.trash` before the cron purges it.
pub const DEFAULT_KEEP_DAYS: i64 = 30;

/// `TRASH_KEEP_DAYS` (default 30): how long moved items stay in .trash. The
/// cron purges with it (from /api/health) unless given `--keep-days`, and
/// the UI and the Notify email quote it.
pub fn keep_days() -> i64 {
    std::env::var("TRASH_KEEP_DAYS")
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|d| *d >= 0)
        .unwrap_or(DEFAULT_KEEP_DAYS)
}

/// Default undo window: entries wait this long before the cron may claim them.
pub const DEFAULT_UNDO_HOURS: i64 = 24;
#[cfg(test)]
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
    // FILESYSTEMS are Unix mount points; a backslash would switch normalize_path
    // to Windows rules and let `/./.trash` or `/.` slip past check_target.
    if req.path.contains('\\') {
        return bad_request(&claims.sub, format!("invalid path (backslash): {}", req.path));
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
    let built_at = crate::db_version().and_then(|v| v.parse::<i64>().ok());
    let res = with_both(move |q, history| {
        if let Some(why) = readd_blocked(latest_for(history, &entry.path, &claims), built_at) {
            return (Err((StatusCode::CONFLICT, why)), Changed::default());
        }
        match place(q, entry, &claims) {
            Ok((code, e)) => {
                let queue = code == StatusCode::CREATED;
                (Ok((code, e)), Changed { queue, history: false })
            }
            Err(err) => (Err(err), Changed::default()),
        }
    })
    .await;
    match res {
        Ok(Ok((code, e))) => {
            tracing::info!(actor = %actor, path = %e.path, owner = %e.owner, "{code} POST /api/trash");
            (code, Json(e)).into_response()
        }
        Ok(Err((code, msg))) => {
            tracing::warn!(actor = %actor, %msg, "{code} POST /api/trash");
            (code, msg).into_response()
        }
        Err(resp) => resp,
    }
}

/// Put `entry` in the queue `q`. 200 with the existing entry when the path
/// is already queued; 409 when it lies inside a queued folder, or when that
/// entry is someone else's. Queued children of a new folder are absorbed
/// into it (one move instead of several) when they have the same owner and
/// filesystem; any other child is a 409 (absorbing it would drop someone
/// else's request). Messages name a conflicting path only when the caller
/// may manage it.
fn place(
    q: &mut Vec<Entry>,
    entry: Entry,
    claims: &Claims,
) -> Result<(StatusCode, Entry), (StatusCode, String)> {
    let inside = |child: &str, parent: &str| path_in_roots(std::slice::from_ref(&parent.to_string()), child);
    let conflict = |msg: String| Err((StatusCode::CONFLICT, msg));
    if let Some(e) = q.iter().find(|e| e.path == entry.path) {
        return if can_manage(claims, &e.owner, &e.path) {
            Ok((StatusCode::OK, e.clone()))
        } else {
            conflict("already queued by someone else".into())
        };
    }
    if let Some(p) = q.iter().find(|e| inside(&entry.path, &e.path)) {
        return conflict(if can_manage(claims, &p.owner, &p.path) {
            format!("already in the trash: inside queued {}", p.path)
        } else {
            "a folder above it is already queued for the trash".into()
        });
    }
    let children: Vec<&Entry> = q.iter().filter(|e| inside(&e.path, &entry.path)).collect();
    if let Some(c) = children.iter().find(|c| c.owner != entry.owner || c.fs != entry.fs) {
        return conflict(if can_manage(claims, &c.owner, &c.path) {
            format!("{} is queued for another owner or filesystem: keep it first", c.path)
        } else {
            "something inside it is queued by someone else".into()
        });
    }
    if q.len() + 1 - children.len() > MAX_ENTRIES {
        return Err((StatusCode::INSUFFICIENT_STORAGE, format!("trash queue is full ({MAX_ENTRIES} items)")));
    }
    q.retain(|e| !inside(&e.path, &entry.path));
    q.push(entry.clone());
    Ok((StatusCode::CREATED, entry))
}

enum KeepOutcome {
    Kept(Entry),
    /// Not queued: the answer explains what became of it.
    Gone(Response),
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
    let now = Utc::now().timestamp();
    let res = with_both(move |queue, history| match queue.iter().position(|e| e.path == path) {
        None => {
            let answer = keep_too_late(latest_for(history, &path, &claims));
            (KeepOutcome::Gone(answer), Changed::default())
        }
        Some(i) if !can_manage(&claims, &queue[i].owner, &queue[i].path) => {
            (KeepOutcome::Forbidden, Changed::default())
        }
        Some(i) => {
            let e = queue.remove(i);
            let who = format!("kept by {}", claims.sub);
            history.push(Outcome::new(e.clone(), "kept", who, now));
            (KeepOutcome::Kept(e), Changed { queue: true, history: true })
        }
    })
    .await;
    match res {
        Ok(KeepOutcome::Kept(e)) => {
            tracing::info!(actor = %actor, path = %e.path, "200 OK DELETE /api/trash (kept)");
            Json(e).into_response()
        }
        Ok(KeepOutcome::Gone(answer)) => answer,
        Ok(KeepOutcome::Forbidden) => {
            tracing::warn!(actor = %actor, "403 Forbidden DELETE /api/trash");
            AuthError::Forbidden.into_response()
        }
        Err(resp) => resp,
    }
}

#[cfg(test)]
#[path = "trash_tests.rs"]
pub(crate) mod tests;
