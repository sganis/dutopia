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

### Priority 1: Rust Performance (High Impact)

**1.1 Cache UID→username lookups in duapi.rs**
- `get_items()` calls `getpwuid()` for every file
- Add `HashMap<u32, String>` cache (same pattern as dusum.rs)
- Impact: 10-100x faster for directories with many files

**1.2 Add username cache to duhuman.rs (if not present)**
- Same pattern - cache getpwuid results

### Priority 2: File Size Refactoring (Code Health)

Files exceeding 600-line hard limit:

| File | Lines | Action |
|------|-------|--------|
| ~~`duscan.rs`~~ | ~~2,179~~ | ✅ Done: split into 5 modules |
| `duapi.rs` | 1,032 | Extract: index.rs, handler.rs |
| `util.rs` | 1,051 | Split: format.rs, path.rs, csv.rs, platform.rs |
| `duzip.rs` | 958 | Split: compress.rs, decompress.rs |
| `dusum.rs` | 798 | Extract: aggregate.rs |
| `+page.svelte` | 1,079 | Extract: Tooltip, FolderBar, FileBar, AgeFilter |

### Priority 3: Coding Convention (Quick Wins)

**3.1 Add file headers** - Every file needs path comment at top:
```rust
// rs/src/util.rs
```
```typescript
// browser/src/ts/util.ts
```

### Not Recommended (Over-engineering)

- External merge sort for duscan: Only needed for >1GB sorted output (rare)
- Async username resolution: Would complicate code for minimal gain
- Frontend state management library: Svelte 5 runes are sufficient
