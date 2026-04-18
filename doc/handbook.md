# Dutopia Handbook

A Rust toolkit for high-scale filesystem analytics. Turns massive filesystems
(>1B files, tested to 30 PB) into fast, filterable, UTF-8 clean analytics via
a modular pipeline of CLI binaries and a SvelteKit SPA dashboard served by a
REST API.

---

## 1. Pipeline

```
filesystem  ->  duscan  ->  raw CSV/zst  ->  dusum  ->  sum CSV  ->  dudb  ->  SQLite  ->  duapi  ->  REST + SPA
                                                                                               |
                                          duzip (CSV <-> zst)   duhuman (human CSV)    dumachine (reverse)
```

Each stage is a standalone binary. Stages have strict, documented contracts
(CSV schemas, SQLite schema) and can be run independently.

---

## 2. Binaries

All binaries live in `rs/src/bin/`. Build with `cargo build --release`
(Windows: use `cargo.bat` to set up MSVC env).

### 2.1 `duscan` — filesystem scanner

High-throughput, multi-threaded walker. Streams POSIX-like metadata for every
file and directory.

```
duscan [OPTIONS] <folders>...

  -o, --output PATH        output path (default: <folder>.csv or .zst)
  -w, --workers N          parallel workers (default: 2 x CPU, capped at 48)
  -s, --skip SUBSTR        skip paths containing substring
  -b, --bin                write zstd binary instead of CSV
      --no-atime           zero ATIME field (reproducible output)
  -f, --files-hint N       estimated total files (e.g. 750m, 1.2b)
  -q, --quiet              suppress progress
  -v, --verbose            -v errors; -vv errors + paths
```

Output CSV schema (9 fields):

```
INODE,ATIME,MTIME,UID,GID,MODE,SIZE,DISK,PATH
```

| Field | Meaning |
|-------|---------|
| INODE | `device-inode` (Unix); `0-inode` (Windows) |
| ATIME | last access time, epoch seconds |
| MTIME | last modified time, epoch seconds |
| UID   | owner user id |
| GID   | owner group id |
| MODE  | file type + permission bits (octal-readable) |
| SIZE  | logical size in bytes |
| DISK  | on-disk usage (blocks x 512) |
| PATH  | full path, UTF-8 (lossy replacement on non-UTF-8 input) |

Internals: files batched in chunks of 2048; 4 MB flush threshold;
32 MB per-worker `BufWriter`; shards merged into a single output.

### 2.2 `dusum` — folder/user/age rollups

Aggregates raw scan rows by ancestor folder, owning user, and age bucket.
Every ancestor folder of every file gets a row, so parent rollups already
include all descendants — no recursive SUM is needed at query time.

```
dusum <input> [OPTIONS]

  -o, --output PATH        default: <stem>.sum.csv
      --age YOUNG,OLD      age bucket boundaries in days (default: 60,600)
```

Default age buckets:

| Bucket | Condition | Meaning |
|--------|-----------|---------|
| `0`    | mtime within 60 days    | recent |
| `1`    | 60 <= age < 600 days    | not too old |
| `2`    | >= 600 days or unknown  | old |

Output CSV schema (9 fields):

```
path,user,age,files,size,disk,linked,accessed,modified
```

### 2.3 `dudb` — SQLite ingester

Offline, one-shot loader that reads a `dusum` CSV and produces the SQLite
database consumed by `duapi`. Never run by the API server.

```
dudb <input> [OPTIONS]

  -o, --output PATH        default: <stem>.db
      --rebuild            overwrite existing DB
```

SQLite schema (v2):

```sql
CREATE TABLE users (
  id   INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE
);

CREATE TABLE paths (
  id        INTEGER PRIMARY KEY,
  parent_id INTEGER,            -- NULL on synthetic root
  full_path TEXT NOT NULL UNIQUE
);
CREATE INDEX idx_paths_parent ON paths(parent_id);

CREATE TABLE stats (
  path_id     INTEGER NOT NULL,
  user_id     INTEGER NOT NULL,
  age         INTEGER NOT NULL,
  file_count  INTEGER NOT NULL,
  file_size   INTEGER NOT NULL,
  disk_bytes  INTEGER NOT NULL,
  linked_size INTEGER NOT NULL,
  atime       INTEGER NOT NULL,
  mtime       INTEGER NOT NULL,
  PRIMARY KEY (path_id, user_id, age)
) WITHOUT ROWID;

CREATE TABLE metadata (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
```

