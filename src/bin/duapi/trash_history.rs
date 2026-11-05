// rs/src/bin/duapi/trash_history.rs
//
// The cron side of the trash, and what it leaves behind.
//
//   POST /api/trash/claim   (Bearer $TRASH_TOKEN) due entries leave the queue
//                           and are recorded as "claimed" (?action=archive:
//                           the archive cron's entries; else the trash's)
//   POST /api/trash/report  (Bearer $TRASH_TOKEN) the cron's result for each:
//                           moved / skipped / error, superseding "claimed"
//   GET  /api/trash/history what happened, as far as the caller may see
//
// history.json (oldest first, capped) also records each Keep ("kept"), so a
// Keep or a re-add on a path that left the queue can say what became of it.

use axum::{
    extract::Query,
    http::{header::AUTHORIZATION, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use dutopia::auth::Claims;

use crate::trash::{can_manage, undo_hours, Action, Entry};
use crate::trash_store::{with_both, Changed};

/// Oldest outcomes are dropped past this many.
const MAX_HISTORY: usize = 50_000;
/// GET /api/trash/history returns at most this many, newest first.
const HISTORY_PAGE: usize = 1000;
/// Statuses the cron may report; "claimed" and "kept" are duapi's own.
const REPORTED: [&str; 3] = ["moved", "skipped", "error"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    #[serde(flatten)]
    pub entry: Entry,
    /// claimed | moved | skipped | error | kept
    pub status: String,
    /// Why it was skipped or failed; who kept it.
    #[serde(default)]
    pub detail: String,
    /// Where it ended up (moved).
    #[serde(default)]
    pub dest: Option<String>,
    /// Unix seconds; stamped by duapi when the cron leaves it at 0.
    #[serde(default)]
    pub done_at: i64,
}

impl Outcome {
    pub fn new(entry: Entry, status: &str, detail: String, now: i64) -> Self {
        Outcome { entry, status: status.into(), detail, dest: None, done_at: now }
    }
}

/// Keep only the newest `max` outcomes (the list is oldest first).
pub fn trim_history(history: &mut Vec<Outcome>, max: usize) {
    if history.len() > max {
        history.drain(..history.len() - max);
    }
}

/// The newest outcome for `path` that `claims` may see.
pub fn latest_for<'a>(history: &'a [Outcome], path: &str, claims: &Claims) -> Option<&'a Outcome> {
    history
        .iter()
        .rev()
        .find(|o| o.entry.path == path && can_manage(claims, &o.entry.owner, &o.entry.path))
}

fn when(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_default()
}

/// The Keep answer for a path no longer queued, from its latest outcome.
pub fn keep_too_late(latest: Option<&Outcome>) -> Response {
    let Some(o) = latest else {
        return (StatusCode::NOT_FOUND, "not queued").into_response();
    };
    let at = when(o.done_at);
    let msg = match o.status.as_str() {
        "claimed" => format!("too late to keep: the nightly run is moving it (since {at})"),
        "moved" => format!(
            "too late to keep: the nightly run moved it to {} on {at}",
            o.dest.as_deref().unwrap_or(o.entry.action.dir())
        ),
        "skipped" => format!(
            "not queued any more: the nightly run left it in place on {at} ({})",
            o.detail
        ),
        "error" => format!(
            "not queued any more: the nightly run failed on it on {at} ({}); it may still be in place",
            o.detail
        ),
        _ => return (StatusCode::NOT_FOUND, format!("not queued (kept on {at})")).into_response(),
    };
    (StatusCode::GONE, msg).into_response()
}

/// Why `path` cannot be queued again: the cron holds it, or moved it after
/// the DB was built (the listing still shows it until the next scan).
pub fn readd_blocked(latest: Option<&Outcome>, db_built_at: Option<i64>) -> Option<String> {
    let o = latest?;
    match o.status.as_str() {
        "claimed" => Some(format!("the nightly run is moving it (since {})", when(o.done_at))),
        "moved" if db_built_at.is_none_or(|b| o.done_at > b) => Some(format!(
            "already moved to the {} on {} ({}); the listing shows it until the next scan",
            o.entry.action.name(),
            when(o.done_at),
            o.dest.as_deref().unwrap_or(o.entry.action.dir())
        )),
        _ => None,
    }
}

