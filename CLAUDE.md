# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Dutopia is a Rust toolkit for high-scale filesystem analytics. It turns massive filesystems (tested on >1B files, 30PB storage) into fast, filterable, UTF-8-clean analytics via a modular pipeline of CLI binaries, served through a REST API with a Svelte SPA dashboard.

## Build & Development Commands

### Rust Backend (rs/)

```bash
cd rs
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
npm run build                # Production build (outputs to build/)
npm run check                # TypeScript/Svelte type checking
```

### Running the Full Pipeline

```bash
./duscan /path -o scan.csv        # 1. Scan filesystem
./dusum scan.csv -o sum.csv       # 2. Aggregate by folder/user/age
./duapi sum.csv --port 8000       # 3. Serve REST API + frontend
```

## Architecture

### Pipeline

```
filesystem → duscan → [raw CSV] → dusum → [aggregated CSV] → duapi → REST API → Svelte SPA
                                                                ↑
                              duzip (optional CSV ↔ .zst compression)
                              duhuman (optional human-readable conversion)
```

### Binaries (rs/src/bin/)

Each binary is a directory with focused modules under `rs/src/bin/<name>/`:

- **duscan** — Multi-threaded filesystem scanner. Streams metadata as CSV/zstd with 32MB buffer batching.
- **dusum** — Aggregates raw scan data into rollups by folder, user, and age bucket (0=<60d, 1=60-600d, 2=>600d).
- **duapi** — Axum REST API with JWT auth, in-memory trie-based filesystem index, optional TLS. Serves the Svelte SPA from `browser/public/`.
- **duhuman** — Converts machine data (epochs, UIDs, mode bits) to human-readable format. Single-file binary.
- **duzip** — Bidirectional CSV ↔ Zstandard compression. Single-file binary.
- **dumachine** — Binary data processor. Single-file binary.

### Shared Library (rs/src/)

- `lib.rs` — Re-exports util, auth, storage
- `auth.rs` — JWT authentication with platform-specific credential verification (macOS: dscl, Linux: su)
- `storage.rs` — Cross-platform storage info (Unix: statvfs, Windows: Win32 API)
- `util/` — Row struct, CSV helpers, human formatting, path utilities, platform-specific filesystem functions

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

GitHub Actions on push to master (Ubuntu only). Runs `cargo test -r` and `npm run build`. Auto-bumps patch version via cargo-edit on release.

## Constraints

- Do not run `git commit` — commits are handled externally.
- Follow the Vibe Coding Standards from the user's global CLAUDE.md (file headers with paths, 300-500 line target, 600 line hard limit, naming conventions).
