# duapi HIGH-Severity Production Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the HIGH-severity gaps from the production-readiness audit so duapi can be deployed safely on a Linux server (rate limiting, request limits, path-traversal blocking, structured logging in auth, removal of panicking unwraps in startup/request paths, graceful shutdown).

**Architecture:** All changes are scoped to `rs/` (the duapi binary and the shared `dutopia::auth` module). No frontend changes. We add three tower middleware layers (timeout, body-size limit, per-IP rate limit), introduce a `normalize_path` helper to reject `..` traversal, replace `println!` in Linux auth with `tracing` macros, replace the unsafe CORS origin `unwrap()` with proper error handling, and wire a tokio signal handler that drives graceful shutdown for both the plain HTTP and TLS server paths.

**Tech Stack:** Rust 2024, axum 0.8, axum-server 0.7 (TLS), tower-http 0.6 (`cors`, `fs`, `limit`, `timeout`), tower-governor 0.7 (per-IP rate limiting, new dep), tokio 1 (signal), tracing 0.1.

**Out of scope (deferred):** All BLOCKER items (Windows defaults, JWT in localStorage, dscl argv leak, JWT TTL config, CORS unwrap is in HIGH not BLOCKER — fixed here). All MEDIUM/LOW items.

**Important repo rule:** This repo's CLAUDE.md states `Do not run git commit ever.` Each task ends with a "stage changes" step — **commits are made externally by the user**, never by the executor.

---

## File Structure

**Modify:**
- `rs/Cargo.toml` — add `tower-governor`, enable `tower-http` features `limit` + `timeout`.
- `rs/src/bin/duapi/main.rs` — wire middleware layers, fix CORS unwrap, add graceful shutdown.
- `rs/src/bin/duapi/handler.rs` — call `normalize_path`, clamp result lengths to `MAX_PAGE_SIZE`.
- `rs/src/bin/duapi/query.rs` — add `normalize_path` helper + tests.
- `rs/src/auth.rs` — replace `println!` in Linux platform module with `tracing::warn!` (no password values).

**Create:**
- `rs/src/bin/duapi/shutdown.rs` — `shutdown_signal()` future awaiting SIGINT/SIGTERM.

No new files in `src/util/`. Logging is already initialized via `dutopia::util::logging::init_tracing` in `main.rs:61`.

---

## Task 1: Block path-traversal via `normalize_path`

**Files:**
- Modify: `rs/src/bin/duapi/query.rs`
- Test: `rs/src/bin/duapi/query.rs` (inline `#[cfg(test)]`)

**Why first:** Pure function, no async, fastest TDD loop. Other tasks depend on this helper existing.

- [ ] **Step 1: Write the failing tests**

Append inside the existing `mod tests { ... }` block in `rs/src/bin/duapi/query.rs`:

```rust
#[test]
fn test_normalize_path_basic() {
    assert_eq!(normalize_path("/").as_deref(), Some("/"));
    assert_eq!(normalize_path("").as_deref(), Some("/"));
    assert_eq!(normalize_path("/var/log").as_deref(), Some("/var/log"));
    assert_eq!(normalize_path("var/log").as_deref(), Some("/var/log"));
}

#[test]
fn test_normalize_path_collapses_slashes_and_dots() {
    assert_eq!(normalize_path("//var//log/").as_deref(), Some("/var/log"));
    assert_eq!(normalize_path("/var/./log").as_deref(), Some("/var/log"));
    assert_eq!(normalize_path("/var/log/.").as_deref(), Some("/var/log"));
}

#[test]
fn test_normalize_path_rejects_traversal() {
    assert!(normalize_path("/var/../etc/passwd").is_none());
    assert!(normalize_path("..").is_none());
    assert!(normalize_path("/a/b/../../c").is_none());
    assert!(normalize_path("/a/%2e%2e/b").as_deref() == Some("/a/%2e%2e/b"));
    // ^ percent-decoding is axum's job; we only block literal ".." segments.
}

#[test]
fn test_normalize_path_rejects_nul_byte() {
    assert!(normalize_path("/var/log\0/etc").is_none());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd rs && cargo test --bin duapi normalize_path`
