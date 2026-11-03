// rs/src/bin/duapi/trash_tests.rs
use super::*;
use axum::body::to_bytes;
use axum::http::HeaderValue;
use serial_test::serial;

const LIMIT: usize = 1024 * 1024;
const FSS: &str = "/red/nas1,/red/nas2";

fn claims(sub: &str, is_admin: bool, roots: &[&str]) -> Claims {
    Claims {
        sub: sub.into(),
        is_admin,
        roots: roots.iter().map(|s| s.to_string()).collect(),
        exp: 9_999_999_999,
    }
}

fn fss() -> Vec<String> {
    vec!["/red/nas1".into(), "/red/nas2".into(), "/red/nas2/deep".into()]
}

fn entry(path: &str, owner: &str, queued_at: i64) -> Entry {
    Entry {
        path: path.into(),
        size: 10,
        owner: owner.into(),
        actor: owner.into(),
        fs: "/red/nas1".into(),
        queued_at,
    }
}

/// Point TRASH_DIR at a fresh temp dir and set FILESYSTEMS. Keep the
/// returned guard alive for the test.
fn setup() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("TRASH_DIR", dir.path());
        std::env::set_var("FILESYSTEMS", FSS);
        std::env::set_var("TRASH_TOKEN", "s3cret");
    }
    dir
}

async fn add(c: &Claims, path: &str, owner: &str) -> StatusCode {
    let req = AddReq { path: path.into(), size: 100, owner: owner.into() };
    add_handler(c.clone(), Json(req)).await.status()
}

async fn list(c: &Claims) -> Vec<Entry> {
    let resp = list_handler(c.clone()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    serde_json::from_slice(&to_bytes(resp.into_body(), LIMIT).await.unwrap()).unwrap()
}

async fn keep(c: &Claims, path: Option<&str>) -> StatusCode {
    let q = KeepQuery { path: path.map(Into::into) };
    keep_handler(c.clone(), Query(q)).await.status()
}

async fn claim(auth: Option<&str>) -> (StatusCode, Vec<Entry>) {
    let mut headers = HeaderMap::new();
    if let Some(a) = auth {
        headers.insert("authorization", HeaderValue::from_str(a).unwrap());
    }
    let resp = claim_handler(headers).await;
    let status = resp.status();
    let body = to_bytes(resp.into_body(), LIMIT).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap_or_default())
}

// ---- pure rules ----

#[test]
fn can_manage_follows_the_three_tiers() {
    let global = claims("root", true, &[]);
    assert!(can_manage(&global, "bob", "/red/nas1/x"));

    let sa = claims("sa", false, &["/red/nas1"]);
    assert!(can_manage(&sa, "bob", "/red/nas1/x"));
    assert!(!can_manage(&sa, "bob", "/red/nas2/x"));
    assert!(can_manage(&sa, "sa", "/red/nas2/x")); // own, outside roots
    assert!(!can_manage(&sa, "bob", "/red/nas10/x")); // prefix trap

    let user = claims("alice", false, &[]);
    assert!(can_manage(&user, "alice", "/red/nas1/x"));
    assert!(!can_manage(&user, "bob", "/red/nas1/x"));
}

#[test]
fn check_target_returns_the_holding_filesystem() {
    assert_eq!(check_target("/red/nas1/a/b", &fss()), Ok("/red/nas1".into()));
    assert_eq!(check_target("/red/nas2/deep/x", &fss()), Ok("/red/nas2/deep".into()));
    // `.trashy` is not `.trash`.
    assert!(check_target("/red/nas1/.trashy", &fss()).is_ok());
}

#[test]
fn check_target_rejects_roots_trash_stats_and_outside() {
    for p in [
        "/red/nas1",
        "/red/nas2/deep",
        "/red/nas1/.trash",
        "/red/nas1/.trash/2026-11-04/a",
        "/red/nas1/.stats",
        "/red/nas1/.stats/scan.csv",
        "/red/nas10/x",
        "/opt/x",
    ] {
        assert!(check_target(p, &fss()).is_err(), "{p} should be rejected");
    }
    assert!(check_target("/red/nas1/x", &[]).is_err());
}

