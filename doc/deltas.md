# Growth-Delta Detection Across Scans

Proposal for adding scan-over-scan comparison to dutopia, enabling
*"what changed since last week?"* queries — the single highest-value
question HPC admins ask and dutopia cannot currently answer.

## Current state (why we can't do this today)

`dudb` **overwrites** on every run. No history exists.

Evidence in `rs/src/bin/dudb/`:

- `main.rs:50-63` — if the output DB exists, `dudb` errors unless
  `--rebuild` is passed. `--rebuild` calls `remove_db_files()`
  (`main.rs:112-121`) which deletes the `.db`, `-wal`, and `-shm`
  files before rebuilding.
- `schema.rs:33-44` — `stats` PK is `(path_id, user_id, age)`. No
  `scan_id` or timestamp dimension. A second scan replaces the data.
- `schema.rs:74-77` — `metadata` writes use
  `ON CONFLICT(key) DO UPDATE SET value = excluded.value`. Even
  `built_at` and `source_csv` are overwritten.
- `desktop/src-tauri/src/state.rs` hardcodes `scan.csv`, `sum.csv`,
  `scan.db` — single-snapshot workflow.

Any delta feature must first introduce a history model.

## Target questions

| Signal | LLM-friendly meaning |
|--------|---------------------|
| **Fastest growers** | top folders by `Δdisk_bytes` desc |
| **Cold piles** | folders where `files_age2` grew and `files_age0 ≈ 0` — write-once, never read |
| **Runaway users** | per-user `Δdisk_bytes` across whole tree |
| **Abandoned projects** | `files > 0` but `Δatime ≈ 0` over a long window |
| **New consumers** | paths present in B but not in A |
| **Disappeared** | paths present in A but not in B (deleted / moved / archived) |

## Design — Option A (recommended): file-per-snapshot

Every `dudb` run produces a timestamped snapshot into a configurable
history directory; keep the last N (default 7).

### Why Option A

- **Smallest schema churn.** The `stats` PK stays `(path_id, user_id, age)`.
- **Matches existing single-process-per-scan model.** No changes to the
  ingest path except a filename convention.
- **Bounded retention is trivial** — delete old files.
- **Snapshots compress well** — each `.db` is already a rollup, far
  smaller than the raw CSV.

Alternatives considered:

- **Option B — single DB, add `scan_id` dimension to `stats`.** Cleanest
  joins but biggest change; DB size grows ~linearly with scans; massive
  at HPC scale (1B files × N scans).
- **Option C — pre-computed delta table.** Fast reads, but can only
  answer pairs you chose to compare at ingest time.

### Snapshot layout

```
<history-dir>/
  scan-20260411-0200.db
  scan-20260412-0200.db
  scan-20260418-0200.db
  latest.db -> scan-20260418-0200.db   (symlink / config pointer)
```

Filename carries built_at for human scanning; the `metadata` table
inside each DB is the source of truth.

### Cross-snapshot query via `ATTACH DATABASE`

SQLite's `ATTACH DATABASE` lets one connection join across DBs:

```sql
ATTACH DATABASE '/data/history/scan-20260411-0200.db' AS prev;

SELECT cur_p.full_path,
       cur_u.name AS user,
       cur_s.age,
       cur_s.disk_bytes  - IFNULL(prev_s.disk_bytes,  0) AS delta_disk,
       cur_s.file_size   - IFNULL(prev_s.file_size,   0) AS delta_size,
       cur_s.file_count  - IFNULL(prev_s.file_count,  0) AS delta_files
  FROM main.paths  cur_p
  JOIN main.stats  cur_s  ON cur_s.path_id = cur_p.id
  JOIN main.users  cur_u  ON cur_u.id      = cur_s.user_id
  LEFT JOIN prev.paths  prev_p ON prev_p.full_path = cur_p.full_path
  LEFT JOIN prev.users  prev_u ON prev_u.name      = cur_u.name
  LEFT JOIN prev.stats  prev_s ON prev_s.path_id = prev_p.id
                               AND prev_s.user_id = prev_u.id
                               AND prev_s.age     = cur_s.age
 WHERE cur_p.full_path LIKE ? || '%'
 ORDER BY delta_disk DESC
 LIMIT ?;
```

