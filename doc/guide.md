# Dutopia User Guide

## Overview
Dutopia scans large filesystems, aggregates ownership/age statistics, and serves results through a REST API with a Svelte UI. The typical flow is:

`duscan` → `dusum` → `duapi` → browser UI

## Build
```bash
cd rs
cargo build --release
```

## Quick Start
```bash
# 1) Scan
./target/release/duscan /data -o scan.csv

# 2) Aggregate
./target/release/dusum scan.csv -o scan.sum.csv

# 3) Serve API + UI
JWT_SECRET=... ./target/release/duapi scan.sum.csv --port 8000
```

## CLI Summary
- `duscan` — multi-threaded filesystem scan to CSV or Zstd (`--bin`).
- `dusum` — aggregates scan CSV into per-folder/user/age buckets.
- `duapi` — REST API server + static UI hosting.
- `duzip` — CSV ↔ Zstandard conversion.
- `duhuman` — human-readable CSV output.
- `dumachine` — reverse of `duhuman`.

## Authentication
- Set `JWT_SECRET` before running `duapi`.
- Optional `ADMIN_GROUP` (comma-separated) controls admin access.
- On Unix systems, authentication uses PAM (`PAM_SERVICE`, default `login`).

## TLS
To enable HTTPS:
```bash
TLS_CERT=/path/cert.pem TLS_KEY=/path/key.pem ./duapi scan.sum.csv
```

## Output Formats
`duscan` CSV header:
```
INODE,ATIME,MTIME,UID,GID,MODE,SIZE,DISK,PATH
```

`dusum` output includes per-folder/user/age aggregates.

## Troubleshooting
- If `duapi` refuses to start, verify `JWT_SECRET` is set.
- If login fails on Unix, confirm PAM is configured and `PAM_SERVICE` is valid.
- UI not loading? Ensure `STATIC_DIR` points to the built UI output.
