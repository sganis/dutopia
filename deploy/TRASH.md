# Trash and archive

The web UI only queues items. Keep undoes it for 24h (`TRASH_UNDO_HOURS`).
Then:

| What | When | Does |
|---|---|---|
| `mover.py` | root cron, nightly | moves each item as is (a rename: instant at any size; owners, modes and ACLs untouched): trash to `<fs>/.trash/<date>/<path>`, archive to `<fs>/.archive/<path>.<date>/<name>` |
| `archiver.py` | by hand | packs what the mover put in `.archive` into `<fs>/.archive/<path>.<date>.tar.gz` |
| purge | by hand | deletes old trash (`find`, below) |

The mover skips what is gone, is not its owner's, is a folder holding other
owners' files, or is reached through a symlink; the log says why. Once the
mover takes an item it leaves the queue. After the next scan `.trash` and
`.archive` are browsed in the dashboard like any folder. Upgrade duapi
before the cron.

## Install (on the node)

Needs Python 3.7+, the filesystems mounted at the same paths as
`FILESYSTEMS` without root squash, and for the archiver GNU tar and gzip
(pigz if installed). Files root-owned, not writable by anyone else:

```bash
sudo install -D -m 0755 script/mover.py      /opt/dutopia/mover.py
sudo install -D -m 0755 script/archiver.py   /opt/dutopia/archiver.py
sudo install -D -m 0644 script/trash_fsops.py /opt/dutopia/trash_fsops.py
sudo install -D -m 0600 trash.token          /etc/dutopia/trash.token   # the Secret's TRASH_TOKEN
```

## Mover (cron)

```bash
T="--api https://dutopia.apps.ocp.corp.local --token-file /etc/dutopia/trash.token --fs /red/nas1 --fs /red/nas2"
sudo python3 /opt/dutopia/mover.py $T --dry-run   # look first, changes nothing
sudo python3 /opt/dutopia/mover.py $T             # move
```

`--fs` = the filesystems this node handles; nothing else is touched.

`/etc/cron.d/dutopia-mover`:

```cron
MAILTO=storage-admins@corp.local
0 2 * * * root python3 /opt/dutopia/mover.py --api https://dutopia.apps.ocp.corp.local --token-file /etc/dutopia/trash.token --fs /red/nas1 --fs /red/nas2 >> /var/log/dutopia-mover.log 2>&1 || echo "mover failed, see /var/log/dutopia-mover.log"
```

## Archiver (by hand)

```bash
sudo python3 /opt/dutopia/archiver.py --fs /red/nas1 --fs /red/nas2 --dry-run   # list
sudo python3 /opt/dutopia/archiver.py --fs /red/nas1 --fs /red/nas2             # pack
```

No API, no token. Each archive is written as `.part`, read back whole, given
to the item's owner, and only then is the staged copy removed. If tar fails
(e.g. the filesystem is full) the item stays staged for the next run.

## Purge (by hand)

The trash of days older than 30 (`TRASH_KEEP_DAYS`, which the UI quotes):

```bash
sudo find /red/nas1/.trash -mindepth 1 -maxdepth 1 -type d -mtime +30 -exec rm -rf {} +
```

It selects the day folders by when they were made. Never `find -mtime` on
the files themselves: moving keeps their own dates, so it would delete what
was trashed yesterday.

## Restore

- **Trash:** `mv <fs>/.trash/<date>/a/b/run1 <fs>/a/b/`
- **Archive, not packed yet:** `mv <fs>/.archive/a/b/run1.<date>/run1 <fs>/a/b/`
- **Archive, packed:** `cd <fs>/a/b && sudo tar -xpzf <fs>/.archive/a/b/run1.<date>.tar.gz`
  (owners and modes are kept)

## If something goes wrong

- **What happened to an item:** the log, one line per item (`ok`, `skip` or
  `err`); the claimed batch is in `/var/lib/dutopia-mover/<date>.json`.
- **A run died halfway:** `... mover.py $T --batch /var/lib/dutopia-mover/<date>.json`
  (what was already moved is skipped as gone).
- **401:** the token file differs from the pod's `TRASH_TOKEN`.
- **Private CA:** add `--cafile /path/to/ca.pem`.

## Limited sudo

For an account that may only run listed commands, ask for exactly these
(`visudo -f /etc/sudoers.d/dutopia`; one `find` line per filesystem):

```
Cmnd_Alias DUTOPIA = \
  /usr/bin/python3 /opt/dutopia/mover.py --api https://dutopia.apps.ocp.corp.local --token-file /etc/dutopia/trash.token --fs /red/nas1 --fs /red/nas2 --dry-run, \
  /usr/bin/python3 /opt/dutopia/mover.py --api https://dutopia.apps.ocp.corp.local --token-file /etc/dutopia/trash.token --fs /red/nas1 --fs /red/nas2, \
  /usr/bin/python3 /opt/dutopia/archiver.py --fs /red/nas1 --fs /red/nas2 --dry-run, \
  /usr/bin/python3 /opt/dutopia/archiver.py --fs /red/nas1 --fs /red/nas2, \
  /usr/bin/find /red/nas1/.trash -mindepth 1 -maxdepth 1 -type d -mtime +30 -exec rm -rf {} +, \
  /usr/bin/find /red/nas2/.trash -mindepth 1 -maxdepth 1 -type d -mtime +30 -exec rm -rf {} +
<user> ALL=(root) NOPASSWD: DUTOPIA
```

Run them with the same arguments in the same order. No `*` in these lines:
in sudoers it also matches spaces, so it would let extra flags in (`--dev`
turns off the owner checks). Restore stays with whoever has root.

## Local test

```bash
TRASH_TOKEN=x python3 script/mover.py --dev --api http://localhost:18088
python3 script/archiver.py --dev --fs <dir>
```

with duapi started with `FILESYSTEMS=<dirs> TRASH_TOKEN=x TRASH_UNDO_HOURS=0`.