Joining on `full_path` (text) rather than `id` is deliberate — path IDs
are local to each snapshot.

## API surface

New `duapi` endpoints (all read-only):

```
GET /api/scans
    → [{ id, path, built_at, row_count, source_csv }, ...]

GET /api/deltas?from=<id>&to=<id>&path=&users=&age=&limit=
    → [{ path, user, age, delta_disk, delta_size, delta_files, ... }, ...]

GET /api/deltas/summary?from=<id>&to=<id>
    → { top_growers: [...], top_shrinkers: [...], cold_piles: [...],
        runaway_users: [...], new_paths: [...], disappeared_paths: [...] }
```

Response extension for existing folder fetches — add an optional
`?compare=<scan_id>` query param to `GET /api/folders` so the dashboard
can show current values **with delta badges** in one round-trip:

```json
{
  "path": "/proj/genome3",
  "current": { "disk": 8884998144, ... },
  "delta":   { "disk": 4294967296, "files": 18234, "users": {...} }
}
```

## `dudb` changes

- New flag `--history-dir <path>` — write to
  `<history-dir>/scan-<YYYYMMDD-HHMM>.db` in addition to (or instead of)
  the positional output.
- New flag `--retain <N>` — after writing, prune oldest snapshots so at
  most N remain. Default 7.
- No schema change. Metadata already captures `built_at`.

## `duapi` changes

- New flag `--history-dir <path>`. On startup, scan the directory and
  register each `.db` as an available snapshot (lazy-open on query).
- New module `rs/src/bin/duapi/deltas.rs` — handles ATTACH, summary
  aggregations, and pagination.
- Reuse `rs/src/query.rs` path normalization for `path=` filter.

## LLM narration example (the payoff)

Once `dumcp` (see `mcp.md`) exposes `list_deltas` and `deltas_summary`
as tools, an agent can produce:

> *"Between scan-20260411 and scan-20260418, `/proj/genome3` grew
> 4.2 TB. 92% is attributable to user `alice`. All of it landed in
> age-0 (<60d) and none has been re-read — likely a one-shot
> bioinformatics run. Recommendation: ask alice whether this is done,
> then archive to cold tier."*

A human drilling through the current UI takes ~20 minutes to derive
that paragraph from folder-by-folder navigation. The LLM does it in
seconds. This is the whole point of the feature.

## Verification

- Unit: ingest two fabricated CSVs (same tree with a known 1 GB
  growth on `/a/b`), run delta query, assert `delta_disk` matches.
- Integration: snapshot a real scratch area on day 1, induce a
  controlled change (write known file, touch a known file), re-scan,
  and verify `/api/deltas` returns the expected path + magnitudes.
- LLM narration: eyeball a summary response, confirm top-growers
  order is correct.

## Critical files

- Modified: `rs/src/bin/dudb/main.rs` — add `--history-dir`, `--retain`
- Modified: `rs/src/bin/dudb/schema.rs` — no change (confirm PK stays)
- New: `rs/src/bin/duapi/deltas.rs` — ATTACH + delta query module
- Modified: `rs/src/bin/duapi/handler.rs` — new routes `/api/scans`,
  `/api/deltas`, `/api/deltas/summary`, and `?compare=` on `/api/folders`
- Modified: `rs/src/bin/duapi/main.rs` — `--history-dir` flag, register
  snapshots
- Reused: `rs/src/db.rs`, `rs/src/query.rs`

## Open questions

1. **Retention policy:** fixed N, or tiered (daily for 2 weeks, weekly
   for 6 months)? HPC admins at 30PB care about this.
2. **Snapshot trigger:** `dudb` writes to history automatically (opt-in
   via `--history-dir`) or mandatory? Opt-in is safer for v1.
3. **Path identity under renames:** a renamed folder looks like
   delete + new-folder. Fuzzy matching (via inode data from `duscan`)
   is a future enhancement. v1 uses exact-path match.
4. **Snapshot DB storage cost at 1B-file / 30PB scale:** need to measure.
   A rollup DB for that size may be a few GB; keeping 7 is tens of GB.
   Likely acceptable; confirm before committing.
5. **Partial scans:** if a snapshot covered only `/proj` and the next
   covered `/proj + /home`, cross-snapshot diff needs a "scope" concept
   to avoid phantom "new paths" under `/home`.