Expected: FAIL with `cannot find function 'normalize_path' in this scope`.

- [ ] **Step 3: Implement `normalize_path`**

Add this function to `rs/src/bin/duapi/query.rs`, after `parse_users_csv`:

```rust
/// Normalize a user-supplied path. Returns `Some(clean)` where `clean` always starts with `/`
/// and contains no `..`, `.`, empty, or NUL-containing segments. Returns `None` if the input
/// would escape via `..` or contains a NUL byte.
pub fn normalize_path(input: &str) -> Option<String> {
    if input.as_bytes().contains(&0) {
        return None;
    }
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return Some("/".to_string());
    }
    let mut out: Vec<&str> = Vec::new();
    for seg in trimmed.split('/') {
        match seg {
            "" | "." => continue,
            ".." => return None,
            s => out.push(s),
        }
    }
    if out.is_empty() {
        Some("/".to_string())
    } else {
        Some(format!("/{}", out.join("/")))
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd rs && cargo test --bin duapi normalize_path`
Expected: PASS — 4 tests.

- [ ] **Step 5: Wire `normalize_path` into the handlers**

In `rs/src/bin/duapi/handler.rs`:

Replace lines 78–84 of `get_folders_handler` (the manual `path.starts_with('/')` block) with:

```rust
    let raw_path = q.path.unwrap_or_default();
    let path = match crate::query::normalize_path(&raw_path) {
        Some(p) => p,
        None => {
            tracing::warn!(input = %raw_path, "400 Bad Request /api/folders rejected path");
            return (StatusCode::BAD_REQUEST, "invalid path").into_response();
        }
    };
```

Replace lines 119–125 of `get_files_handler` (the `match q.path.as_deref() { ... }` block) with:

```rust
    let folder = match q.path.as_deref().map(crate::query::normalize_path) {
        Some(Some(p)) if p != "/" => p,
        Some(Some(_)) | Some(None) => {
            tracing::warn!(input = ?q.path, "400 Bad Request /api/files rejected path");
            return (StatusCode::BAD_REQUEST, "invalid or missing path").into_response();
        }
        None => {
            tracing::warn!("400 Bad Request /api/files missing 'path'");
            return (StatusCode::BAD_REQUEST, "missing 'path' query parameter").into_response();
        }
    };
```

- [ ] **Step 6: Add a handler-level traversal test**

Inside the `mod tests` block at the bottom of `rs/src/bin/duapi/handler.rs`, add:

```rust
#[tokio::test]
async fn test_get_files_handler_rejects_traversal() {
    let claims = Claims {
        sub: "root".into(),
        is_admin: true,
        exp: 9_999_999_999usize,
    };
    let q = FilesQuery {
        path: Some("/var/../etc/passwd".into()),
        users: None,
        age: None,
    };
    let resp = get_files_handler(claims, Query(q)).await.into_response();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
```

- [ ] **Step 7: Run all duapi tests**

Run: `cd rs && cargo test --bin duapi`
Expected: PASS — all existing tests + the new traversal test.

- [ ] **Step 8: Stage changes (do NOT commit — repo rule)**

```bash
cd /Users/san/dev/dutopia
git add rs/src/bin/duapi/query.rs rs/src/bin/duapi/handler.rs
git status
```

Expected: only the two files staged. **Stop here. Do not run `git commit`.**

---

## Task 2: Replace `println!` in Linux auth with `tracing`

**Files:**
- Modify: `rs/src/auth.rs:173-223` (Linux `pub mod platform`)

**Why:** `println!` writes to stdout in line-buffered text, bypassing the JSON tracing setup; structured logs lose the auth signal. Never log the password value.

- [ ] **Step 1: Write a smoke compile-check test**

