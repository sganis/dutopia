# Trash plan (browser + OpenShift + cron)

## Context

Today the web cart lives only in the browser (`localStorage`, `browser/src/ts/cleanupQueue.svelte.ts`).
No external process can see it. Goal: the cart moves to the server, **Keep** is the 1-day undo, and
a nightly cron moves the items to `.trash` and purges after 30 days.
Users can't restore after the undo window.

Dutopia has no notion of a filesystem today: the DB is a path tree from `/`, and `/red/nas1` is just
a folder. This plan adds one, as an explicit list, because both the trash and the home page need it.

## Flow

```
UI trash icon ─► duapi ─► /data/trash/queue.json          (the cart, on the PVC)
UI Keep       ─► duapi ─► removes the entry
cron 02:00    ─► POST /api/trash/claim (token) ─► items older than 24h, removed from the queue
              ─► mv path to <fs>/.trash/<date>/…, purge dates > 30 days
```

## 0. Filesystems — `FILESYSTEMS` env in duapi

- `FILESYSTEMS=/red/nas1,/red/nas2,…` in the ConfigMap: the mount points that duscan scans
  (each already has `<fs>/.stats`). Parsed once at startup; an item's filesystem = the longest
  listed prefix of its path (same `/`-boundary rule as `authz::path_in_roots`).
