# Trash plan (browser + OpenShift + cron)

> Saved in the repo as `doc/trash-plan.md`.

## Context

Today the web cart lives only in the browser (`localStorage`, `browser/src/ts/cleanupQueue.svelte.ts`).
No external process can see it. Goal: the cart moves to the server, **Keep** is the 1-day undo, and
a nightly cron moves the items to `.trash`, packs folders into one `.tar.zst`, and purges after 30 days.
Users can't restore after the undo window.

## Flow

```
UI trash icon ─► duapi ─► /data/trash/queue.json          (the cart, on the PVC)
UI Keep       ─► duapi ─► removes the entry
cron 02:00    ─► POST /api/trash/claim (token) ─► items older than 24h, removed from the queue
              ─► mv path to <mount>/.trash/<date>/…, tar+zstd folders, purge dates > 30 days
```

## 1. duapi — new `src/bin/duapi/trash.rs`

- `queue.json` in `/data/trash/` (`TRASH_DIR` env). Entry: `{path, size, owner, actor, queued_at}`.
  One mutex; write tmp + rename.
- `GET /api/trash` → my items (admin: all). `POST /api/trash` → add. `DELETE /api/trash?path=` → Keep.
  Only the owner or an admin. Reuse `validate()` from `cleanup.rs`.
- `POST /api/trash/claim` → needs `Authorization: Bearer $TRASH_TOKEN`; returns items with
  `queued_at` older than 24h and removes them from the queue.
- Register the routes in `main.rs` next to `/cleanup/*`.

## 2. Browser

- `cleanupQueue.svelte.ts`: drop localStorage; load/add/keep through the 3 endpoints
  (`api.web.svelte.ts`). The rest of the store API stays the same.
- `CleanupPanel.svelte`: each row shows "trash after <date>"; Keep calls the server. The Script and
  Notify buttons stay but no longer empty the cart.

## 3. Cron — `script/trash_mover.py` (Python 3 stdlib, runs as root on a node that sees the FS)

1. `POST /api/trash/claim` with the token; save the JSON to `/var/lib/dutopia-trash/<date>.json`.
2. For each item: skip it if it's gone, inside `.trash`, or no longer owned by `owner`.
3. Filesystems are passed in explicitly, the same list the duscan crons use:
   `--fs /red/nas1 --fs /red/nas2`. An item's filesystem = the longest listed prefix of its path;
   its trash = `<fs>/.trash` (`0700 root`), next to `<fs>/.stats`. Items outside the list are skipped.
   Destination keeps the path: `/red/nas1/a/b/run1` → `/red/nas1/.trash/2026-11-04/a/b/run1`.
4. `os.rename` (instant), then folder → `run1.tar.zst` (`tar -I 'zstd -T0'`) and `rm -rf`.
5. Delete `.trash/<date>` folders older than 30 days.

Crontab: `0 2 * * * python3 /opt/dutopia/trash_mover.py --fs /red/nas1 --fs /red/nas2 --api https://dutopia.apps.ocp.corp.local --token-file /etc/dutopia/trash.token`

## 4. OpenShift

- `deploy/openshift/secret.example.sh`: add `TRASH_TOKEN`.
- `/data` is already a writable PVC; nothing else changes. Short cron section in `deploy/README.md`.

## Verification

- `cargo test` (add/keep/claim, 24h rule, owner vs admin, wrong token → 401) and `npm run check && npm run build`.
- Local: `TRASH_TOKEN=x ./duapi du.db` → trash a folder → `queue.json` gets the entry → Keep removes it →
  run the script against a test folder → it ends up as `.trash/<date>/…/name.tar.zst`.