We can't easily test that tracing fires (it requires a global subscriber), so we test the function still returns a bool for invalid creds. Add to `rs/src/auth.rs` inside a new `#[cfg(test)] mod tests` block at the end of the file:

```rust
#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests {
    use super::platform::verify_user;

    #[test]
    fn test_verify_user_rejects_garbage() {
        // su will fail for a non-existent user; we just want to confirm the function
        // returns `false` rather than panicking after the println! → tracing migration.
        assert!(!verify_user("definitely_not_a_real_user_xyz", "wrong"));
    }
}
```

- [ ] **Step 2: Run the test to confirm it builds before changes**

Run: `cd rs && cargo test --lib verify_user_rejects_garbage`
Expected on Linux: PASS. On macOS: test is `#[cfg]`-gated out and skipped — that's fine.

- [ ] **Step 3: Replace `println!` calls with `tracing` macros**

In `rs/src/auth.rs`, the Linux `pub mod platform` block (currently lines 173–223) becomes:

```rust
#[cfg(target_os = "linux")]
pub mod platform {
    use std::process::{Command, Stdio};
    use std::io::Write;

    /// Verify user credentials using the `su` command.
    /// Returns true if authentication succeeds, false otherwise.
    /// NOTE: passwords are written to su's stdin; they are NEVER logged.
    pub fn verify_user(username: &str, password: &str) -> bool {
        tracing::debug!(user = %username, "verify_user: spawning su");
        let mut child = match Command::new("su")
            .arg(username)
            .arg("-c")
            .arg("true")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(e) => {
                tracing::warn!(user = %username, error = %e, "verify_user: failed to spawn su");
                return false;
            }
        };

        if let Some(mut stdin) = child.stdin.take() {
            if let Err(e) = writeln!(stdin, "{}", password) {
                tracing::warn!(user = %username, error = %e, "verify_user: failed to write password to su stdin");
                return false;
            }
        }

        match child.wait() {
            Ok(status) if status.success() => true,
            Ok(status) => {
                tracing::warn!(user = %username, code = ?status.code(), "verify_user: su exited non-zero");
                false
            }
            Err(e) => {
                tracing::warn!(user = %username, error = %e, "verify_user: failed waiting for su");
                false
            }
        }
    }
}
```

- [ ] **Step 4: Repeat for the macOS `pub mod platform` block (lines 145–171)**

Even though deploy is Linux, we keep the codebase clean. Replace its two `println!` calls with the same `tracing::warn!` style — username only, never password.

- [ ] **Step 5: Run lib tests**

Run: `cd rs && cargo test --lib`
Expected: PASS.

- [ ] **Step 6: Run clippy**

Run: `cd rs && cargo clippy --all-targets -- -D warnings`
Expected: no warnings introduced by these changes.

- [ ] **Step 7: Stage changes**

```bash
git add rs/src/auth.rs
git status
```

**Do not commit.**

---

## Task 3: Replace CORS origin `unwrap()` with proper error exit

**Files:**
- Modify: `rs/src/bin/duapi/main.rs:116-125`

**Why:** A malformed `CORS_ORIGIN` env var currently panics the server at startup with a useless backtrace. Convert to a clean fatal-error exit matching the surrounding style (TLS path, port-in-use, etc.).

- [ ] **Step 1: Write a unit test for an `origin_to_header` helper**

We extract the parsing into a small helper to make it testable. Add this to `rs/src/bin/duapi/main.rs` (inside the existing `#[cfg(test)] mod tests` block at the bottom):

```rust
#[test]
fn test_parse_cors_origin_valid() {
    let v = parse_cors_origin("http://localhost:5173").expect("parse ok");
    assert_eq!(v.to_string(), "http://localhost:5173");
}

#[test]
fn test_parse_cors_origin_invalid() {
    assert!(parse_cors_origin("not a url").is_err());
    assert!(parse_cors_origin("").is_err());
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cd rs && cargo test --bin duapi parse_cors_origin`
Expected: FAIL — `cannot find function 'parse_cors_origin'`.

