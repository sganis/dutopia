# Trash mover: install, run, operate

The web UI's trash icon only *queues* items: duapi runs in a pod that cannot
see the scanned filesystems. A nightly cron on a node that mounts them does
the work:

```
UI trash icon ─► duapi ─► /data/trash/queue.json            (Keep = undo)
cron 02:00    ─► POST /api/trash/claim   items queued ≥ 24h on the filesystems this node mounts
              ─► mv to <fs>/.trash/<date>/<path>, folders packed as .tar.zst
              ─► POST /api/trash/report  moved / skipped / error  → History tab
              ─► delete <fs>/.trash/<date> older than 30 days
```

Because the cron runs once a day, an item waits between 24 and 48 hours before
it moves (queued Monday 03:00 → due Tuesday 03:00 → moved Wednesday 02:00).

Deploy duapi with `FILESYSTEMS` and `TRASH_TOKEN` first ([README §6](README.md#6-deploy)).
**Upgrade duapi before the cron**, always: a newer cron stops if the server does
not confirm `--dry-run` and the per-node filter.

---

## 1. Prepare the node (once)

A Linux node that mounts every filesystem you want it to handle, at the same
paths as in `FILESYSTEMS` (and as duscan scanned them).

```bash
python3 --version          # 3.7 or newer
tar --version | head -1    # GNU tar
zstd --version             # sudo dnf -y install zstd
```

- **Root access to the data.** The export must not squash root (`no_root_squash`),
  or the renames and the `chown` of `<fs>/.trash` fail with *Operation not permitted*.
- **Reach the Route.** `curl -s https://dutopia.apps.ocp.corp.local/api/health`
  must answer from the node. With a private CA, note the bundle path for `--cafile`.
- **Free space.** Packing a folder needs room for the archive next to it until
  the folder is removed. On a nearly full filesystem `tar` fails and the folder
  stays in `.trash` uncompressed (logged as an error), still purged after 30 days.

## 2. Install

From a checkout of the repo (both files, in the same folder):

```bash
sudo install -D -m 0755 script/trash_mover.py /opt/dutopia/trash_mover.py
sudo install -D -m 0644 script/trash_fsops.py /opt/dutopia/trash_fsops.py
sudo install -D -m 0600 trash.token /etc/dutopia/trash.token   # same value as the Secret's TRASH_TOKEN
```

`trash.token` is the file `deploy/openshift/secret.example.sh` generated (git
ignores it). Keep a copy in your password manager: it exists only in the
Secret and on the cron nodes.

## 3. Check it (dry run)

As root. It reads the server's FILESYSTEMS, checks every due item exactly as a
real run would, lists what it would move and purge, and changes nothing: the
items stay queued and nothing is reported.

```bash
sudo python3 /opt/dutopia/trash_mover.py \
  --api https://dutopia.apps.ocp.corp.local \
  --token-file /etc/dutopia/trash.token \
  --fs /red/nas1 --fs /red/nas2 \
  --dry-run
```

```
[warn] …                                   only against an older duapi (see §8)
[claim] 3 item(s) on /red/nas1, /red/nas2 (dry run: left queued)
[would move] /red/nas1/projA/run1
[skip] /red/nas1/projB: contains files not owned by alice
[would purge] /red/nas1/.trash/2026-09-30
```

`--fs` (one per filesystem this node should handle) is the allowlist of what
root may touch, even if the API were compromised. Keep it in the crontab.

## 4. Schedule

`/etc/cron.d/dutopia-trash` (root, 02:00, output to a log; set `MAILTO` to get
failures by mail):

```cron
MAILTO=storage-admins@corp.local
0 2 * * * root python3 /opt/dutopia/trash_mover.py --api https://dutopia.apps.ocp.corp.local --token-file /etc/dutopia/trash.token --fs /red/nas1 --fs /red/nas2 >> /var/log/dutopia-trash.log 2>&1 || echo "dutopia trash mover failed, see /var/log/dutopia-trash.log"
```

Exit status is 0 when everything went fine and 1 when anything failed (an
item, the claim, the report, the lock, …); the `|| echo` line is what cron mails.

**Several nodes.** Install the cron on each node with its own `--fs` list. Each
claims only the filesystems it mounts and leaves the rest queued for the
others. One run at a time per node (`/var/lib/dutopia-trash/.lock`).

## 5. Read a run

```
[claim] 2 item(s) on /red/nas1          claimed: now "being moved" in History
[saved] /var/lib/dutopia-trash/2026-11-05.json
[ok] /red/nas1/projA/run1 -> /red/nas1/.trash/2026-11-05/projA/run1.tar.zst
[skip] /red/nas1/projB: no longer owned by owner alice
[purged] /red/nas1/.trash/2026-10-04
[report] 2 result(s) sent               History now says moved / skipped
```

| Tag | Meaning |
|-----|---------|
| `ok` | moved; folders are packed, files kept as they are |
| `skip` | left in place, reason given (see below); not an error |
| `err` | failed (the run exits 1); reason given |
| `warn` | something to fix, the run continues |
| `would move`, `would purge` | dry run only |

Skip reasons: `gone`; `no longer owned by owner X`; `contains files not owned
by X` (a folder with other users' files is never one user's to trash);
`contains another filesystem`; `reached through a symlink…`; `owner X is a
system account (uid < 1000)`; `unknown owner X`; `/red/nasN is not a mount
point on this node`; `inside .trash` / `.stats`; `filesystem … is no longer
listed in FILESYSTEMS` (retired by duapi).

Users see the same results in the trash panel's **History** tab, and a Keep
that came too late says what happened.

## 6. Rerun a batch

Every claim is saved before anything moves. If a run died halfway (reboot,
OOM), the items show as *being moved* in History; process them again:

```bash
sudo python3 /opt/dutopia/trash_mover.py --api https://dutopia.apps.ocp.corp.local \
  --token-file /etc/dutopia/trash.token --fs /red/nas1 --fs /red/nas2 \
  --batch /var/lib/dutopia-trash/2026-11-05.json
```

Already moved items come out as `skip: gone`; History keeps them as moved.

## 7. Restore (admins, until the purge)

History shows where each item went; the path under `<fs>/.trash/<date>/` is
the original one. Folders come back with their owners and modes:

```bash
sudo tar --use-compress-program 'zstd -d' -xpf \
  /red/nas1/.trash/2026-11-05/projA/run1.tar.zst -C /red/nas1/projA
sudo mv /red/nas1/.trash/2026-11-05/projA/file.dat /red/nas1/projA/
```

## 8. Troubleshooting

| Message | Fix |
|---------|-----|
| `HTTP Error 401` on claim | `TRASH_TOKEN` is unset in the pod, or differs from `/etc/dutopia/trash.token` |
| `refusing non-https API` | use the https Route (`--allow-http` is for local tests) |
| `CERTIFICATE_VERIFY_FAILED` | `--cafile /etc/pki/corp/ca.pem` |
| `none of the server's FILESYSTEMS (…) is mounted on this node` | mount them at exactly those paths |
| `--fs … matches none of the server's FILESYSTEMS` | typo in `--fs`, or the ConfigMap lacks it |
| `another run holds …/.lock` | the previous run is still going (big folders); it is fine |
| `cannot use state dir` | run as root, or pass `--state-dir` |
| `unsafe .trash (a symlink, not a folder, or not owned by this user)` | someone created `<fs>/.trash`; inspect it, move it away, rerun |
| `the server ignored dry_run (older duapi?)` | upgrade duapi; then process the saved batch with `--batch` (§6) |
| `[warn] the server ignored the filesystem filter` | upgrade duapi: until then this node also gets other nodes' items (skipped) |
| `Operation not permitted` on rename/chown | the export squashes root |
| `cannot read …/api/health` | network or Route; the run still purges filesystems it knows, exits 1 |

## 9. Options

| Option | |
|--------|-|
| `--api URL` | duapi base URL (the Route) |
| `--token-file F` | file holding TRASH_TOKEN (default: the `TRASH_TOKEN` env var) |
| `--fs FS` | only these filesystems (repeat; recommended) |
| `--dry-run` | check and list, change nothing |
| `--batch F` | process a saved batch instead of claiming |
| `--cafile F` | CA bundle for a private-CA Route |
| `--keep-days N` | purge trash older than N days (default 30) |
| `--purge FS` | also purge FS/.trash (repeat) |
| `--min-uid N` | never move items of owners below this uid (default 1000) |
| `--state-dir D` | saved batches and lock (default `/var/lib/dutopia-trash`) |
| `--dev` | local testing: `--allow-http --no-mount-check --min-uid 0`, state in `~/.dutopia-trash` |

Server side: `TRASH_UNDO_HOURS` (default 24) is how long Keep can undo;
`TRASH_DIR` (default `/data/trash`) holds `queue.json` and `history.json`.

## 10. Local test (no root, no mounts)

```bash
JWT_SECRET=dev ADMIN_PASSWORD=pw FILESYSTEMS=$HOME/t/fs1,$HOME/t/fs2 \
TRASH_TOKEN=x TRASH_UNDO_HOURS=0 TRASH_DIR=$HOME/t/trashq ./duapi $HOME/t/du.db --port 18088
# queue something in http://localhost:18088, then:
TRASH_TOKEN=x python3 script/trash_mover.py --dev --api http://localhost:18088 [--dry-run]
```

The DB must come from a scan of those folders (`duscan … && dusum … && dudb … --raw … --rebuild`).