- **Home page shows the filesystems directly.** When `FILESYSTEMS` is set, `GET /api/folders?path=`
  (the SPA's synthetic root `""`) returns one row per filesystem, with the same stats a normal
  folder row has, instead of `/`. No more clicking `/` → `/red` → `/red/nas1`.
  - Needs a `db::folder_stats(pool, paths, users, age)` next to `list_children` in `src/db.rs`
    (same row shape).
  - Same access rules as today: regular users see their own usage on each filesystem; global
    admins see everything; storage admins see the filesystems that hold their roots (their
    roots usually *are* filesystems, e.g. `STORAGE_ADMINS=user1:/red/nas1`).
  - SPA: "up" from a filesystem goes to home (`""`), not `/red`. Storage admins keep landing on
    their first root, as they do now.
  - Without `FILESYSTEMS`, nothing changes.
- `/api/health` adds `filesystems: [...]` so the SPA knows the list.

## 1. duapi — new `src/bin/duapi/trash.rs`

- `queue.json` in `/data/trash/` (`TRASH_DIR` env). Entry: `{path, size, owner, actor, fs, queued_at}`.
  One mutex; write tmp + rename.
- `GET /api/trash` → visible items. `POST /api/trash` → add. `DELETE /api/trash?path=` → Keep.
  Reuse `validate()` from `cleanup.rs`, plus: the path must be inside a `FILESYSTEMS` entry, must
  not be the filesystem root itself, and must not be inside `<fs>/.trash` or `<fs>/.stats`.
  duapi fills `fs`.
- Who, using the tiers in `src/authz.rs` (`Claims.roots`):
  - global admin (`claims.is_admin`): any item;
  - storage admin: items with `authz::path_in_roots(&claims.roots, path)`, plus their own;
  - regular user: only items whose `owner == claims.sub`.
  Same rule for add, Keep and what `GET` lists. `actor` = `claims.sub` (with `ADMIN_PASSWORD`
  logins that is just the typed username).
- `POST /api/trash/claim` → needs `Authorization: Bearer $TRASH_TOKEN`; returns items with
  `queued_at` older than 24h and removes them from the queue.
- Register the routes in `main.rs` next to `/cleanup/*`.

## 2. Browser

- `cleanupQueue.svelte.ts`: drop localStorage; load/add/keep through the 3 endpoints
  (`api.web.svelte.ts`). The rest of the store API stays the same. Call them with
  `use_cache = false`: `fetchCached` (cache.ts) would serve a stale cart.
- `CleanupPanel.svelte`: each row shows "trash after <date>"; Keep calls the server. The Script and
  Notify buttons stay but no longer empty the cart.
- The trash icon follows the same tiers: shown for own single-owner folders, and for any
  single-owner folder when `State.isAdmin` or the path is inside `State.roots`. Hidden on the
  filesystem rows themselves.

## 3. Cron — `script/trash_mover.py` (Python 3 stdlib, runs as root on a node that sees the FS)

1. `POST /api/trash/claim` with the token; save the JSON to `/var/lib/dutopia-trash/<date>.json`.
2. For each item: skip it if it's gone, inside `.trash`, no longer owned by `owner`, or its `fs`
   is not a mount point on this node (`os.path.ismount`).
3. Trash = `<fs>/.trash` (`0700 root`), next to `<fs>/.stats`. `fs` comes in the batch, so the
   cron needs no filesystem list of its own.
   Destination keeps the path: `/red/nas1/a/b/run1` → `/red/nas1/.trash/2026-11-04/a/b/run1`.
4. `os.rename` (instant, same filesystem); folders stay folders (see below).
5. Delete `<fs>/.trash/<date>` folders older than 30 days, for every `fs` it has seen (or
   `--purge /red/nas1 --purge /red/nas2`).

Crontab: `0 2 * * * python3 /opt/dutopia/trash_mover.py --api https://dutopia.apps.ocp.corp.local --token-file /etc/dutopia/trash.token`

## 4. OpenShift

- `deploy/openshift/config.example.yaml`: add `FILESYSTEMS`.
- `deploy/openshift/secret.example.sh`: add `TRASH_TOKEN`.
- `/data` is already a writable PVC; nothing else changes. Short cron section in `deploy/README.md`.

## Verification

- `cargo test` (FILESYSTEMS parsing and longest prefix; home rows per tier; add/keep/claim, 24h rule,
  owner vs storage admin vs global admin, fs root and `.trash`/`.stats` rejected, wrong token → 401)
  and `npm test && npm run build`.
- Local: `FILESYSTEMS=/tmp/fs1,/tmp/fs2 TRASH_TOKEN=x ./duapi du.db` → home shows fs1 and fs2 →
  trash a folder → `queue.json` gets the entry with `fs` → Keep removes it → run the script →
  it ends up as `/tmp/fs1/.trash/<date>/…/name`.

## Implemented beyond the plan

Found while building and reviewing it (see `deploy/README.md` §9):

- **Safety of the root cron** (`script/trash_fsops.py`): every check, rename and
  delete runs on O_NOFOLLOW file descriptors from the filesystem root, folders
  are walked so one holding another owner's files or another mount is skipped,
  a planted `<fs>/.trash` symlink is refused, owners below uid 1000 are never
  moved, `--fs` allowlist, https only.
- **Single owner per the DB**: a folder is queued only when the DB shows one
  owner inside; `UNK` (UID with no account) is refused; backslashes refused.
- **Nested items**: a path inside a queued folder is a 409; a folder absorbs
  queued children of the same owner and filesystem.
- **Per-node claim**: the cron reads FILESYSTEMS from `/api/health` and claims
  only the filesystems it mounts (`?fs=`), so several cron nodes coexist.
- **Folders are not packed**: packing into `.tar.zst` read and rewrote all
  the data (hours for a 100 TB folder, and the space twice while it ran)
  only to delete it 30 days later, and lost ACLs. A rename is instant at any
  size, keeps everything, leaves the folder browsable in the dashboard, and
  restore is a rename back. The archive still packs (`.tar.gz`): keeping it
  compressed is its purpose.
- **No history**: a claimed item leaves `queue.json`, the cron logs each
  result on its node, and the next scan shows it under `<fs>/.trash` (or
  `.archive`). An earlier `history.json` was dropped: the queue file only
  ever holds what is waiting. Due items of filesystems no longer listed are
  dropped at the next claim.
- **Operations**: `trash_mover.py [move|purge]` (default both), `--dry-run`
  (server confirms it applied it, `X-Trash-Dry-Run`), `TRASH_KEEP_DAYS`,
  single-instance lock, `--batch` reruns, `TRASH_UNDO_HOURS`, `--dev` for local
  testing, restore instructions.
- **UI**: rows show "in trash · keep" and toggle Keep; the Notify email gives
  each queued item's trash date and how to Keep it.