- [ ] **Step 3: Add the helper and use it**

Add this free function to `rs/src/bin/duapi/main.rs` (above `is_port_taken`):

```rust
/// Parse a CORS origin string into the `HeaderValue` that `tower_http`'s `CorsLayer` requires.
fn parse_cors_origin(s: &str) -> Result<axum::http::HeaderValue, anyhow::Error> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        anyhow::bail!("CORS_ORIGIN is empty");
    }
    trimmed
        .parse::<axum::http::HeaderValue>()
        .with_context(|| format!("invalid CORS_ORIGIN value: {trimmed:?}"))
}
```

Replace the `cors` builder block (current lines 116–125) with:

```rust
    let cors = if let Some(ref origin) = args.cors_origin {
        let header = match parse_cors_origin(origin) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("{}", format!("FATAL: {e}").red());
                std::process::exit(1);
            }
        };
        CorsLayer::new()
            .allow_origin(header)
            .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
            .allow_headers(Any)
    } else {
        CorsLayer::new()
            .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
            .allow_headers(Any)
    };
```

Note: `.allow_origin(header)` (single `HeaderValue`) replaces the old `.allow_origin([origin.parse().unwrap()])` (slice form). This compiles because `HeaderValue: Into<AllowOrigin>`.

- [ ] **Step 4: Run the new tests**

Run: `cd rs && cargo test --bin duapi parse_cors_origin`
Expected: PASS — both tests.

- [ ] **Step 5: Stage changes**

```bash
git add rs/src/bin/duapi/main.rs
git status
```

**Do not commit.**

---

## Task 4: Wire request timeout + body-size limit middleware

**Files:**
- Modify: `rs/Cargo.toml` (enable `tower-http` features `limit` + `timeout`)
- Modify: `rs/src/bin/duapi/main.rs` (read env vars, attach layers)

**Why:** `.env.example` documents `REQUEST_TIMEOUT_SECS=30`; the code currently ignores it. A hung handler today blocks a worker indefinitely. A hostile POST body with no `Content-Length` cap can OOM the process.

- [ ] **Step 1: Enable the tower-http features**

In `rs/Cargo.toml`, change line 25 from:

```toml
tower-http = { version = "0.6", features = ["cors","fs"] }
```

to:

```toml
tower-http = { version = "0.6", features = ["cors", "fs", "limit", "timeout"] }
```

- [ ] **Step 2: Run `cargo check` to confirm features compile**

Run: `cd rs && cargo check --bin duapi`
Expected: clean check, no warnings about missing features.

- [ ] **Step 3: Add an env-helper unit test**

Add to the existing `#[cfg(test)] mod tests` block in `rs/src/bin/duapi/main.rs`:

```rust
#[test]
fn test_env_u64_with_default() {
    assert_eq!(env_u64("DUAPI_TEST_MISSING_VAR", 30), 30);
    // SAFETY: env mutation in a unit test; serial_test crate is already a dep.
    unsafe { std::env::set_var("DUAPI_TEST_PARSE_OK", "120") };
    assert_eq!(env_u64("DUAPI_TEST_PARSE_OK", 30), 120);
    unsafe { std::env::set_var("DUAPI_TEST_PARSE_BAD", "not-a-number") };
    assert_eq!(env_u64("DUAPI_TEST_PARSE_BAD", 30), 30);
}
```

- [ ] **Step 4: Run to confirm failure**

Run: `cd rs && cargo test --bin duapi env_u64`
Expected: FAIL — `cannot find function 'env_u64'`.

- [ ] **Step 5: Add the helper and the middleware**

Add the helper to `rs/src/bin/duapi/main.rs` (above `is_port_taken`):

```rust
fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}
```

Add to the imports at the top of `rs/src/bin/duapi/main.rs`:

```rust
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
```

Replace the existing `app` construction (current lines 137–140) with:

