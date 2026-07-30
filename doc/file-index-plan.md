# Plan: File-Level Index in SQLite (replace live `ls` in /api/files)

**Status:** implemented (2026-07-30) — schema v3, `dudb --raw`, `db::list_files`
**Date:** 2026-07-30
**Problem:** `/api/files` lists files with a live `fs::read_dir` (`src/item.rs::get_items`).
In production the API host has no access to the scanned filesystem, so file listing
is broken there. Folder stats already come from the dudb SQLite DB; files should too.

## Requirement

- Ingest the raw duscan CSV (`INODE,ATIME,MTIME,UID,GID,MODE,SIZE,DISK,PATH`,
  ~50 GB, ~1B rows in production) into SQLite so `/api/files` becomes a DB query.
- Keep the existing API JSON shape (`FsItemOut`) — no frontend changes.
- Estimated DB size with folders + files: ~100 GB (acceptable).

## Technology decision: stay on SQLite

The workload is a point lookup — "files in folder X, filtered by user/age,
capped at 2000" — a single indexed B-tree probe, ~1 ms even at 1B+ rows.
Read-only snapshot, rebuilt in batch, single writer; the existing `open_pool`
(read-only, mmap, r2d2) scales to a 100 GB file via the OS page cache.
Zero new dependencies, fits the single-binary/no-server ethos.

Alternatives considered:

| Tech | Take |
|---|---|
| DuckDB | Only serious contender. Parallel CSV ingest in minutes, compresses to ~15–25 GB. But point lookups under concurrent API load are weaker than an indexed B-tree, and it adds a second heavy engine. Only worth it if whole-tree analytics (top-N largest files globally, per-extension rollups) become a feature. |
| ClickHouse / Postgres | Server processes to operate; no benefit for a read-only snapshot. |
| Parquet, RocksDB/LMDB | Lose SQL or add custom serialization for no gain. |

**Bonus consistency win:** the age-bucket drift noted in `src/item.rs:66`
disappears — files and folders will come from the same snapshot, so file
listings finally match the aggregated bucket stats.

## Size & time estimates

- Row cost with filename-only storage (folder is an FK into `paths`):
  ~40–70 bytes in a clustered B-tree → **~60–90 GB total** for a 50 GB raw CSV
  (may come in under the CSV since full paths are deduplicated).
- Ingest at 300–600k rows/s single-threaded → **30–90 min for ~1B rows**.
- Operational cost to plan for: shipping a ~100 GB DB file to the API host each
  rebuild cycle (rsync or build in place).

## Design

### Schema (dudb schema v3, same DB file)

```sql
CREATE TABLE files (
    folder_id INTEGER NOT NULL,   -- FK → paths.id (the parent folder)
    name      TEXT    NOT NULL,   -- filename only, not full path
    user_id   INTEGER NOT NULL,   -- FK → users.id
    age       INTEGER NOT NULL,   -- bucket 0/1/2, computed at ingest
    size      INTEGER NOT NULL,
    atime     INTEGER NOT NULL,
    mtime     INTEGER NOT NULL,
    PRIMARY KEY (folder_id, name)
) WITHOUT ROWID;
```

- **`WITHOUT ROWID`, PK `(folder_id, name)`** clusters a folder's files into
  contiguous pages — one listing touches a handful of pages, no secondary index
  needed. duscan emits files grouped by directory, so inserts arrive mostly in
  PK order (fast appends).
- **Regular files only** (filter `MODE & S_IFMT == S_IFREG`), matching
  `get_items` semantics — directories already live in `paths`.
- **`age` stored at ingest** with the same `AgeCfg` defaults (60/600); dusum and
  dudb run minutes apart in the pipeline, so drift is negligible. Expose
  `--age young,old` like dusum.
- **uid→username** resolved like `dusum::aggregate::resolve_user` (getpwuid on
  the ingest host), sharing the existing `users` table. Constraint: dudb must
  run on a host in the same identity domain as dusum (in practice the same
  host, same pipeline run).
- Parent folder string derived with `dutopia::util::dusum_parent` so it matches
  `paths.full_path` byte-for-byte; lookup misses (mismatched CSV runs) are
  counted and warned about.

### CLI

`dudb sum.csv --raw scan.csv -o out.db` — one invocation builds both levels.
Without `--raw`, today's folder-only DB is produced and `/api/files` falls back
to the live `read_dir` (keeps dev mode working).

### API

New `db::list_files(pool, folder, users, age) -> Vec<FsItemOut>` with
`ORDER BY name LIMIT cap` **pushed into SQL** — a folder with millions of files
must not be materialized in RAM before truncation (current code truncates after
collecting). `get_files_handler` and `mcp.rs::tool_list_files` use it when the
DB has a `files` table; JSON shape unchanged, frontend untouched.

## Implementation steps

1. **Schema v3** (`src/bin/dudb/schema.rs`): add `files` table, bump
   `SCHEMA_VERSION` / `db::SUPPORTED_SCHEMA_VERSION` to `3`, set
   `page_size=8192` before table creation, record `has_files`, age cutoffs, and
   raw-CSV provenance in `metadata`.
2. **Raw ingest** (`src/bin/dudb/ingest_raw.rs`, new): stream raw CSV with
   `ByteRecord` (no per-row String allocs), reuse the folder `path_cache` from
   the dusum pass for folder_id lookups, batch commits every ~1M rows with
   progress output, `INSERT OR IGNORE` for defensive dedup, warn-and-count on
   unmatched folders.
3. **CLI** (`src/bin/dudb/main.rs`): `--raw` flag, wire the second pass, report
   file-row counts.
4. **Query layer** (`src/db.rs`): `list_files()` with user/age filters and
   SQL-side `LIMIT`, plus a `has_files(pool)` probe.
5. **Handlers** (`src/bin/duapi/handler.rs`, `mcp.rs`): use `list_files` when
   available, else fall back to `get_items`; keep the `path == "/"` guard.
6. **Tests**: extend `db::test_support::populate` with a `files` fixture;
   ingest tests for Linux/Windows/UNC paths, mode filtering, unmatched-folder
   handling; handler tests for filters and the cap.
7. **Benchmark** (de-risk before the 1B-row build): synthetic 100M-row CSV,
   measure ingest rate and per-folder query latency at depth; verify
   `WITHOUT ROWID` vs. plain-table+index.
8. **Docs**: update CLAUDE.md/README pipeline diagram
   (`duscan → dusum → dudb → duapi`) and note the same-host requirement for uid
   resolution.

## Open decisions (defaults chosen, overridable)

- **`DISK` excluded** from `files` — `FsItemOut` doesn't serve it. Including it
  costs ~5 GB and enables per-file disk-usage views later.
- **One DB file** (not a separate files DB) — simpler deploy. Downside:
  folder-only rebuilds rewrite everything. If independent rebuild cadence is
  ever needed, a second file + `ATTACH` is the escape hatch.
