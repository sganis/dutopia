# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Dutopia is a Rust toolkit for high-scale filesystem analytics. It turns massive filesystems (tested on >1B files, 30PB storage) into fast, filterable, UTF-8-clean analytics via a modular pipeline of CLI binaries, served through a REST API with a Svelte SPA dashboard.

## Build & Development Commands

### Rust Backend

```bash
cargo build --release        # Build all binaries
cargo test                   # Run all tests
cargo test test_name         # Run a specific test
cargo test -- --nocapture    # Run tests with stdout visible
cargo check                  # Type-check without building
cargo clippy                 # Lint
```

### Browser Frontend (browser/)

```bash
cd browser
npm install                  # Install dependencies
npm run dev                  # Dev server on port 5173
npm run build                # Production build (outputs to browser/public/)
npm run check                # TypeScript/Svelte type checking
```

### Running the Full Pipeline

```bash
./duscan /path -o scan.csv               # 1. Scan filesystem
./dusum scan.csv -o sum.csv              # 2. Aggregate by folder/user/age
./dudb sum.csv --raw scan.csv -o du.db   # 3. Build SQLite DB (--raw adds the file index)
./duapi du.db --port 8000                # 4. Serve REST API + frontend
```

## Architecture

### Pipeline

```
filesystem → duscan → [raw CSV] → dusum → [aggregated CSV] → dudb → [SQLite DB] → duapi → REST API → Svelte SPA
                          │                                    ↑
                          └────────────── --raw ───────────────┘
                              duzip (optional CSV ↔ .zst compression)
                              duhuman (optional human-readable conversion)
```

### Binaries (src/bin/)

Each binary is a directory with focused modules under `src/bin/<name>/`:

