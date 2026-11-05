// rs/src/bin/duapi/trash_claim_tests.rs
// Claim: the token, the 24h rule, dry run, per-node filesystems, actions,
// and what is left for Keep and re-add once an item is claimed.
use super::*;
use crate::trash::tests::*;
use dutopia::auth::Claims;
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


// ---- after the claim ----

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
async fn claim_drops_entries_of_unlisted_filesystems() {
    let dir = setup();
    let gone_fs = due("/red/old/a", "alice", "/red/old");
    let mine = due("/red/nas1/b", "alice", "/red/nas1");
    save(dir.path(), &[gone_fs.clone(), mine.clone()]).unwrap();
    // Even a node that only claims nas2 retires the orphan.
    let q = ClaimQuery { dry_run: None, fs: Some("/red/nas2".into()), action: None };
    assert!(claim_q(Some("Bearer s3cret"), q).await.1.is_empty());
    assert_eq!(load(dir.path()).unwrap(), vec![mine]);
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

#[tokio::test]
#[serial]
async fn once_claimed_keep_is_a_404_and_the_path_can_be_queued_again() {
    let _dir = setup();
    let a = due("/red/nas1/a", "alice", "/red/nas1");
    save(_dir.path(), &[a.clone()]).unwrap();
    assert_eq!(claim(Some("Bearer s3cret")).await.1, vec![a]);
    assert!(load(_dir.path()).unwrap().is_empty(), "nothing is kept once claimed");
    let alice = claims("alice", false, &[]);
    let (code, body) = keep_full(&alice, "/red/nas1/a").await;
    assert_eq!(code, StatusCode::NOT_FOUND);
    assert!(body.contains("nightly run took it"), "{body}");
    // The listing shows it until the next scan; queuing it again is allowed
    // (the next run finds it gone and logs a skip).
    assert_eq!(add_full(&alice, "/red/nas1/a", "alice").await.0, StatusCode::CREATED);
}
