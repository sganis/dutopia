# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build Commands

```bash
cargo build --release      # Build all binaries
cargo test                 # Run all tests
cargo test test_name       # Run specific test
cargo check                # Check without building
cargo clippy               # Lint check
```

## Binaries

| Binary | Purpose | Key Features |
|--------|---------|--------------|
| `duscan` | Filesystem scanner | Multi-threaded, CSV/zstd output |
| `dusum` | Aggregator | Rollups by folder/user/age |
| `duapi` | REST API | JWT auth, in-memory trie index |
| `duhuman` | Humanizer | Converts epochs/uids to readable |
| `duzip` | Compressor | CSV ↔ Zstandard |
| `dumachine` | Machine processor | Binary data processing |

## Architecture

### Scanner Pipeline
```
duscan → [raw CSV/zstd] → dusum → [aggregated CSV] → duapi → [REST API]
```

### Key Data Structures

- **Row** (`util.rs`): File metadata (dev, ino, mode, uid, gid, size, blocks, atime, mtime)
- **InMemoryFSIndex** (`duapi.rs`): Trie-based path index with per-user/age stats
- **UserStats** (`dusum.rs`): Aggregated file counts, sizes, timestamps per user

### CSV Format
```
INODE,ATIME,MTIME,UID,GID,MODE,SIZE,DISK,PATH
device-inode,epoch,epoch,uid,gid,octal,bytes,bytes,path
```

### Age Buckets
- `0`: Recent (< 60 days)
- `1`: Not too old (60-600 days)
- `2`: Old (> 600 days)

## Performance Patterns

**Good patterns already in use:**
- Thread-local `itoa::Buffer` for number formatting (avoid allocation)
- 32MB buffer batching with 4MB flush threshold
- `Relaxed` atomic ordering for counters (no sync needed)
- Zero-copy path handling via `as_os_str().as_bytes()` on Unix
- Smart CSV quoting (only quote when needed)
- OnceLock for lazy global initialization

**Known issues:**
- `duapi.rs:get_items()` calls `getpwuid()` per file - should cache
- `duscan.rs` sorted mode loads all lines into memory - use external sort for >1GB

## Platform-Specific Code

- Unix: `libc` for getpwuid, statvfs
- Windows: `windows-sys` for GetNamedSecurityInfoW, GetDiskFreeSpaceExW
- macOS auth: `dscl` command
- Linux auth: `su` with stdin password

## Testing

Tests are inline with `#[cfg(test)] mod tests`. Run with:
```bash
cargo test -- --nocapture  # Show println output
cargo test --release       # Test release builds
```

## Module Structure

### duscan (multi-file binary)
```
src/bin/duscan/
├── main.rs   (358 lines) - CLI, main function, progress reporter
├── worker.rs (858 lines) - Worker threads, Task enum, enum_dir
├── csv.rs    (532 lines) - CSV/binary writing utilities
├── merge.rs  (295 lines) - Shard merging
└── row.rs    (160 lines) - Row operations, stat_row
```

## File Size Status

Files requiring refactoring (> 600 line limit):
- `duapi.rs`: 1,032 lines (extract index, handlers)
- `util.rs`: 1,051 lines (split by concern)
- `duzip.rs`: 958 lines
- `dusum.rs`: 798 lines