```rust
    let timeout_secs = env_u64("REQUEST_TIMEOUT_SECS", 30);
    let body_limit_bytes = env_u64("MAX_BODY_BYTES", 64 * 1024) as usize;
    tracing::info!(timeout_secs, body_limit_bytes, "request limits configured");

    let app = Router::new()
        .nest("/api", api)
        .fallback_service(frontend)
        .layer(cors)
        .layer(TimeoutLayer::new(Duration::from_secs(timeout_secs)))
        .layer(RequestBodyLimitLayer::new(body_limit_bytes));
```

- [ ] **Step 6: Run the helper test**

Run: `cd rs && cargo test --bin duapi env_u64`
Expected: PASS.

- [ ] **Step 7: Run the full duapi test suite**

Run: `cd rs && cargo test --bin duapi`
Expected: all pass.

- [ ] **Step 8: Add `MAX_BODY_BYTES` to `.env.example`**

Append to `/Users/san/dev/dutopia/.env.example` after the `RATE_LIMIT_PER_MIN=300` line:

```
# Maximum request body size in bytes (login JSON, etc.)
MAX_BODY_BYTES=65536
```

- [ ] **Step 9: Stage changes**

```bash
git add rs/Cargo.toml rs/src/bin/duapi/main.rs .env.example
git status
```

**Do not commit.**

---

## Task 5: Add per-IP rate limiting via `tower-governor`

**Files:**
- Modify: `rs/Cargo.toml` (add `tower-governor`)
- Modify: `rs/src/bin/duapi/main.rs` (read `RATE_LIMIT_PER_MIN`, attach layer)

**Why:** Audit item #6 — `RATE_LIMIT_PER_MIN` is documented but not implemented. Anyone can spam `/api/login` to brute-force credentials.

- [ ] **Step 1: Add the dependency**

In `rs/Cargo.toml`, add to the main `[dependencies]` block (after the `tower-http` line):

```toml
tower_governor = "0.7"
```

- [ ] **Step 2: Run `cargo check`**

Run: `cd rs && cargo check --bin duapi`
Expected: clean check (downloads `tower_governor` if not cached).

> If `cargo check` fails because `tower_governor` 0.7 has a different API on the installed `axum` version, fall back to `tower_governor = "0.4"`. The wiring in step 4 below works for both — `GovernorConfigBuilder::default().per_second().burst_size().finish()` is stable.

- [ ] **Step 3: Add the imports**

Add to the top of `rs/src/bin/duapi/main.rs`:

```rust
use std::sync::Arc;
use tower_governor::{governor::GovernorConfigBuilder, GovernorLayer};
```

- [ ] **Step 4: Build the layer and attach it**

Insert this block in `main()` immediately before the `let app = Router::new()` block from Task 4:

```rust
    let rate_per_min = env_u64("RATE_LIMIT_PER_MIN", 300);
    let per_sec = (rate_per_min / 60).max(1);
    let burst = (rate_per_min / 6).max(5) as u32; // ~10s burst window
    let governor_conf = match GovernorConfigBuilder::default()
        .per_second(per_sec)
        .burst_size(burst)
        .finish()
    {
        Some(c) => Arc::new(c),
        None => {
            eprintln!(
                "{}",
                format!("FATAL: invalid rate limit (per_sec={per_sec}, burst={burst})").red()
            );
            std::process::exit(1);
        }
    };
    tracing::info!(per_sec, burst, "rate limiter configured");
    let governor_layer = GovernorLayer { config: governor_conf };
```

Then add `.layer(governor_layer)` to the `app` chain (place it after the `RequestBodyLimitLayer` from Task 4):

```rust
    let app = Router::new()
        .nest("/api", api)
        .fallback_service(frontend)
        .layer(cors)
        .layer(TimeoutLayer::new(Duration::from_secs(timeout_secs)))
        .layer(RequestBodyLimitLayer::new(body_limit_bytes))
        .layer(governor_layer);
```

- [ ] **Step 5: Run the duapi test suite**

