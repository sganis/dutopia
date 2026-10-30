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
- **dudb** — Builds the SQLite DB from the dusum CSV (`paths`/`users`/`stats` tables). With `--raw scan.csv` it also builds the file index so `/api/files` is served from the DB instead of a live filesystem read (required in production where the API host cannot see the scanned filesystem): `folder_files` holds one zstd-compressed `filepack` blob per folder (~18 B/file vs ~57 B for the old one-row-per-file v3 table), keeping only the `--max-files` (default 1000) largest files per (folder, user, age). The raw pass external-sorts rows by (folder_id, name) (`extsort.rs`) so each folder's files arrive together, then `folder_pack.rs` caps and packs them. Builds into `<output>.building`, renames on success, and leaves the DB in DELETE journal mode. Resolves uid→username via the local account database, so run it on a host in the same identity domain as dusum.
- **duapi** — Axum REST API with JWT auth over the dudb SQLite DB, optional TLS. Serves the Svelte SPA from `browser/public/`. `/api/files` uses the DB `folder_files` index when present (`has_files` metadata), largest first and clamped to `files_cap`, else falls back to a live `read_dir`.
- **duhuman** — Converts machine data (epochs, UIDs, mode bits) to human-readable format. Single-file binary.
- **duzip** — Bidirectional CSV ↔ Zstandard compression. Single-file binary.
- **dumachine** — Binary data processor. Single-file binary.

### Shared Library (src/)

- `lib.rs` — Re-exports util, auth, storage, db, filepack, item, query, analytic
- `auth.rs` — JWT authentication with platform-specific credential verification (macOS: dscl, Linux: su)
- `db.rs` — Read-only SQLite pool + queries over the dudb DB (`list_children`, `list_files`, `has_files`, `files_cap`)
- `filepack.rs` — Binary format of the per-folder file blobs (columnar varints + zstd), shared by dudb and duapi
- `item.rs` — Live-filesystem file listing (`get_items`), the /api/files fallback when the DB has no file index
- `storage.rs` — Cross-platform storage info (Unix: statvfs, Windows: Win32 API)
- `util/` — Row struct, CSV helpers, age bucketing (shared by dusum/dudb), human formatting, path utilities, platform-specific functions (incl. uid→username)

### Frontend (browser/)

SvelteKit 2 + Svelte 5 SPA with Tailwind CSS 4. Uses adapter-static (no SSR).

- `src/routes/+page.svelte` — Main dashboard page
- `src/lib/` — Extracted components (PascalCase): Login, TreeMap, FolderBar, FileBar, PathStats, AgeFilter, SortDropdown, Tooltip, Picker*
- `src/ts/` — TypeScript modules (lowercase): api.svelte.ts (API client with $state), cache.ts (IndexedDB, 1min TTL), store.svelte.ts (global state), util.ts (formatting/colors/paths)

### API Endpoints

```
POST /api/login                         # Returns JWT
GET  /api/users                         # List usernames
GET  /api/folders?path=&users=&age=     # Folder statistics
GET  /api/files?path=&users=&age=       # File listing
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
- `ADMIN_GROUP` — Optional admin group check (duapi)

## CI/CD

GitHub Actions on push to master (Ubuntu only). Runs `cargo test -r` and `npm run build`. Auto-bumps patch version via cargo-edit on release and syncs `browser/package.json` to it (the SPA header shows that version).

## Constraints

- Do not run `git commit` — commits are handled externally.
- Follow the Vibe Coding Standards from the user's global CLAUDE.md (file headers with paths, 300-500 line target, 600 line hard limit, naming conventions).
