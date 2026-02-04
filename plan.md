# Dutopia Production Readiness Plan

## Assessment Summary

**Overall Verdict: NOT production-ready.** The codebase is well-architected but has critical security gaps, missing operational infrastructure, and insufficient documentation for users/operators.

| Area | Grade | Blocking? |
|------|-------|-----------|
| Security | D+ | YES |
| Error Handling | C+ | YES |
| API Robustness | C | YES |
| Concurrency | B | No |
| Code Structure | B+ | No |
| Testing | C+ | Soft yes |
| Frontend Safety | C | YES |
| Logging/Observability | F | YES |
| Deployment/Ops | F | YES |
| Documentation | C | YES |

---

## Phase 1: Critical Bug Fixes

### 1.0 Remove committed secrets and add hygiene guardrails
- **File:** `.env`
- **Bug:** Real secret committed to repo (security incident)
- **Fix:** Remove `.env` from repo, rotate leaked key, add `.env.example`, and ensure `.env` is in `.gitignore`.

### 1.1 Remove default insecure JWT secret
- **File:** `rs/src/bin/duapi/main.rs:59-67`
- **Bug:** Falls back to `"1234567890abcdef"` if `JWT_SECRET` not set — any attacker can forge tokens
- **Fix:** Panic on missing `JWT_SECRET` with clear error message. Remove the `unsafe { set_var }` block entirely.

### 1.2 Fix token expiry not enforced in frontend
- **File:** `browser/src/lib/Login.svelte:56`
- **Bug:** `State.expiresAt` is never set (line is commented out), so tokens live forever in localStorage
- **Fix:** Uncomment and wire `expiresAt` into State. The check in `+layout.svelte:9-16` already uses it.

### 1.3 Fix XSS in user color renderer
- **File:** `browser/src/routes/+page.svelte:97-118`
- **Bug:** `colorRenderer()` interpolates `item.user` into raw HTML string without escaping
- **Fix:** Escape HTML entities in `item.user` before interpolation (add a `escapeHtml()` utility).

### 1.4 Clear cache on logout
- **File:** `browser/src/ts/cache.ts` + `browser/src/lib/Login.svelte` (or wherever logout lives)
- **Bug:** IndexedDB cache is not cleared on logout — next user sees stale data
- **Fix:** Add `clearCache()` function and call it on logout.

### 1.5 Fix silent write failures in scanner
- **File:** `rs/src/bin/duscan/worker.rs:118,154,169`
- **Bug:** `let _ = writer.write_all(&buf)` and `let _ = writer.flush()` silently ignore I/O errors — data loss risk
- **Fix:** Propagate errors or at minimum log them and increment an error counter.

### 1.6 Replace panicking `expect()` calls with proper error handling
- **Files:**
  - `rs/src/bin/duscan/main.rs:304,316` — worker join and merge panics
  - `rs/src/bin/duscan/worker.rs:71,78` — shard file creation panics
  - `rs/src/bin/duapi/main.rs:103-104` — OnceLock panics
- **Fix:** Convert to `anyhow::Result` propagation or graceful error messages.

### 1.7 Make CORS origins configurable
- **File:** `rs/src/bin/duapi/main.rs:106-109`
- **Bug:** Hardcoded `http://localhost:5173` — won't work in any deployment
- **Fix:** Accept `CORS_ORIGIN` env var (or `--cors-origin` flag), default to same-origin.

### 1.8 Enforce base path for API queries
- **Files:** `rs/src/bin/duapi/handler.rs`, `rs/src/bin/duapi/item.rs`
- **Bug:** `/api/files` and `/api/folders` accept arbitrary paths
- **Fix:** Add `BASE_PATH` config and reject any query outside it (normalize and validate).

### 1.9 Fix platform auth reliability/security
- **File:** `rs/src/auth.rs`
- **Bug:** Linux `su` often fails without a TTY; macOS `dscl` passes password as process arg
- **Fix:** Replace with PAM or native OS auth APIs; document OS support.

