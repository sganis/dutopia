# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Dutopia is a high-performance Rust toolkit for filesystem analytics at scale (tested on 1B+ files, 30PB storage). It provides modular CLI tools for scanning, aggregating, and serving filesystem metadata.

## Build Commands

```bash
# Build all Rust binaries
cd rs && cargo build --release

# Run tests
cd rs && cargo test

# Run a specific test
cd rs && cargo test test_name

# Check without building
cd rs && cargo check
```

## Frontend (Svelte SPA)

```bash
cd browser
npm install
npm run dev      # Development server
npm run build    # Production build
npm run check    # TypeScript check
```

## Architecture

### Rust Binaries (`rs/src/bin/`)
- **duscan** - Filesystem scanner with multi-threaded traversal, outputs CSV or zstd-compressed binary
- **duhuman** - Converts machine data (epochs, uids) to human-readable format
- **dusum** - Aggregates scan output by folder, user, and file-age buckets
- **duapi** - REST API server (Axum) exposing aggregated data with JWT auth
- **duzip** - CSV ↔ Zstandard compression utility
- **dumachine** - Machine data processor

### Shared Library (`rs/src/`)
- `lib.rs` - Re-exports util, auth, storage modules
- `util.rs` - Path helpers, CSV formatters, progress display, platform-specific filesystem functions
- `auth.rs` - JWT authentication, platform-specific user verification (PAM on Linux, dscl on macOS)
- `storage.rs` - Storage abstractions

### Frontend (`browser/`)
- SvelteKit 5 with TailwindCSS 4
- Static adapter for deployment alongside the API

### Python (`python/`)
- Legacy `statwalker` package for filesystem walking (Python 2.7)

## CSV Format

Scanner output columns: `INODE,ATIME,MTIME,UID,GID,MODE,SIZE,DISK,PATH`
- INODE: `device-inode` format
- ATIME/MTIME: Unix epoch seconds
- SIZE: Logical file size
- DISK: Actual disk usage (blocks × 512)

## Key Patterns

- Multi-threaded scanning via crossbeam channels with work-stealing
- Shard files per worker, merged at the end
- Binary output uses zstd compression with little-endian encoding
- JWT tokens for API authentication with platform-specific credential verification
- Age buckets: 0=recent (≤60d), 1=mid (≤730d), 2=old (>730d)

## Environment Variables

- `JWT_SECRET` - Required for API authentication
- `ADMIN_GROUP` - Comma-separated list of admin usernames
- `STATIC_DIR` - Path to frontend static files
- `PORT` - API server port (default: 8080)
- `TLS_CERT`/`TLS_KEY` - HTTPS certificate paths

---

## Improvement Plan

### Completed

| Task | Status |
|------|--------|
| UID→username caching in duapi | ✅ Done (`item.rs:37-44`) |
| Refactor duscan.rs (2,179 lines) | ✅ Done: 5 modules (main, worker, csv, merge, row) |
| Refactor duapi.rs (1,043 lines) | ✅ Done: 5 modules (main, handler, index, item, query) |
| Refactor util.rs (1,051 lines) | ✅ Done: 5 modules (mod, row, format, path, csv, platform) |
| Refactor duzip.rs (958 lines) | ✅ Done: 4 modules (main, record, compress, decompress) |
| Refactor dusum.rs (798 lines) | ✅ Done: 4 modules (main, stats, aggregate, output) |
| Refactor +page.svelte (1,079 lines) | ✅ Done: 6 components (Tooltip, AgeFilter, SortDropdown, FolderBar, FileBar, PathStats) → 726 lines |
| File headers on all source files | ✅ Done |

### Remaining Refactoring

All refactoring complete. No files exceed the 600-line limit.

### Not Recommended (Over-engineering)

- External merge sort for duscan: Only needed for >1GB sorted output (rare)
- Async username resolution: Would complicate code for minimal gain
- Frontend state management library: Svelte 5 runes are sufficient