Design notes:

- `paths.full_path` is stored **byte-for-byte** as `dusum` wrote it — Unix
  form on Linux (`/var/log`), Windows-native on Windows (`C:\Users\San`,
  `\\server\share`). `duapi` normalizes request paths to this same form
  before lookup.
- A synthetic root row (`full_path = ""`, `parent_id = NULL`) sits above
  every platform root, so `parent_id = <synthetic root>.id` lists all
  drives / `/`.
- `stats` is `WITHOUT ROWID` — its PK is its natural clustering.
- `metadata.schema_version = "2"` is verified at `duapi` startup.
- Ingest pragmas are tuned for bulk insert (`synchronous=OFF`, WAL, 256 MB
  cache, `temp_store=MEMORY`). Safe because the DB is rebuildable from the
  CSV.
- Indexes and `ANALYZE` run after bulk load.

### 2.4 `duapi` — REST API + SPA host

Axum server. Opens the SQLite DB read-only through an `r2d2` pool, serves
the Svelte SPA from a static directory, answers the REST API under `/api`.

```
duapi <input.db> [OPTIONS]

  -s, --static-dir DIR     SPA directory (env: STATIC_DIR; default: ./public beside binary)
  -p, --port N             listen port (env: PORT; default: 8080)
      --tls-cert FILE      enable HTTPS with certificate (env: TLS_CERT)
      --tls-key FILE       TLS private key (env: TLS_KEY)
      --cors-origin URL    CORS allowed origin (env: CORS_ORIGIN)
```

Startup:

1. Requires `JWT_SECRET` env var; exits if missing.
2. Opens SQLite pool (size = max(num_cpus, 4)) with `query_only=ON`,
   30 GB mmap hint, 64 MB cache per connection.
3. Validates `metadata.schema_version == "2"`; bails with "rebuild with
   newer dudb" otherwise.
4. Caches user list into `OnceLock<Vec<String>>`.

Middleware stack:

- CORS (`CORS_ORIGIN`, else permissive methods only).
- Timeout (`REQUEST_TIMEOUT_SECS`, default 30).
- Body limit (`MAX_BODY_BYTES`, default 65 536).
- Rate limit (`tower_governor`, `RATE_LIMIT_PER_MIN`, default 300).
- Graceful shutdown on SIGTERM/SIGINT.

All DB work runs inside `tokio::task::spawn_blocking` since `rusqlite` is
synchronous.

### 2.5 `duhuman` — human-readable CSV

Resolves machine fields into display fields. Useful for BI tools.

```
duhuman <input> [-o <file>]     default output: <stem>.res.csv
```

Output header:

```
INODE,ACCESSED,MODIFIED,USER,GROUP,TYPE,PERM,SIZE,DISK,PATH
```

Epochs become local dates, UIDs/GIDs become names, mode bits become
type + octal permissions.

### 2.6 `dumachine` — reverse humanization

Converts a `duhuman` file back into raw CSV form. Output header matches
`duscan`.

```
dumachine <input> [-o <file>]   default output: <stem>.raw.csv
```

### 2.7 `duzip` — CSV <-> zstd

Bidirectional; format detected by extension.

```
duzip <input> [-o <file>]
```

---

## 3. REST API

Base URL: `http(s)://<host>:<port>/api`. All endpoints except `/health` and
`/login` require a JWT bearer token.

### `GET /api/health`

Unauthenticated liveness probe.

```json
{ "status": "ok" }
```

### `POST /api/login`

Authenticates against OS user credentials, returns a 24 h JWT.

Request:

```json
{ "username": "alice", "password": "secret" }
```

Response (`200`):

```json
{ "access_token": "<jwt>", "token_type": "Bearer" }
```

Errors: `400` missing credentials, `401` wrong credentials.

Platform auth:

| OS      | Mechanism |
|---------|-----------|
| macOS   | `dscl . -authonly <user> <pass>` |
| Linux   | `su <user> -c true` with password on stdin |
| Windows | Fake auth against `%USERNAME%` / `FAKE_USER` for development |

