# Trash and archive movers

The web UI only queues items. A root cron on a node that mounts the
filesystems moves them to `<fs>/.trash/<date>/<path>` as they are (a rename:
instant at any size, owners, modes and ACLs untouched) and purges old trash. Keep works for 24h (`TRASH_UNDO_HOURS`); moved items stay
30 days (`TRASH_KEEP_DAYS`). Upgrade duapi before the cron.

Once the cron takes an item it leaves the queue; the cron logs what it did.
After the next scan, `<fs>/.trash` and `<fs>/.archive` are browsed in the
dashboard like any folder (archives belong to the item's owner).

Items sent to the archive wait the same 24h, then a second cron
(`archive_mover.py`, see [Archive](#archive)) packs them into
`<fs>/.archive/<path>.<date>.tar.gz` and keeps them.

## Install (on the node)

Needs Python 3.7+, the filesystems mounted at the same paths as
`FILESYSTEMS` without root squash, and for the archive GNU tar and gzip
(pigz if installed).

```bash
sudo install -D -m 0755 script/trash_mover.py /opt/dutopia/trash_mover.py
sudo install -D -m 0755 script/archive_mover.py /opt/dutopia/archive_mover.py
sudo install -D -m 0644 script/trash_fsops.py /opt/dutopia/trash_fsops.py
sudo install -D -m 0600 trash.token /etc/dutopia/trash.token   # the Secret's TRASH_TOKEN
```

## Run

```bash
T="--api https://dutopia.apps.ocp.corp.local --token-file /etc/dutopia/trash.token --fs /red/nas1 --fs /red/nas2"
sudo python3 /opt/dutopia/trash_mover.py $T --dry-run   # look first, changes nothing
sudo python3 /opt/dutopia/trash_mover.py $T             # move, then purge
sudo python3 /opt/dutopia/trash_mover.py move $T        # only move
sudo python3 /opt/dutopia/trash_mover.py purge $T       # only purge
```

`--fs` = the filesystems this node handles; nothing else is touched.

## Schedule

`/etc/cron.d/dutopia-trash`:

```cron
MAILTO=storage-admins@corp.local
0 2 * * * root python3 /opt/dutopia/trash_mover.py --api https://dutopia.apps.ocp.corp.local --token-file /etc/dutopia/trash.token --fs /red/nas1 --fs /red/nas2 >> /var/log/dutopia-trash.log 2>&1 || echo "trash mover failed, see /var/log/dutopia-trash.log"
```

## If something goes wrong

- **A run died halfway:** `... --batch /var/lib/dutopia-trash/<date>.json`
- **What happened to an item:** the log, one line per item (`ok`, `skip` or
  `err`); the claimed batch is in `/var/lib/dutopia-trash/<date>.json`.
- **Restore:** `mv <fs>/.trash/<date>/a/b/run1 <fs>/a/b/` (instant)
- **Free space now:** `... purge --keep-days 7`
- **401:** the token file differs from the pod's `TRASH_TOKEN`.
- **Private CA:** add `--cafile /path/to/ca.pem`.

## Archive

Same flags and token as the trash mover; no purge (archives are kept).

```bash
sudo python3 /opt/dutopia/archive_mover.py $T --dry-run   # look first
sudo python3 /opt/dutopia/archive_mover.py $T             # archive
```

`/etc/cron.d/dutopia-archive`:

```cron
MAILTO=storage-admins@corp.local
30 2 * * * root python3 /opt/dutopia/archive_mover.py --api https://dutopia.apps.ocp.corp.local --token-file /etc/dutopia/trash.token --fs /red/nas1 --fs /red/nas2 >> /var/log/dutopia-archive.log 2>&1 || echo "archive mover failed, see /var/log/dutopia-archive.log"
```

- Files and folders alike become `<fs>/.archive/a/b/run1.2026-11-05.tar.gz`
  (`.1`, `.2`… for a second one the same day). Uses `pigz` when installed,
  else gzip.
- `.archive` is root-only. **Restore:** `cd <fs>/a/b && sudo tar -xpzf <fs>/.archive/a/b/run1.2026-11-05.tar.gz`
- If tar fails (e.g. the filesystem is full) or the archive does not read
  back whole, the item is put back and the log says why. Archives are
  written as `.part` and renamed only once complete.
- **A run died halfway:** `... --batch /var/lib/dutopia-archive/<date>.json`;
  an item left in `<fs>/.archive/a/b/run1.<date>/` was not packed yet.

## Local test

```bash
TRASH_TOKEN=x python3 script/trash_mover.py --dev --api http://localhost:18088
TRASH_TOKEN=x python3 script/archive_mover.py --dev --api http://localhost:18088
```

with duapi started with `FILESYSTEMS=<dirs> TRASH_TOKEN=x TRASH_UNDO_HOURS=0`.