- **duscan** — Multi-threaded filesystem scanner. Streams metadata as CSV/zstd with 32MB buffer batching.
- **dusum** — Aggregates raw scan data into rollups by folder, user, and age bucket (0=<60d, 1=60-600d, 2=>600d).
- **dudb** — Builds the SQLite DB from the dusum CSV (`paths`/`users`/`stats` tables). With `--raw scan.csv` it also builds the file index so `/api/files` is served from the DB instead of a live filesystem read (required in production where the API host cannot see the scanned filesystem): `folder_files` holds one `filepack` blob per folder: totals of all its files per (user, age), plus only the `--max-files` (default 100) largest files of at least `--min-file-size` (default 1M) per (folder, user, age). The raw pass external-sorts rows by (folder_id, name) (`extsort.rs`) so each folder's files arrive together, then `folder_pack.rs` totals, caps and packs them. `--no-files` builds a folders-only DB on purpose (duapi then lists no files and never reads the disk). Builds into `<output>.building`, renames on success, and leaves the DB in DELETE journal mode. Resolves uid→username via the local account database, so run it on a host in the same identity domain as dusum.
- **duapi** — Axum REST API with JWT auth over the dudb SQLite DB, optional TLS. Serves the Svelte SPA from `browser/public/`. `/api/files` returns `{ files, totals }`: from the DB `folder_files` index when present (`has_files` metadata), largest first and clamped to `files_cap`; nothing for a `--no-files` DB; else a live `read_dir`. Boots without a usable DB (missing file / wrong schema): SPA, login and health still serve, DB endpoints return 503. `/api/health` returns `db_version` (the DB's `built_at`) so the SPA can drop its cache when the DB is rebuilt. With `FILESYSTEMS` set, the home page lists the filesystems and `/api/trash` keeps the trash cart in `$TRASH_DIR/queue.json` (`trash.rs`): Keep is the 24h undo, then `script/trash_mover.py` (root cron on a node that mounts the filesystems) claims the items, moves them to `<fs>/.trash/<date>/…` (folders as `.tar.zst`) and purges after 30 days.
- **duxray** — Storage X-ray: one-pass statistics over a duscan raw CSV (size bands, age/abandonment by owner and project, junk files, permission red flags, top-N). Constant memory; `--hardlinks` dedups inodes.
- **duhuman** — Converts machine data (epochs, UIDs, mode bits) to human-readable format. Single-file binary.
- **duzip** — Bidirectional CSV ↔ Zstandard compression. Single-file binary.
- **dumachine** — Binary data processor. Single-file binary.

### Shared Library (src/)

- `lib.rs` — Re-exports util, auth, authz, storage, db, filepack, filesystems, item, query, analytic
- `auth.rs` — JWT authentication with platform-specific credential verification (macOS: dscl, Linux: su). `Claims` carries `sub`, `is_admin`, `roots`
- `authz.rs` — Three tiers shared by REST and MCP: global admin (`GLOBAL_ADMINS`), storage admin (`STORAGE_ADMINS`: full admin inside assigned roots, self-only elsewhere, may navigate the ancestors of their roots), regular user (own files only). `check_access` returns `Full` or `Navigable`
- `db.rs` — Read-only SQLite pool + queries over the dudb DB (`list_children`, `list_files`, `has_files`, `files_cap`, `built_at`). Falls back to the `unix-none` VFS when the mount has no POSIX locks (NFSv3 `local_lock=none`); dudb does the same when building
- `filesystems.rs` — `FILESYSTEMS` parsing and `fs_of` (longest mount-point prefix, `/`-bounded)
- `filepack.rs` — Binary format of the per-folder file blobs (columnar varints + zstd), shared by dudb and duapi
- `item.rs` — Live-filesystem file listing (`get_items`), the /api/files fallback when the DB has no file index
- `storage.rs` — Cross-platform storage info (Unix: statvfs, Windows: Win32 API)
- `util/` — Row struct, CSV helpers, age bucketing (shared by dusum/dudb), human formatting, path utilities, platform-specific functions (incl. uid→username)

### Frontend (browser/)

SvelteKit 2 + Svelte 5 SPA with Tailwind CSS 4. Uses adapter-static (no SSR).

- `src/routes/+page.svelte` — Main dashboard page
- `src/lib/` — Extracted components (PascalCase): Login, TreeMap, FolderBar, FileBar, PathStats, AgeFilter, SortDropdown, Tooltip, Picker*
- `src/ts/` — TypeScript modules (lowercase): api.svelte.ts (API client with $state), cache.ts (IndexedDB, stale-while-revalidate, 1min soft TTL, wiped when `db_version` changes), store.svelte.ts (global state), util.ts (formatting/colors/paths)

### API Endpoints

```
POST /api/login                         # Returns JWT
GET  /api/users                         # List usernames
GET  /api/folders?path=&users=&age=     # Folder statistics
GET  /api/files?path=&users=&age=       # File listing
GET  /api/trash                         # Trash cart: items the caller may manage
POST /api/trash                         # Queue {path,size,owner} for the trash
DELETE /api/trash?path=                 # Keep: the 24h undo
POST /api/trash/claim                   # Cron only (Bearer TRASH_TOKEN): items queued >24h, removed
```

### CSV Format (scanner output)

```
INODE,ATIME,MTIME,UID,GID,MODE,SIZE,DISK,PATH
```

## Platform-Specific Code

Uses `#[cfg(...)]` conditional compilation throughout. Key differences:
- **Auth**: macOS uses `dscl`, Linux uses `su`, Windows uses env-var fake auth
- **Storage**: Unix uses `statvfs`/`getmntinfo`, Windows uses `GetDiskFreeSpaceExW`
- **Scanning**: Unix uses `as_os_str().as_bytes()` for zero-copy path handling

## Environment Variables

- `JWT_SECRET` — Secret for JWT token signing (duapi)
- `GLOBAL_ADMINS` — Comma-separated global admins: usernames, or Keycloak groups/realm roles on OIDC (duapi). `ADMIN_GROUP` is the deprecated name, still honored
- `STORAGE_ADMINS` — Scoped admins, e.g. `user1:/red/peddn7,/red/peddn8;user2:/red/ssd` (duapi)
- `FILESYSTEMS` — Comma-separated mount points duscan scans (duapi). The home page (`/api/folders?path=`) lists one row per filesystem instead of `/`; only paths inside them can be trashed. `/api/health` returns the list
- `TRASH_DIR` — Folder of the trash queue `queue.json` (duapi, default `/data/trash`)
- `TRASH_TOKEN` — Bearer token the trash cron (`script/trash_mover.py`) sends to `POST /api/trash/claim`; claim is refused when unset
- `ADMIN_PASSWORD` — Password login with this value succeeds for any username, as global admin. Active in release builds too (container mode with no local accounts), so treat it as a secret

## CI/CD

GitHub Actions on push to master (Ubuntu only). Runs `cargo test -r` and `npm run build`. Auto-bumps patch version via cargo-edit on release and syncs `browser/package.json` to it (the SPA header shows that version).

## Constraints

- Do not run `git commit` — commits are handled externally.
- Follow the Vibe Coding Standards from the user's global CLAUDE.md (file headers with paths, 300-500 line target, 600 line hard limit, naming conventions).