---

## Phase 2: Essential Features (Production Blockers)

### 2.1 Structured logging
- **All Rust binaries** currently use `println!`/`eprintln!`
- Add `tracing` crate with `tracing-subscriber` for structured JSON logs
- Add request logging middleware to duapi (method, path, status, duration)
- Minimum: INFO level for requests, WARN for errors, ERROR for panics

### 2.2 Health check endpoint
- **File:** `rs/src/bin/duapi/handler.rs`
- Add `GET /api/health` returning `{"status":"ok","version":"4.x.x","uptime_secs":N}`
- Essential for load balancers, monitoring, container orchestration

### 2.3 Graceful shutdown
- **File:** `rs/src/bin/duapi/main.rs`
- Handle SIGTERM/SIGINT — drain active connections, then exit
- Tokio has built-in `signal::ctrl_c()` support

### 2.4 Request timeout + basic rate limiting
- **File:** `rs/src/bin/duapi/main.rs`
- Add `tower::timeout::TimeoutLayer` (30s default)
- Add basic rate limiting with `tower-governor` or simple token bucket per IP

### 2.5 Disable sourcemaps in production build
- **File:** `browser/vite.config.ts:6`
- Change `sourcemap: true` to `sourcemap: process.env.NODE_ENV !== 'production'`

### 2.6 Add pagination and hard limits for list endpoints
- **Files:** `rs/src/bin/duapi/handler.rs`, `rs/src/bin/duapi/index.rs`
- **Fix:** Add `limit`/`offset` (or `cursor`) for `/api/folders` and `/api/files`, and enforce max defaults.

---

## Phase 3: Documentation

### 3.1 User Guide (`doc/guide.md`)
Write a comprehensive user-facing guide covering:
- **Installation** — building from source, pre-built binaries (when available)
- **Quick Start** — scan → summarize → serve pipeline with real examples
- **CLI Reference** — every binary, every flag, with examples:
  - `duscan` — all options (threads, output format, skip patterns, verbosity)
  - `dusum` — aggregation options
  - `duapi` — server config (port, TLS, JWT, CORS, static dir)
  - `duzip` — compression/decompression
  - `duhuman` — human-readable conversion
  - `dumachine` — machine format conversion
- **Authentication** — how JWT works, setting `JWT_SECRET`, admin groups
- **TLS Setup** — using `script/cert.sh`, production certificates
- **CSV Schema** — input/output formats documented
- **Age Buckets** — explain 0/1/2 mapping (0=<60d, 1=60-600d, 2=>600d)
- **Troubleshooting** — common errors and fixes

### 3.2 API Reference (`doc/api.md`)
Document all REST endpoints:
- `POST /api/login` — request/response schema, error codes
- `GET /api/users` — response schema
- `GET /api/folders` — query params, response schema, filtering behavior
- `GET /api/files` — query params, response schema
- `GET /api/health` — (new endpoint from 2.2)
- Authentication header format
- Error response format

### 3.3 Deployment Guide (`doc/deploy.md`)
- **Docker** — write a multi-stage Dockerfile (build Rust + Node, slim runtime image)
- **docker-compose** — full pipeline example (scan volume, serve API)
- **Systemd** — unit file for duapi as a service
- **Environment Variables** — complete reference table with defaults
- **Security Hardening** — production JWT secret, TLS, firewall rules, non-root user
- **Reverse Proxy** — nginx/caddy config examples for TLS termination

### 3.4 Technical Specification (`doc/architecture.md`)
- Pipeline architecture diagram (text-based)
- Data flow: filesystem → CSV → aggregation → trie index → REST → SPA
- Module dependency graph
- Platform-specific code map
- Performance characteristics (tested scale, memory usage, throughput)
- CSV binary format specification (for duzip)
- Trie index structure (duapi/index.rs)
- Concurrency model (duscan worker pool, channel-based work stealing)

