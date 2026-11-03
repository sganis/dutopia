# Trash mover

The web UI only queues items. A root cron on a node that mounts the
filesystems moves them to `<fs>/.trash/<date>/<path>` (folders as `.tar.zst`)
and purges old trash. Keep works for 24h (`TRASH_UNDO_HOURS`); moved items stay
30 days (`TRASH_KEEP_DAYS`). Upgrade duapi before the cron.

## Install (on the node)

Needs Python 3.7+, GNU tar, zstd, and the filesystems mounted at the same
paths as `FILESYSTEMS`, without root squash.

```bash
sudo install -D -m 0755 script/trash_mover.py /opt/dutopia/trash_mover.py
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
- **Restore:** History shows where it went;
  `tar --use-compress-program 'zstd -d' -xpf <fs>/.trash/<date>/a/b/run1.tar.zst -C <fs>/a/b`
- **Free space now:** `... purge --keep-days 7`
- **401:** the token file differs from the pod's `TRASH_TOKEN`.
- **Private CA:** add `--cafile /path/to/ca.pem`.

## Local test

```bash
TRASH_TOKEN=x python3 script/trash_mover.py --dev --api http://localhost:18088
```

with duapi started with `FILESYSTEMS=<dirs> TRASH_TOKEN=x TRASH_UNDO_HOURS=0`.