#[test]
fn split_due_uses_the_24h_rule() {
    let now = 1_000_000;
    let old = entry("/red/nas1/old", "a", now - CLAIM_AFTER_SECS);
    let fresh = entry("/red/nas1/fresh", "a", now - CLAIM_AFTER_SECS + 1);
    let (due, kept) = split_due(vec![old.clone(), fresh.clone()], now, CLAIM_AFTER_SECS);
    assert_eq!(due, vec![old]);
    assert_eq!(kept, vec![fresh]);
}

#[test]
fn load_save_roundtrip_and_missing_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let qdir = dir.path().join("sub");
    assert!(load(&qdir).unwrap().is_empty());
    let items = vec![entry("/red/nas1/a", "alice", 5)];
    save(&qdir, &items).unwrap();
    assert_eq!(load(&qdir).unwrap(), items);
    let names: Vec<_> = std::fs::read_dir(&qdir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, vec!["queue.json".to_string()]); // no tmp left behind
}

// ---- handlers ----

#[tokio::test]
#[serial]
async fn add_then_list_records_fs_actor_and_owner() {
    let dir = setup();
    let alice = claims("alice", false, &[]);
    assert_eq!(add(&alice, "/red/nas1/proj/run1/", "alice").await, StatusCode::CREATED);
    let items = list(&alice).await;
    assert_eq!(items.len(), 1);
    let e = &items[0];
    assert_eq!(e.path, "/red/nas1/proj/run1"); // normalized
    assert_eq!((e.fs.as_str(), e.owner.as_str(), e.actor.as_str()), ("/red/nas1", "alice", "alice"));
    assert_eq!(e.size, 100);
    assert!(e.queued_at > 0);
    // On disk, for the cron.
    assert_eq!(load(dir.path()).unwrap(), items);
}

#[tokio::test]
#[serial]
async fn add_is_idempotent_and_keeps_the_first_timestamp() {
    let dir = setup();
    save(dir.path(), &[entry("/red/nas1/a", "alice", 7)]).unwrap();
    let alice = claims("alice", false, &[]);
    assert_eq!(add(&alice, "/red/nas1/a", "alice").await, StatusCode::OK);
    let items = load(dir.path()).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].queued_at, 7);
}

#[tokio::test]
#[serial]
async fn add_enforces_tiers() {
    let _dir = setup();
    let alice = claims("alice", false, &[]);
    let sa = claims("sa", false, &["/red/nas1"]);
    let root = claims("root", true, &[]);
    assert_eq!(add(&alice, "/red/nas1/b", "bob").await, StatusCode::FORBIDDEN);
    assert_eq!(add(&sa, "/red/nas2/b", "bob").await, StatusCode::FORBIDDEN);
    assert_eq!(add(&sa, "/red/nas1/b", "bob").await, StatusCode::CREATED);
    assert_eq!(add(&sa, "/red/nas2/mine", "sa").await, StatusCode::CREATED);
    assert_eq!(add(&root, "/red/nas2/b", "bob").await, StatusCode::CREATED);
    // The storage admin queued it, but the owner may see and keep it too.
    let actor = list(&claims("bob", false, &[])).await;
    assert_eq!(actor.len(), 2);
    assert!(actor.iter().any(|e| e.actor == "sa"));
}