### 3.5 Create `.env.example`
- Template with all environment variables, documented with comments
- Remove any real secrets from repo history note

---

## Phase 4: Supporting Improvements

### 4.1 Fix `+page.svelte` file size violation
- **File:** `browser/src/routes/+page.svelte` (726 lines, exceeds 600 hard limit)
- Extract: navigation logic → `PageNav.svelte`, tooltip logic → `tooltip.ts` util
- Target: under 500 lines

### 4.2 Add missing tests for auth module
- **File:** `rs/src/auth.rs` — currently 0 tests
- Add tests for JWT creation, validation, expiry, invalid tokens
- Mock platform-specific `verify_user` (skip dscl/su in tests)

### 4.3 Fix accessibility violations
- **Files:** `browser/src/lib/FolderBar.svelte:103-104`, `browser/src/lib/FileBar.svelte:60`
- Replace `<div onclick>` with `<button>` elements
- Remove `svelte-ignore a11y_*` suppressions

### 4.4 Add API error states to frontend
- **File:** `browser/src/routes/+page.svelte:428-440`
- Show error UI when `api.getFolders` or `api.getFiles` fails
- Currently shows stale data with no indication of failure

### 4.5 Add fetch timeout in frontend
- **File:** `browser/src/ts/api.svelte.ts`
- Add `AbortController` with 30s timeout to all fetch calls

---

## File Change Summary

| File | Action |
|------|--------|
| `rs/src/bin/duapi/main.rs` | Fix JWT default, CORS config, add graceful shutdown, health route |
| `rs/src/bin/duapi/handler.rs` | Add health endpoint, request logging |
| `rs/src/bin/duscan/main.rs` | Replace expect() with error handling |
| `rs/src/bin/duscan/worker.rs` | Fix silent write failures, shard panics |
| `rs/src/auth.rs` | Add tests (new test module) |
| `rs/Cargo.toml` | Add `tracing`, `tracing-subscriber`, `tower` deps |
| `browser/src/routes/+page.svelte` | Fix XSS, extract components to reduce size |
| `browser/src/lib/Login.svelte` | Enable token expiry, clear cache on logout |
| `browser/src/ts/cache.ts` | Add `clearCache()` export |
| `browser/src/ts/api.svelte.ts` | Add fetch timeout |
| `browser/src/ts/util.ts` | Add `escapeHtml()` utility |
| `browser/src/lib/FolderBar.svelte` | Fix a11y (use buttons) |
| `browser/src/lib/FileBar.svelte` | Fix a11y (use buttons) |
| `browser/vite.config.ts` | Conditional sourcemaps |
| `doc/guide.md` | NEW — User guide |
| `doc/api.md` | NEW — API reference |
| `doc/deploy.md` | NEW — Deployment guide |
| `doc/architecture.md` | NEW — Technical specification |
| `.env.example` | NEW — Environment variable template |
| `browser/src/lib/PageNav.svelte` | NEW — Extracted navigation component |

---

## Verification Plan

1. **Rust backend:** `cd rs && cargo check && cargo test && cargo clippy`
2. **Frontend:** `cd browser && npm run check && npm run build`
3. **Security:** Verify duapi refuses to start without `JWT_SECRET`
4. **Health check:** `curl http://localhost:8080/api/health`
5. **CORS:** Verify configurable origin works with frontend
6. **Token expiry:** Log in, wait for expiry, verify redirect to login
7. **XSS:** Test username with `<script>` tag in color picker
8. **Cache clear:** Log out, verify IndexedDB is empty
9. **Docs:** Read each doc file for completeness and accuracy
10. **a11y:** Tab through FolderBar/FileBar, verify keyboard navigation works

---

## Execution Order

1. Phase 1 (bug fixes) — all items, security-critical first
2. Phase 2 (essential features) — logging first, then health/shutdown/rate-limit
3. Phase 3 (documentation) — all docs in parallel
4. Phase 4 (improvements) — file splitting, tests, a11y, error states