Admin: set in the JWT if the username is in `ADMIN_GROUP` (comma-separated,
case-insensitive) *or* if an `ADMIN_PASSWORD` override matched. The admin
override is development/CI only — never set `ADMIN_PASSWORD` in production.

### `GET /api/users`

Admins get the full user list from the DB; non-admins get only their own
username.

### `GET /api/folders`

Children of a folder, grouped by user and age bucket.

Query params:

| Name   | Required | Notes |
|--------|----------|-------|
| path   | yes      | OS-native form. Empty string lists platform roots. |
| users  | no       | Comma-separated. Non-admins must pass exactly their own username. |
| age    | no       | `0`, `1`, or `2`. Omit for all buckets. |

Response: array of

```json
{
  "path": "/var/log",
  "users": {
    "alice": {
      "0": { "count": 12, "size": 1234, "disk": 2048,
             "linked": 0, "atime": 1700000000, "mtime": 1700000100 }
    }
  }
}
```

Result is capped at `MAX_PAGE_SIZE` (default 2000).

### `GET /api/files`

Lists regular files directly inside a folder. Unlike `/folders`, this reads
the **live filesystem** — it does not touch the SQLite DB. Directories,
symlinks, and non-regular entries are skipped. Path `/` is rejected.

Query params: `path` (required, not `/`), `users`, `age` (same semantics
as `/folders`).

Response: array of `{ path, owner, size, accessed, modified }`, capped at
`MAX_PAGE_SIZE`.

Non-admins must pass exactly their own username in `users`.

On Windows, `owner` is best-effort (`%USERNAME%` / `FAKE_USER`).

---

## 4. Path normalization

`duapi` accepts the OS-native form of any request path. The rules
(`query.rs:normalize_path`):

- Empty input -> `""` (synthetic root).
- `/` -> Unix root.
- `C:` or `C:\` -> `C:\` (Windows drive root).
- UNC prefix `\\server\share` preserved.
- Duplicate separators and `.` segments collapsed.
- Literal `..` segments are rejected with `400`.
- NUL byte rejected with `400`.
- Separator follows the input: backslash in, backslash out.

The DB stores paths exactly as `dusum` wrote them, so there is no second
canonicalization pass inside the API.

---

## 5. Frontend

SvelteKit 2 + Svelte 5 SPA with Tailwind CSS 4, built with
`@sveltejs/adapter-static` (no SSR).

```
browser/
  src/
    routes/+page.svelte          main dashboard
    lib/                         components (PascalCase)
      ActionBar, AgeFilter, CopyToast, FileBar, FolderBar,
      Login, PageNav, PathStats, PickerButton, PickerWrapper,
      SortDropdown, Tooltip, TreeMap
    ts/                          typed modules (lowercase)
      api.svelte.ts              JWT + IndexedDB-cached client
      store.svelte.ts            reactive global state
      cache.ts                   IndexedDB wrapper (1 min TTL)
      util.ts                    humanBytes/humanTime/humanCount/colors
      models.ts, transform.ts, tooltip.ts
```

### Dev

```bash
cd browser
npm install
npm run dev          # http://localhost:5173
npm run build        # outputs to build/ (copy to duapi STATIC_DIR)
npm run check        # svelte-check
```

### Desktop app

`desktop/` contains a Tauri 2 + SvelteKit wrapper of the same UI. Rust
backend in `desktop/src-tauri/`.

---

## 6. Configuration

All `duapi` config is flag-or-env; CLI flags win.

| Env var              | Default         | Purpose |
|----------------------|-----------------|---------|
| `JWT_SECRET`         | (required)      | HMAC secret for JWT signing |
| `ADMIN_GROUP`        | (empty)         | Comma-separated usernames with admin rights |
| `ADMIN_PASSWORD`     | (unset)         | Dev/CI admin override — do not set in prod |
| `PORT`               | 8080            | Listen port |
| `STATIC_DIR`         | `./public`      | SPA directory |
| `CORS_ORIGIN`        | (none)          | Explicit CORS origin |
| `TLS_CERT`, `TLS_KEY`| (none)          | Enable HTTPS |
| `REQUEST_TIMEOUT_SECS` | 30            | Per-request timeout |
| `MAX_BODY_BYTES`     | 65536           | Request body size cap |
| `RATE_LIMIT_PER_MIN` | 300             | Governor rate (per IP) |
| `MAX_PAGE_SIZE`      | 2000            | Cap on `/folders` and `/files` results |
| `FAKE_USER`          | `%USERNAME%`    | Windows dev-auth username |
| `PAM_SERVICE`        | `login`         | (reserved, Linux) |

---

## 7. Quickstart

```bash
# 1) Build
cd rs && cargo build --release
cd ../browser && npm install && npm run build