#[tokio::test]
#[serial]
async fn add_rejects_bad_targets() {
    let _dir = setup();
    let root = claims("root", true, &[]);
    for p in [
        "/red/nas1",
        "/red/nas1/",
        "/red/nas1/.trash/x",
        "/red/nas1/.stats",
        "/opt/x",
        "red/nas1/x",
        "/red/nas1/../nas2/x",
        "",
    ] {
        assert_eq!(add(&root, p, "bob").await, StatusCode::BAD_REQUEST, "{p:?}");
    }
    assert_eq!(add(&root, "/red/nas1/x", " ").await, StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[serial]
async fn add_without_filesystems_is_refused() {
    let _dir = setup();
    unsafe { std::env::remove_var("FILESYSTEMS") };
    let root = claims("root", true, &[]);
    assert_eq!(add(&root, "/red/nas1/x", "bob").await, StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[serial]
async fn list_is_scoped_per_tier() {
    let dir = setup();
    save(
        dir.path(),
        &[
            entry("/red/nas1/a", "alice", 1),
            entry("/red/nas1/b", "bob", 1),
            entry("/red/nas2/c", "bob", 1),
        ],
    )
    .unwrap();
    let paths = |v: Vec<Entry>| v.into_iter().map(|e| e.path).collect::<Vec<_>>();
    assert_eq!(paths(list(&claims("alice", false, &[])).await), vec!["/red/nas1/a"]);
    assert_eq!(
        paths(list(&claims("sa", false, &["/red/nas1"])).await),
        vec!["/red/nas1/a", "/red/nas1/b"]
    );
    assert_eq!(list(&claims("root", true, &[])).await.len(), 3);
}

#[tokio::test]
#[serial]
async fn keep_removes_only_what_the_caller_may_manage() {
    let dir = setup();
    save(dir.path(), &[entry("/red/nas1/a", "alice", 1), entry("/red/nas1/b", "bob", 1)]).unwrap();
    let alice = claims("alice", false, &[]);
    assert_eq!(keep(&alice, Some("/red/nas1/b")).await, StatusCode::FORBIDDEN);
    assert_eq!(keep(&alice, Some("/red/nas1/a/")).await, StatusCode::OK);
    assert_eq!(keep(&alice, Some("/red/nas1/a")).await, StatusCode::NOT_FOUND);
    assert_eq!(keep(&alice, None).await, StatusCode::BAD_REQUEST);
    let left: Vec<String> = load(dir.path()).unwrap().into_iter().map(|e| e.path).collect();
    assert_eq!(left, vec!["/red/nas1/b"]);
    let sa = claims("sa", false, &["/red/nas1"]);
    assert_eq!(keep(&sa, Some("/red/nas1/b")).await, StatusCode::OK);
}

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
async fn add_over_someone_elses_entry_is_a_conflict() {
    let dir = setup();
    save(dir.path(), &[entry("/red/nas1/a", "alice", 7)]).unwrap();
    // Bob claims to own it: passes his own check, but must not see or
    // touch alice's entry.
    let bob = claims("bob", false, &[]);
    assert_eq!(add(&bob, "/red/nas1/a", "bob").await, StatusCode::CONFLICT);
    assert_eq!(load(dir.path()).unwrap()[0].owner, "alice");
}

// ---- review fixes: single owner per the DB (C2), UNK owners (I2) ----

fn folder(path: &str, users: &[&str]) -> dutopia::db::FolderOut {
    dutopia::db::FolderOut {
        path: path.into(),
        users: users.iter().map(|u| (u.to_string(), Default::default())).collect(),
    }
}

#[test]
fn owner_conflict_requires_exactly_the_claimed_owner() {
    assert_eq!(owner_conflict(&[], "alice"), None); // a file, or not in the DB
    assert_eq!(owner_conflict(&[folder("/a", &["alice"])], "alice"), None);
    let mixed = owner_conflict(&[folder("/a", &["alice", "bob"])], "alice").unwrap();
    assert!(mixed.contains("bob"), "{mixed}");
    assert!(owner_conflict(&[folder("/a", &["bob"])], "alice").is_some());
}

#[tokio::test]
#[serial]
async fn add_checks_the_db_owner_of_a_folder() {
    let _dir = setup();
    crate::handler::tests::init_db_once();
    // Fixture: /docs holds only alice's files.
    unsafe { std::env::set_var("FILESYSTEMS", "/") };
    let root = claims("root", true, &[]);
    assert_eq!(add(&root, "/docs", "bob").await, StatusCode::CONFLICT);
    assert_eq!(add(&root, "/docs", "alice").await, StatusCode::CREATED);
}

#[tokio::test]
#[serial]
async fn add_refuses_unknown_owner() {
    let _dir = setup();
    let root = claims("root", true, &[]);
    assert_eq!(add(&root, "/red/nas1/old", "UNK").await, StatusCode::BAD_REQUEST);
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
