# Dutopia Architecture

## Pipeline Overview
```
filesystem → duscan → scan.csv → dusum → scan.sum.csv → duapi → REST API → Svelte UI
                               ↑
                            duzip (CSV ↔ .zst)
                            duhuman (human-readable)
```

## Components
- **duscan**: multi-threaded filesystem scanner. Writes CSV or Zstd with sharded output.
- **dusum**: aggregates scan rows into per-folder/user/age buckets.
- **duapi**: in-memory trie index + Axum REST API. Serves static UI.
- **frontend**: SvelteKit SPA that queries `duapi`.

## Data Model
- **Scan CSV**: `INODE,ATIME,MTIME,UID,GID,MODE,SIZE,DISK,PATH`
- **Aggregate CSV**: `path,user,age,files,size,disk,linked,atime,mtime`

Age buckets (default):
- `0`: < 60 days
- `1`: 60–600 days
- `2`: > 600 days

## duapi Index
- Builds a trie of folder paths.
- Stores per-user/age statistics per path.
- Supports filtered reads by `path`, `users`, `age`.

## Concurrency
- `duscan` uses a work queue with worker threads and sharded output files.
- `duapi` is async (Tokio) with blocking filesystem reads for `/files`.

## Platform Notes
- Auth uses PAM on Unix (`PAM_SERVICE`, default `login`).
- Windows uses environment-based fake auth for development.