Run: `cd rs && cargo test --bin duapi`
Expected: all pass (existing handler tests do not exercise the layer; this is acceptable — manual smoke-test in Step 6).

- [ ] **Step 6: Manual smoke-test rate limiting**

Start the server with a low limit and hammer `/api/health` from a second terminal:

```bash
# terminal 1
cd /Users/san/dev/dutopia/rs
RATE_LIMIT_PER_MIN=60 JWT_SECRET=test cargo run --bin duapi -- /tmp/empty.csv --port 18080

# terminal 2
for i in $(seq 1 50); do
  curl -s -o /dev/null -w "%{http_code} " http://127.0.0.1:18080/api/health
done; echo
```

Expected: a mix of `200` and `429` responses once the burst is exhausted. If you see only `200`s, the layer is not attached. If you see `500`s, the config builder failed — check the error message.

- [ ] **Step 7: Stage changes**

```bash
git add rs/Cargo.toml rs/Cargo.lock rs/src/bin/duapi/main.rs
git status
```

**Do not commit.**

---

## Task 6: Clamp response sizes via `MAX_PAGE_SIZE`

**Files:**
- Modify: `rs/src/bin/duapi/handler.rs` (folders + files handlers)

**Why:** Audit item #6 (second half) — `MAX_PAGE_SIZE` is documented but unused. Pathological queries on a 1B-file index could serialize gigabytes of JSON. We clamp + log when truncated. (Pagination is out of scope for this plan — clamp-and-warn is the smallest fix that closes the DoS surface.)

- [ ] **Step 1: Add a small helper**

Append to `rs/src/bin/duapi/query.rs`:

```rust
pub fn max_page_size() -> usize {
    std::env::var("MAX_PAGE_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2000)
}
```

Add a unit test for it in the same file's `mod tests`:

```rust
#[test]
fn test_max_page_size_default() {
    // SAFETY: serial_test would isolate, but for a default-only check we just unset.
    unsafe { std::env::remove_var("MAX_PAGE_SIZE") };
    assert_eq!(max_page_size(), 2000);
}
```

- [ ] **Step 2: Run the helper test**

Run: `cd rs && cargo test --bin duapi max_page_size`
Expected: PASS.

- [ ] **Step 3: Apply the clamp in `get_folders_handler`**

Replace the existing `Ok(v) => { ... }` arm in `get_folders_handler` (around handler.rs line 104) with:

```rust
        Ok(mut v) => {
            let cap = crate::query::max_page_size();
            if v.len() > cap {
                tracing::warn!(path = %path, total = v.len(), cap, "/api/folders truncated");
                v.truncate(cap);
            }
            tracing::info!(path = %path, items = v.len(), "200 OK /api/folders");
            v
        }
```

- [ ] **Step 4: Apply the clamp in `get_files_handler`**

Replace the `Ok(Ok(items)) => { ... }` arm at the end of `get_files_handler` with:

```rust
        Ok(Ok(mut items)) => {
            let cap = crate::query::max_page_size();
            if items.len() > cap {
                tracing::warn!(total = items.len(), cap, "/api/files truncated");
                items.truncate(cap);
            }
            tracing::info!(items = items.len(), "200 OK /api/files");
            Json(items).into_response()
        }
```

- [ ] **Step 5: Add a clamp-behavior test**

Add inside the `mod tests` block in `rs/src/bin/duapi/handler.rs`:

```rust
#[tokio::test]
#[serial]
async fn test_get_folders_handler_clamps_to_max_page_size() {
    init_index_once();
    // SAFETY: we set then restore for test isolation.
    unsafe { std::env::set_var("MAX_PAGE_SIZE", "1") };
    let admin = Claims {
        sub: "root".into(),
        is_admin: true,
        exp: 9_999_999_999usize,
    };
    let q = FolderQuery {
        path: Some("/".into()),
        users: None,
        age: None,
    };
    let resp = get_folders_handler(admin, Query(q)).await.into_response();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), TEST_BODY_LIMIT).await.unwrap();
    let arr: Vec<FolderOut> = serde_json::from_slice(&body).unwrap();
    assert!(arr.len() <= 1);
    unsafe { std::env::remove_var("MAX_PAGE_SIZE") };
}
```

