// rs/src/bin/duapi/trash_history_tests.rs
// Claim, report, history and the Keep/re-add answers that depend on them.
use super::*;
use crate::trash::tests::*;
use crate::trash::{add_handler, keep_handler, load, save, AddReq, Entry, KeepQuery, CLAIM_AFTER_SECS};
use axum::body::to_bytes;
use axum::extract::Query;
use axum::http::HeaderValue;
use axum::Json;
use serial_test::serial;

#[tokio::test]
#[serial]
async fn claim_needs_the_token() {
    let dir = setup();
    save(dir.path(), &[entry("/red/nas1/a", "alice", 1)]).unwrap();
    assert_eq!(claim(None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(claim(Some("Bearer wrong")).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(claim(Some("s3cret")).await.0, StatusCode::UNAUTHORIZED);
    unsafe { std::env::remove_var("TRASH_TOKEN") };
    assert_eq!(claim(Some("Bearer ")).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(load(dir.path()).unwrap().len(), 1); // nothing claimed
}

#[tokio::test]
#[serial]
async fn claim_returns_and_removes_only_old_entries() {
    let dir = setup();
    let now = chrono::Utc::now().timestamp();
    let old = entry("/red/nas1/old", "alice", now - CLAIM_AFTER_SECS - 60);
    let fresh = entry("/red/nas1/fresh", "alice", now - 60);
    save(dir.path(), &[old.clone(), fresh.clone()]).unwrap();
    let (status, got) = claim(Some("Bearer s3cret")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got, vec![old]);
    assert_eq!(load(dir.path()).unwrap(), vec![fresh]);
    // A second run finds nothing new.
    assert!(claim(Some("Bearer s3cret")).await.1.is_empty());
}


#[tokio::test]
#[serial]
async fn undo_hours_env_shortens_the_wait() {
    let dir = setup();
    let now = chrono::Utc::now().timestamp();
    let fresh = entry("/red/nas1/fresh", "alice", now - 60);
    save(dir.path(), &[fresh.clone()]).unwrap();
    unsafe { std::env::set_var("TRASH_UNDO_HOURS", "0") };
    assert_eq!(undo_hours(), 0);
    let (status, got) = claim(Some("Bearer s3cret")).await;
    unsafe { std::env::remove_var("TRASH_UNDO_HOURS") };
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got, vec![fresh]);
    assert_eq!(undo_hours(), 24);
}

#[tokio::test]
#[serial]
async fn dry_run_claim_lists_due_entries_and_keeps_them() {
    let dir = setup();
    let now = chrono::Utc::now().timestamp();
    let old = entry("/red/nas1/old", "alice", now - CLAIM_AFTER_SECS - 60);
    let fresh = entry("/red/nas1/fresh", "alice", now - 60);
    save(dir.path(), &[old.clone(), fresh.clone()]).unwrap();
    let (status, got) = claim_with(Some("Bearer s3cret"), Some("1")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got, vec![old.clone()]);
    assert_eq!(load(dir.path()).unwrap(), vec![old.clone(), fresh]); // untouched
    // Still needs the token.
    assert_eq!(claim_with(Some("Bearer nope"), Some("1")).await.0, StatusCode::UNAUTHORIZED);
    // A real claim afterwards still gets it.
    assert_eq!(claim(Some("Bearer s3cret")).await.1, vec![old]);
}

#[tokio::test]
#[serial]
async fn claim_takes_only_the_requested_filesystems() {
    let dir = setup();
    let mut a = entry("/red/nas1/a", "alice", 1);
    a.fs = "/red/nas1".into();
    let mut b = entry("/red/nas2/b", "alice", 1);
    b.fs = "/red/nas2".into();
    save(dir.path(), &[a.clone(), b.clone()]).unwrap();
    // A cron node that mounts only nas2 must leave nas1's items queued.
    let q = ClaimQuery { dry_run: None, fs: Some("/red/nas2/".into()), action: None };
    let (status, got) = claim_q(Some("Bearer s3cret"), q).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got, vec![b]);
    assert_eq!(load(dir.path()).unwrap(), vec![a.clone()]);
    // An empty list claims nothing.
    let q = ClaimQuery { dry_run: None, fs: Some("".into()), action: None };
    assert!(claim_q(Some("Bearer s3cret"), q).await.1.is_empty());
    assert_eq!(load(dir.path()).unwrap(), vec![a]);
}


// ---- report + history: what happened to each claimed item ----


fn outcome(e: &Entry, status: &str, detail: &str, done_at: i64) -> Outcome {
    Outcome { entry: e.clone(), status: status.into(), detail: detail.into(), dest: None, done_at }
}

async fn report(auth: Option<&str>, outcomes: Vec<Outcome>) -> StatusCode {
    let mut headers = HeaderMap::new();
    if let Some(a) = auth {
        headers.insert("authorization", HeaderValue::from_str(a).unwrap());
    }
    report_handler(headers, Json(outcomes)).await.status()
}

async fn history(c: &Claims) -> Vec<Outcome> {
    let resp = history_handler(c.clone()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    serde_json::from_slice(&to_bytes(resp.into_body(), LIMIT).await.unwrap()).unwrap()
}

#[tokio::test]
#[serial]
async fn report_needs_the_token_and_feeds_a_scoped_history() {
    let _dir = setup();
    let a = entry("/red/nas1/a", "alice", 1);
    let b = entry("/red/nas1/b", "bob", 1);
    let sent = vec![outcome(&a, "moved", "", 0), outcome(&b, "skipped", "gone", 0)];
    assert_eq!(report(Some("Bearer nope"), sent.clone()).await, StatusCode::UNAUTHORIZED);
    assert_eq!(report(Some("Bearer s3cret"), sent).await, StatusCode::OK);
    let mine = history(&claims("alice", false, &[])).await;
    assert_eq!(mine.len(), 1);
    assert_eq!((mine[0].entry.path.as_str(), mine[0].status.as_str()), ("/red/nas1/a", "moved"));
    assert!(mine[0].done_at > 0, "server stamps done_at");
    assert_eq!(history(&claims("root", true, &[])).await.len(), 2);
}

#[tokio::test]
#[serial]
async fn report_rejects_unknown_status() {
    let _dir = setup();
    let a = entry("/red/nas1/a", "alice", 1);
    assert_eq!(report(Some("Bearer s3cret"), vec![outcome(&a, "exploded", "", 0)]).await,
               StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[serial]
async fn keep_after_the_cron_took_it_explains_what_happened() {
    let _dir = setup();
    let a = entry("/red/nas1/a", "alice", 1);
    assert_eq!(report(Some("Bearer s3cret"), vec![outcome(&a, "moved", "", 0)]).await, StatusCode::OK);
    let alice = claims("alice", false, &[]);
    assert_eq!(keep(&alice, Some("/red/nas1/a")).await, StatusCode::GONE);
    // Someone who cannot see it gets the plain 404.
    assert_eq!(keep(&claims("bob", false, &[]), Some("/red/nas1/a")).await, StatusCode::NOT_FOUND);
}

#[test]
fn trim_history_keeps_the_newest() {
    let e = entry("/red/nas1/a", "alice", 1);
    let mut h: Vec<Outcome> = (0..5).map(|i| outcome(&e, "moved", "", i)).collect();
    trim_history(&mut h, 3);
    assert_eq!(h.iter().map(|o| o.done_at).collect::<Vec<_>>(), vec![2, 3, 4]);
}


// ---- claim → claimed → report lifecycle (review 2) ----

async fn add_full(c: &Claims, path: &str, owner: &str) -> (StatusCode, String) {
    let req = AddReq { path: path.into(), size: 100, owner: owner.into(), action: Default::default() };
    let resp = add_handler(c.clone(), Json(req)).await;
    let status = resp.status();
    let body = to_bytes(resp.into_body(), LIMIT).await.unwrap();
    (status, String::from_utf8_lossy(&body).into_owned())
}

async fn keep_full(c: &Claims, path: &str) -> (StatusCode, String) {
    let q = KeepQuery { path: Some(path.into()) };
    let resp = keep_handler(c.clone(), Query(q)).await;
    let status = resp.status();
    let body = to_bytes(resp.into_body(), LIMIT).await.unwrap();
    (status, String::from_utf8_lossy(&body).into_owned())
}

async fn claim_headers(q: ClaimQuery) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("authorization", HeaderValue::from_static("Bearer s3cret"));
    claim_handler(headers, Query(q)).await.headers().clone()
}

fn due(path: &str, owner: &str, fs: &str) -> Entry {
    let mut e = entry(path, owner, 1);
    e.fs = fs.into();
    e
}

#[tokio::test]
#[serial]
async fn claim_with_root_filesystem() {
    let dir = setup();
    unsafe { std::env::set_var("FILESYSTEMS", "/") };
    save(dir.path(), &[due("/x/y", "alice", "/")]).unwrap();
    let q = ClaimQuery { dry_run: None, fs: Some("/".into()), action: None };
    assert_eq!(claim_q(Some("Bearer s3cret"), q).await.1.len(), 1);
}

#[tokio::test]
#[serial]
async fn claimed_items_are_visible_and_locked_until_reported() {
    let _dir = setup();
    let a = due("/red/nas1/a", "alice", "/red/nas1");
    save(_dir.path(), &[a.clone()]).unwrap();
    assert_eq!(claim(Some("Bearer s3cret")).await.1, vec![a.clone()]);
    let alice = claims("alice", false, &[]);
    let h = history(&alice).await;
    assert_eq!((h.len(), h[0].status.as_str()), (1, "claimed"));
    let (code, body) = keep_full(&alice, "/red/nas1/a").await;
    assert_eq!(code, StatusCode::GONE);
    assert!(body.contains("moving it"), "{body}");
    assert_eq!(add_full(&alice, "/red/nas1/a", "alice").await.0, StatusCode::CONFLICT);
    // The report supersedes the claim; a later "skipped: gone" cannot undo "moved".
    let mut moved = outcome(&a, "moved", "", 0);
    moved.dest = Some("/red/nas1/.trash/d/a".into());
    assert_eq!(report(Some("Bearer s3cret"), vec![moved]).await, StatusCode::OK);
    assert_eq!(report(Some("Bearer s3cret"), vec![outcome(&a, "skipped", "gone", 0)]).await, StatusCode::OK);
    let h = history(&alice).await;
    assert_eq!(h.len(), 1, "{h:?}");
    assert_eq!(h[0].status, "moved");
}

#[tokio::test]
#[serial]
async fn report_cannot_claim_server_statuses() {
    let _dir = setup();
    let a = due("/red/nas1/a", "alice", "/red/nas1");
    for s in ["claimed", "kept"] {
        assert_eq!(report(Some("Bearer s3cret"), vec![outcome(&a, s, "", 0)]).await, StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
#[serial]
async fn claim_retires_entries_of_unlisted_filesystems() {
    let dir = setup();
    let gone_fs = due("/red/old/a", "alice", "/red/old");
    let mine = due("/red/nas1/b", "alice", "/red/nas1");
    save(dir.path(), &[gone_fs.clone(), mine.clone()]).unwrap();
    // Even a node that only claims nas2 retires the orphan.
    let q = ClaimQuery { dry_run: None, fs: Some("/red/nas2".into()), action: None };
    assert!(claim_q(Some("Bearer s3cret"), q).await.1.is_empty());
    assert_eq!(load(dir.path()).unwrap(), vec![mine]);
    let h = history(&claims("root", true, &[])).await;
    assert_eq!(h[0].status, "skipped");
    assert!(h[0].detail.contains("FILESYSTEMS"), "{}", h[0].detail);
}

#[tokio::test]
#[serial]
async fn claim_confirms_the_options_it_applied() {
    let _dir = setup();
    let h = claim_headers(ClaimQuery { dry_run: Some("1".into()), fs: Some("/red/nas1".into()), action: None }).await;
    assert_eq!(h.get("x-trash-dry-run").unwrap(), "1");
    assert_eq!(h.get("x-trash-fs-filter").unwrap(), "1");
    let h = claim_headers(ClaimQuery { dry_run: None, fs: None, action: None }).await;
    assert!(h.get("x-trash-dry-run").is_none());
}

#[tokio::test]
#[serial]
async fn keep_wording_follows_the_outcome_and_keeps_are_recorded() {
    let dir = setup();
    let a = due("/red/nas1/a", "alice", "/red/nas1");
    assert_eq!(report(Some("Bearer s3cret"), vec![outcome(&a, "skipped", "gone", 0)]).await, StatusCode::OK);
    let alice = claims("alice", false, &[]);
    let (code, body) = keep_full(&alice, "/red/nas1/a").await;
    assert_eq!(code, StatusCode::GONE);
    assert!(body.contains("left it in place"), "{body}");
    // Re-queue then Keep: the Keep is recorded, and a second Keep is a plain 404.
    save(dir.path(), &[a.clone()]).unwrap();
    assert_eq!(keep(&alice, Some("/red/nas1/a")).await, StatusCode::OK);
    assert_eq!(history(&alice).await[0].status, "kept");
    assert_eq!(keep(&alice, Some("/red/nas1/a")).await, StatusCode::NOT_FOUND);
}

#[tokio::test]
#[serial]
async fn add_refuses_an_item_already_moved_since_the_scan() {
    let _dir = setup();
    let a = due("/red/nas1/a", "alice", "/red/nas1");
    assert_eq!(report(Some("Bearer s3cret"), vec![outcome(&a, "moved", "", 0)]).await, StatusCode::OK);
    let (code, body) = add_full(&claims("alice", false, &[]), "/red/nas1/a", "alice").await;
    assert_eq!(code, StatusCode::CONFLICT);
    assert!(body.contains("already moved"), "{body}");
}

#[tokio::test]
#[serial]
async fn conflicts_do_not_name_paths_the_caller_cannot_manage() {
    let dir = setup();
    save(dir.path(), &[due("/red/nas1/b", "bob", "/red/nas1")]).unwrap();
    let (code, body) = add_full(&claims("alice", false, &[]), "/red/nas1/b/x", "alice").await;
    assert_eq!(code, StatusCode::CONFLICT);
    assert!(!body.contains("/red/nas1/b"), "{body}");
}

#[tokio::test]
#[serial]
async fn a_parent_absorbs_only_children_of_its_owner_and_filesystem() {
    let dir = setup();
    save(dir.path(), &[due("/red/nas1/a/b", "bob", "/red/nas1")]).unwrap();
    // An admin may manage bob's child, but it is not alice's: refuse, keep it.
    assert_eq!(add(&claims("root", true, &[]), "/red/nas1/a", "alice").await, StatusCode::CONFLICT);
    assert_eq!(load(dir.path()).unwrap()[0].owner, "bob");
}

#[tokio::test]
#[serial]
async fn claim_names_the_action_it_took() {
    let _dir = setup();
    let h = claim_headers(ClaimQuery { dry_run: None, fs: None, action: None }).await;
    assert_eq!(h.get("x-trash-action").unwrap(), "trash");
    let h = claim_headers(ClaimQuery { dry_run: None, fs: None, action: Some("archive".into()) }).await;
    assert_eq!(h.get("x-trash-action").unwrap(), "archive");
}