# 2) Scan a filesystem
./rs/target/release/duscan /data -o /tmp/data.csv

# 3) Aggregate by folder/user/age
./rs/target/release/dusum /tmp/data.csv -o /tmp/data.sum.csv

# 4) Build the SQLite index
./rs/target/release/dudb /tmp/data.sum.csv -o /tmp/data.db

# 5) Serve API + UI
JWT_SECRET=<secret> ADMIN_GROUP=root,alice \
  ./rs/target/release/duapi /tmp/data.db \
  --static-dir ./browser/build --port 8000
```

Open `http://localhost:8000`, log in with an OS account.

Optional analyst-friendly CSV:

```bash
./rs/target/release/duhuman /tmp/data.csv -o /tmp/data.human.csv
```

Optional compression:

```bash
./rs/target/release/duzip /tmp/data.csv -o /tmp/data.csv.zst
./rs/target/release/duzip /tmp/data.csv.zst -o /tmp/data.csv
```

---

## 8. Deployment

### Docker

Multi-stage build: compile `rs/` and `browser/` in a build image, copy
`duapi`, `dudb`, and `browser/build/` into a slim runtime image.

- Mount a volume that holds the input CSV and the built `*.db`.
- Run `dudb` in an init job (or build step), not at container start.
- Main process is `duapi /data/data.db --static-dir /app/public`.

### systemd

```ini
[Service]
ExecStart=/opt/dutopia/duapi /opt/dutopia/data.db --port 8000
Environment=JWT_SECRET=...
Environment=ADMIN_GROUP=root,alice
Environment=STATIC_DIR=/opt/dutopia/public
User=dutopia
```

### Reverse proxy

Terminate TLS at nginx/caddy and forward to `duapi` on localhost. If the
UI is hosted on a separate origin, set `CORS_ORIGIN` explicitly.

### Hardening

- Run as a non-root service account.
- Strong `JWT_SECRET`; rotate periodically (invalidates all tokens).
- Never set `ADMIN_PASSWORD` in production.
- Enable TLS or terminate at a trusted proxy.
- Keep the input DB on a mount the service can only read.

---

## 9. Repository layout

```
dutopia/
  rs/                   Rust workspace (binaries + shared lib)
    src/
      lib.rs            re-exports util, auth, storage
      auth.rs           JWT + per-OS credential verification
      storage.rs        statvfs / Win32 disk info
      util/             Row, CSV helpers, path utils, platform fns, logging
      bin/
        duscan/         scanner (main, worker, csv, merge, row)
        dusum/          aggregator (main, stats, aggregate, output)
        dudb/           SQLite ingester (main, schema, ingest)
        duapi/          API server (main, handler, db, item, query, shutdown)
        duzip/          CSV <-> zst (main, record, compress, decompress)
        duhuman.rs      single-file humanizer
        dumachine.rs    single-file reverse humanizer
    Cargo.toml
  browser/              SvelteKit SPA (static build)
  desktop/              Tauri 2 + SvelteKit desktop wrapper
  doc/                  this handbook
```

CI (GitHub Actions, Ubuntu): `cargo test -r` + `npm run build` on push to
`master`. Patch version auto-bumped via `cargo-edit` on release.

---

## 10. Scale notes

- DB size scales with **folder count x users x 3 ages**, not file count.
  A realistic upper estimate (50 M folders x 3 users x 3 ages) keeps
  `stats` in the tens of GB.
- Hot query is one PK lookup on `paths.full_path` plus one clustered range
  scan on `stats` per child folder, capped by `MAX_PAGE_SIZE`.
  Sub-millisecond on warm cache.
- RSS is dominated by mmap / OS page cache, not Rust heap.
- `dudb`'s bottleneck at load time is the in-process
  `full_path -> path_id` cache; for extreme datasets, swap it for a
  DB-backed lookup (slower build, bounded memory).