- [ ] **Step 6: Run the duapi test suite**

Run: `cd rs && cargo test --bin duapi`
Expected: all pass, including the new clamp test.

- [ ] **Step 7: Stage changes**

```bash
git add rs/src/bin/duapi/handler.rs rs/src/bin/duapi/query.rs
git status
```

**Do not commit.**

---

## Task 7: Graceful shutdown on SIGINT/SIGTERM

**Files:**
- Create: `rs/src/bin/duapi/shutdown.rs`
- Modify: `rs/src/bin/duapi/main.rs`

**Why:** Audit item #10. `systemctl stop` and `docker stop` send SIGTERM. Without a handler, in-flight requests are killed mid-response and TLS handshakes can leak.

- [ ] **Step 1: Write the failing test**

Create `rs/src/bin/duapi/shutdown.rs` with a test stub:

```rust
// rs/src/bin/duapi/shutdown.rs

/// Future that completes when the process receives Ctrl+C (SIGINT) or SIGTERM (Unix).
pub async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        if let Err(e) = signal::ctrl_c().await {
            tracing::warn!(error = %e, "failed to install Ctrl+C handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match signal::unix::signal(signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("received Ctrl+C, shutting down"),
        _ = terminate => tracing::info!("received SIGTERM, shutting down"),
    }
}

#[cfg(test)]
mod tests {
    use super::shutdown_signal;
    use std::time::Duration;

    /// Smoke test: the future does not complete on its own within a short window.
    #[tokio::test]
    async fn test_shutdown_signal_pending_without_signal() {
        let res = tokio::time::timeout(Duration::from_millis(50), shutdown_signal()).await;
        assert!(res.is_err(), "shutdown_signal completed without a signal");
    }
}
```

- [ ] **Step 2: Wire the module into duapi**

In `rs/src/bin/duapi/main.rs`, add to the `mod` declarations (currently lines 21–24):

```rust
mod handler;
mod index;
mod item;
mod query;
mod shutdown;
```

- [ ] **Step 3: Run the smoke test**

Run: `cd rs && cargo test --bin duapi shutdown_signal_pending_without_signal`
Expected: PASS.

- [ ] **Step 4: Wire shutdown into the plain HTTP serve path**

In `rs/src/bin/duapi/main.rs`, replace the current `(None, None) => { ... }` arm of the TLS `match` (currently around line 176–179) with:

```rust
        (None, None) => {
            println!("Serving on http://{addr}  (static dir: {static_dir})");
            let listener = tokio::net::TcpListener::bind(addr).await?;
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown::shutdown_signal())
                .await?;
        }
```

- [ ] **Step 5: Wire shutdown into the TLS serve path**

Replace the current `(Some(cert_path), Some(key_path)) => { ... }` arm with the version below. The change is the addition of the `axum_server::Handle` and the spawned task driving `graceful_shutdown` when the signal fires:

```rust
        (Some(cert_path), Some(key_path)) => {
            if !cert_path.exists() {
                eprintln!(
                    "{}",
                    format!("Error: Certificate file not found: {}", cert_path.display()).red()
                );
                std::process::exit(1);
            }
            if !key_path.exists() {
                eprintln!(
                    "{}",
                    format!("Error: Key file not found: {}", key_path.display()).red()
                );
                std::process::exit(1);
            }

            println!(
                "Loading TLS certificate from {} and key from {}",
                cert_path.display(),
                key_path.display()
            );

            let config = RustlsConfig::from_pem_file(cert_path, key_path)
                .await
                .context("Failed to load TLS certificate/key")?;

            println!("Serving on https://{addr}  (static dir: {static_dir})");

            let handle = axum_server::Handle::new();
            let shutdown_handle = handle.clone();
            tokio::spawn(async move {
                shutdown::shutdown_signal().await;
                shutdown_handle.graceful_shutdown(Some(Duration::from_secs(30)));
            });

            axum_server::bind_rustls(addr, config)
                .handle(handle)
                .serve(app.into_make_service())
                .await?;
        }
```