// ---- cron endpoints ----

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
/// of the requested filesystems leave the queue and are recorded as
/// "claimed": from here on they belong to the cron. Due entries whose
/// filesystem is no longer in FILESYSTEMS are retired as "skipped" (no node
/// would ever take them). `?dry_run=1` returns the list and changes nothing.
/// Only entries of `?action` (default trash) are claimed or retired.
/// The reply confirms the options it applied (X-Trash-Dry-Run,
/// X-Trash-Fs-Filter, X-Trash-Action) so a newer cron can tell an older
/// duapi ignored them.
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
    let res = with_both(move |queue, history| {
        let due = |e: &Entry| e.action == action && is_due(e, now, after_secs);
        let wanted = |e: &Entry| only_fs.as_ref().is_none_or(|fss| fss.contains(&e.fs));
        let orphan = |e: &Entry| !listed.contains(&e.fs);
        let take: Vec<Entry> =
            queue.iter().filter(|e| due(e) && !orphan(e) && wanted(e)).cloned().collect();
        if dry_run {
            return (take, Changed::default());
        }
        let retired: Vec<Entry> = queue.iter().filter(|e| due(e) && orphan(e)).cloned().collect();
        queue.retain(|e| !(due(e) && (orphan(e) || wanted(e))));
        for e in &retired {
            let why = format!("filesystem {} is no longer listed in FILESYSTEMS", e.fs);
            history.push(Outcome::new(e.clone(), "skipped", why, now));
        }
        history.extend(take.iter().map(|e| Outcome::new(e.clone(), "claimed", String::new(), now)));
        trim_history(history, MAX_HISTORY);
        let changed = !take.is_empty() || !retired.is_empty();
        if !retired.is_empty() {
            tracing::warn!(items = retired.len(), "trash claim retired entries of unlisted filesystems");
        }
        (take, Changed { queue: changed, history: changed })
    })
    .await;
    match res {
        Ok(due) => {
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

/// POST /api/trash/report — the cron's results. Each replaces the record of
/// the same (path, queued_at) — normally its "claimed" — except that a
/// "moved" is never replaced by a rerun's "skipped: gone".
pub async fn report_handler(headers: HeaderMap, Json(outcomes): Json<Vec<Outcome>>) -> Response {
    if let Err(resp) = check_cron_token(&headers, "/api/trash/report") {
        return resp;
    }
    if let Some(bad) = outcomes.iter().find(|o| !REPORTED.contains(&o.status.as_str())) {
        let msg = format!("unknown status {:?} (expected moved, skipped or error)", bad.status);
        tracing::warn!(%msg, "400 POST /api/trash/report");
        return (StatusCode::BAD_REQUEST, msg).into_response();
    }
    let now = Utc::now().timestamp();
    let n = outcomes.len();
    let res = with_both(move |_queue, history| {
        for mut o in outcomes {
            if o.done_at == 0 {
                o.done_at = now;
            }
            let same = history.iter().rposition(|h| {
                h.entry.path == o.entry.path
                    && h.entry.queued_at == o.entry.queued_at
                    && h.status != "kept"
            });
            match same {
                Some(i) if history[i].status == "moved" && o.status != "moved" => continue,
                Some(i) => {
                    history.remove(i);
                    history.push(o);
                }
                None => history.push(o),
            }
        }
        trim_history(history, MAX_HISTORY);
        ((), Changed { queue: false, history: n > 0 })
    })
    .await;
    match res {
        Ok(()) => {
            tracing::info!(items = n, "200 OK POST /api/trash/report");
            Json(serde_json::json!({ "recorded": n })).into_response()
        }
        Err(resp) => resp,
    }
}

/// GET /api/trash/history — outcomes the caller may see, newest first.
pub async fn history_handler(claims: Claims) -> Response {
    let actor = claims.sub.clone();
    let res = with_both(move |_queue, history| {
        let visible: Vec<Outcome> = history
            .iter()
            .rev()
            .filter(|o| can_manage(&claims, &o.entry.owner, &o.entry.path))
            .take(HISTORY_PAGE)
            .cloned()
            .collect();
        (visible, Changed::default())
    })
    .await;
    match res {
        Ok(v) => {
            tracing::info!(actor = %actor, items = v.len(), "200 OK GET /api/trash/history");
            Json(v).into_response()
        }
        Err(resp) => resp,
    }
}

#[cfg(test)]
#[path = "trash_history_tests.rs"]
mod tests;
