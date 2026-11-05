// rs/src/bin/duapi/trash_claim.rs
//
// The cron side of the trash queue:
//
//   POST /api/trash/claim   (Bearer $TRASH_TOKEN) due entries leave the queue
//                           and go to the cron (?action=archive: the archive
//                           cron's entries; else the trash's)
//
// Nothing is kept once an item leaves the queue: the cron logs what it did
// on its node, duapi logs each claimed path, and the next scan shows the
// item in <fs>/.trash or <fs>/.archive.

use axum::{
    extract::Query,
    http::{header::AUTHORIZATION, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use serde::Deserialize;

use crate::trash::{undo_hours, Action, Entry};
use crate::trash_store::with_queue;

/// The cron's `Authorization: Bearer $TRASH_TOKEN`; Err is a ready 401.
#[allow(clippy::result_large_err)] // Err is the ready axum response, like with_queue's
fn check_cron_token(headers: &HeaderMap, route: &str) -> Result<(), Response> {
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
        (Some(e), Some(g)) if same_token(e.as_bytes(), g.as_bytes()) => Ok(()),
        (None, _) => {
            tracing::warn!("401 POST {route}: TRASH_TOKEN is not set");
            Err((StatusCode::UNAUTHORIZED, "trash cron endpoints disabled").into_response())
        }
        _ => {
            tracing::warn!("401 POST {route}: bad token");
            Err((StatusCode::UNAUTHORIZED, "invalid trash token").into_response())
        }
    }
}

/// Constant-time byte comparison for the cron token.
fn same_token(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Split `entries` into (due, kept): due are those queued at least
/// `after_secs` before `now`.
#[cfg(test)]
pub fn split_due(entries: Vec<Entry>, now: i64, after_secs: i64) -> (Vec<Entry>, Vec<Entry>) {
    entries.into_iter().partition(|e| is_due(e, now, after_secs))
}

fn is_due(e: &Entry, now: i64, after_secs: i64) -> bool {
    now - e.queued_at >= after_secs
}

#[derive(Debug, Deserialize)]
pub struct ClaimQuery {
    /// `?dry_run=1`: return what a claim would take, leave the queue as is.
    pub dry_run: Option<String>,
    /// `?fs=/red/nas1,/red/nas2`: only entries of these filesystems (the
    /// ones the calling cron node mounts). Absent = every filesystem.
    pub fs: Option<String>,
    /// `?action=archive`: the archive cron's entries. Absent = trash, so a
    /// trash cron never takes what was queued for the archive.
    #[serde(default)]
    pub action: Option<String>,
}

/// POST /api/trash/claim — the due entries (queued over `undo_hours()` ago)
/// of the requested filesystems leave the queue: from here on they belong
/// to the cron. Due entries whose filesystem is no longer in FILESYSTEMS
/// are dropped (no node would ever take them). `?dry_run=1` returns the list
/// and changes nothing. Only entries of `?action` (default trash) are
/// claimed or dropped. The reply confirms the options it applied
/// (X-Trash-Dry-Run, X-Trash-Fs-Filter, X-Trash-Action) so a newer cron can
/// tell an older duapi ignored them.
pub async fn claim_handler(headers: HeaderMap, Query(q): Query<ClaimQuery>) -> Response {
    if let Err(resp) = check_cron_token(&headers, "/api/trash/claim") {
        return resp;
    }
    let action = match q.action.as_deref().map(Action::parse) {
        None => Action::Trash,
        Some(Some(a)) => a,
        Some(None) => {
            tracing::warn!(action = ?q.action, "400 POST /api/trash/claim: unknown action");
            return (StatusCode::BAD_REQUEST, "unknown action (expected trash or archive)").into_response();
        }
    };
    let dry_run = matches!(q.dry_run.as_deref().map(str::trim), Some("1" | "true" | "yes" | "on"));
    let only_fs: Option<Vec<String>> = q.fs.as_deref().map(dutopia::filesystems::parse);
    let fs_filter = only_fs.is_some();
    let listed = crate::filesystems();
    let now = Utc::now().timestamp();
    let after_secs = undo_hours() * 60 * 60;
    let res = with_queue(move |queue| {
        let due = |e: &Entry| e.action == action && is_due(e, now, after_secs);
        let wanted = |e: &Entry| only_fs.as_ref().is_none_or(|fss| fss.contains(&e.fs));
        let orphan = |e: &Entry| !listed.contains(&e.fs);
        let take: Vec<Entry> =
            queue.iter().filter(|e| due(e) && !orphan(e) && wanted(e)).cloned().collect();
        if dry_run {
            return (take, false);
        }
        for e in queue.iter().filter(|e| due(e) && orphan(e)) {
            tracing::warn!(path = %e.path, fs = %e.fs, "trash claim dropped an entry: filesystem no longer in FILESYSTEMS");
        }
        let before = queue.len();
        queue.retain(|e| !(due(e) && (orphan(e) || wanted(e))));
        let changed = queue.len() != before;
        (take, changed)
    })
    .await;
    match res {
        Ok(due) => {
            for e in &due {
                if !dry_run {
                    tracing::info!(path = %e.path, owner = %e.owner, actor = %e.actor, action = action.name(), "trash claimed");
                }
            }
            tracing::info!(items = due.len(), dry_run, action = action.name(), "200 OK POST /api/trash/claim");
            let mut resp = Json(due).into_response();
            resp.headers_mut().insert("x-trash-action", HeaderValue::from_static(action.name()));
            if dry_run {
                resp.headers_mut().insert("x-trash-dry-run", HeaderValue::from_static("1"));
            }
            if fs_filter {
                resp.headers_mut().insert("x-trash-fs-filter", HeaderValue::from_static("1"));
            }
            resp
        }
        Err(resp) => resp,
    }
}

#[cfg(test)]
#[path = "trash_claim_tests.rs"]
mod tests;