- [ ] **Step 6: Build the release binary**

Run: `cd rs && cargo build --release --bin duapi`
Expected: clean build.

- [ ] **Step 7: Manual end-to-end test**

```bash
# terminal 1
cd /Users/san/dev/dutopia
JWT_SECRET=test ./rs/target/release/duapi /tmp/empty.csv --port 18081 &
SERVER_PID=$!

# terminal 2 — issue a request, then send SIGTERM during it
sleep 0.5
curl -s http://127.0.0.1:18081/api/health
kill -TERM $SERVER_PID
wait $SERVER_PID
echo "exit: $?"
```

Expected: the server prints `received SIGTERM, shutting down` (JSON form) and exits 0. Without this fix it would exit on the dropped TCP listener with a non-zero code or stack trace.

- [ ] **Step 8: Stage changes**

```bash
git add rs/src/bin/duapi/main.rs rs/src/bin/duapi/shutdown.rs
git status
```

**Do not commit.**

---

## Task 8: Final verification

- [ ] **Step 1: Full test suite**

Run: `cd rs && cargo test --release`
Expected: PASS, no warnings.

- [ ] **Step 2: Clippy**

Run: `cd rs && cargo clippy --all-targets --release -- -D warnings`
Expected: no warnings.

- [ ] **Step 3: Build all binaries**

Run: `cd rs && cargo build --release`
Expected: clean build.

- [ ] **Step 4: Smoke-test the full pipeline against `/tmp`**

```bash
cd /Users/san/dev/dutopia
./rs/target/release/duscan /tmp -o /tmp/scan.csv
./rs/target/release/dusum /tmp/scan.csv -o /tmp/sum.csv
JWT_SECRET=smoke RATE_LIMIT_PER_MIN=120 REQUEST_TIMEOUT_SECS=10 \
  ./rs/target/release/duapi /tmp/sum.csv --port 18082 &
SERVER_PID=$!
sleep 1
curl -s http://127.0.0.1:18082/api/health
curl -s -o /dev/null -w "traversal: %{http_code}\n" \
  -H "Authorization: Bearer x" \
  "http://127.0.0.1:18082/api/files?path=/tmp/../etc/passwd"
kill -TERM $SERVER_PID
wait $SERVER_PID
```

Expected:
- `/api/health` → `{"status":"ok"}`
- `traversal: 400` (or `401` if blocked at auth before normalize — also acceptable)
- Server exits cleanly after SIGTERM

- [ ] **Step 5: Final `git status` review**

```bash
git status
git diff --stat HEAD
```

Expected: only the files listed in the "File Structure" section appear modified/created. **Do NOT commit — the user handles commits.**

---

## Self-review notes

- **Spec coverage:** HIGH items 6 (rate limit + page size: Tasks 5, 6), 7 (path traversal: Task 1), 8 (println→tracing in auth: Task 2), 9 (CORS unwrap: Task 3 — the other unwraps at main.rs:195/214/218 are not in the request path and don't fire under any reachable input; intentionally deferred), 10 (graceful shutdown: Task 7). Bonus: REQUEST_TIMEOUT_SECS + MAX_BODY_BYTES (Task 4) — close to the rate-limit gap, cheap to ship together.
- **No placeholders:** every step shows the actual code or command.
- **Type consistency:** `normalize_path` returns `Option<String>` everywhere it appears; `parse_cors_origin` returns `Result<HeaderValue, anyhow::Error>` and is consumed by `.allow_origin(header)`; `env_u64` and `max_page_size` are both `fn(...) -> u64`/`usize` with default-on-failure semantics. `shutdown_signal` is `async fn -> ()`.
