# duapi: build on Linux, deploy to on-prem OpenShift

The cluster cannot build. Compile on a Linux host, package the artifacts into a
thin image, push, apply.

```
Linux host:  build-linux.sh -> dist/  -> podman build -> push
                dudb        -> data.db ──────────────────────> PVC
OpenShift:                                                     oc apply
```

Everything below runs on the Linux build host unless it starts with `oc`.

---

## 1. Install the toolchain (once)

```bash
sudo dnf -y install gcc gcc-c++ make cmake perl clang git sqlite binutils
sudo dnf -y module enable nodejs:22 && sudo dnf -y install nodejs
curl -sSfL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.89.0
source "$HOME/.cargo/env"
```

`gcc`/`make` build the bundled SQLite; `cmake`/`perl`/`clang` build `aws-lc-sys`.

## 2. Build

```bash
git clone <repo> && cd dutopia
./deploy/build-linux.sh
```

Output: `dist/{duapi,dudb,public}` for the image, other binaries in
`target/release/`. The script prints the minimum glibc the binary needs.

## 3. Pick the base image to match glibc

A binary built on a newer glibc than the runtime dies with `GLIBC_2.3x not found`.

| Build host   | glibc | Base image in `deploy/Dockerfile` |
|--------------|-------|-----------------------------------|
| RHEL/Rocky 9 | 2.34  | `ubi9/ubi-minimal` (default)      |
| RHEL/Rocky 8 | 2.28  | `ubi8/ubi-minimal`                |
| Ubuntu 22.04 | 2.35  | `ubuntu:22.04`                    |
| Ubuntu 24.04 | 2.39  | `ubuntu:24.04`                    |

Older build host than runtime is always safe. The reverse is not.

## 4. Build and push the image

```bash
podman build -f deploy/Dockerfile -t registry.corp.local/dutopia/duapi:4.0.26 .
podman push registry.corp.local/dutopia/duapi:4.0.26
```

## 5. Build the database

Run on a host in the same identity domain as the scan — `dudb` resolves
uid -> username locally. Never in the cluster.

```bash
./target/release/duscan /path -o scan.csv
./target/release/dusum  scan.csv -o sum.csv
./target/release/dudb   sum.csv --raw scan.csv -o /nfs/share/data.db --rebuild \
                        --tmp-dir /local/scratch
oc rollout restart deployment/duapi
```

`--raw` is mandatory: the pod cannot see the scanned filesystem, so
`/api/files` must come from the DB. `dudb` leaves the DB in rollback-journal
(DELETE) mode — a read-only open of a WAL database would need to create a
`-shm` file next to it — so no `VACUUM`/`journal_mode` step is needed (a
`VACUUM` of a billion-row DB rewrites all of it).

The `--raw` pass sorts the per-file rows on disk, then packs each folder's
files into one compressed row (totals of all of them, but only the 100 largest
of at least 1 MB per folder, owner and age are listed; see `--max-files` and
`--min-file-size`), so its cost stays linear at a billion
files. Point `--tmp-dir` (or `DUDB_TMPDIR`) at
local disk with about a quarter of the raw CSV size free; `--sort-mem` (MiB,
default 8192) sets the in-memory sort budget. The DB is built as
`<output>.building` and renamed over `<output>` only when complete, so the old
DB stays readable during the rebuild. Restart the pod afterwards so its
connections reopen the new file.

## 6. Deploy

Edit the image reference and Route host in `duapi.yaml`, and the OIDC values in
`config.example.yaml`, then:

```bash
oc new-project dutopia
./deploy/openshift/secret.example.sh
oc apply -f deploy/openshift/config.example.yaml
oc apply -f deploy/openshift/duapi.yaml
oc rollout status deploy/duapi
```

## 7. Load the database

```bash
POD=$(oc get pod -l app=duapi -o name | head -1)
oc rsync ./ "${POD#pod/}":/data/ --include=data.db --exclude='*'
oc rollout restart deploy/duapi
```

## 8. Verify

```bash
oc logs deploy/duapi | head -20
```

Expected:

```
Auth mode: password + oidc
Opening database: /data/data.db
Loaded N users
File index: present (/api/files served from DB)
Serving on http://0.0.0.0:8080  (static dir: /app/public)
```

`File index: absent` means the DB was built without `--raw`.
With `FILESYSTEMS` set, the log also shows `FILESYSTEMS=...` and
`Trash queue: /data/trash/queue.json (claim enabled)`.

## 9. Trash cron

Without `FILESYSTEMS` the web UI shows no trash icon, so the trash cart (and
its Cleanup script / Notify buttons) is not available. With it, the trash
icon queues items in `/data/trash/queue.json` (on the PVC). Keep in
the UI undoes it for 24h. After that, a cron on a node that mounts the
filesystems moves them to `<fs>/.trash/<date>/…`, packs folders into
`.tar.zst`, and deletes trash older than 30 days. Users cannot restore after
the 24h window; admins can, from `<fs>/.trash` until the purge.

On that node, as root (Python 3, GNU tar and zstd):

```bash
install -D -m 0755 script/trash_mover.py /opt/dutopia/trash_mover.py
install -D -m 0644 script/trash_fsops.py /opt/dutopia/trash_fsops.py  # next to it
install -D -m 0600 trash.token /etc/dutopia/trash.token   # same value as the Secret
crontab -e
0 2 * * * python3 /opt/dutopia/trash_mover.py --api https://dutopia.apps.ocp.corp.local --token-file /etc/dutopia/trash.token --fs /red/nas1 --fs /red/nas2
```

`--fs` (one per FILESYSTEMS entry) limits what root may touch to those
filesystems even if the API were compromised; keep it. The script refuses a
plain-http `--api`, owners below uid 1000 (`--min-uid`), folders that hold
another owner's files or another mount, paths reached through a symlink, and
a `<fs>/.trash` that is a symlink or not root's. The node must mount the
filesystems with root access (no `root_squash`), or the renames fail.

Each node claims only the items of the FILESYSTEMS entries it mounts (it
reads the list from `/api/health`), so with several cron nodes, each one takes
its own filesystems and leaves the rest queued. One run at a time per node
(`/var/lib/dutopia-trash/.lock`).

Claiming records each item as *being moved*; after the run the cron reports
what it did (`POST /api/trash/report`, in chunks of 100). The trash panel's
**History** tab shows each item as being moved, moved (and where to), skipped
(and why: gone, not owned by its owner any more, a folder holding other
users' files, …), failed, or kept (and by whom). Keep on an item the cron
already took says what happened instead of failing silently, and an item the
cron moved cannot be queued again until the next scan drops it from the
listing. Due items of a filesystem removed from FILESYSTEMS are retired as
skipped (no node would ever take them).

**Upgrade duapi before the cron.** A cron asks the server to confirm
`--dry-run` and the per-node filter; against an older duapi that ignores
them, a dry run would really claim the items, so the cron stops, saves them
to the state dir and tells you to rerun with `--batch` after upgrading.

Check before the first real run, as root: it lists what it would move or
purge and changes nothing (the items stay queued):

```bash
python3 /opt/dutopia/trash_mover.py --api https://dutopia.apps.ocp.corp.local \
  --token-file /etc/dutopia/trash.token --fs /red/nas1 --fs /red/nas2 --dry-run
```

Each run saves what it claimed to `/var/lib/dutopia-trash/<date>.json`; if a
run dies halfway, rerun that batch with `--batch /var/lib/dutopia-trash/<date>.json`.
Add `--cafile` when the Route uses a private CA. `TRASH_UNDO_HOURS` (duapi,
default 24) sets how long Keep can undo.

**Restore** (admins, until the 30-day purge). The trash keeps the path under
`<fs>/.trash/<date>/`; History shows the exact archive. A folder comes back
with its owners and modes:

```bash
tar --use-compress-program 'zstd -d' -xpf /red/nas1/.trash/2026-11-04/proj/run1.tar.zst -C /red/nas1/proj
mv /red/nas1/.trash/2026-11-04/proj/file.dat /red/nas1/proj/   # a single file
```

**Local test** (macOS/Linux, no root, no mounts): start duapi with
`FILESYSTEMS=<dirs> TRASH_TOKEN=x TRASH_UNDO_HOURS=0`, queue something in the
UI, then `TRASH_TOKEN=x python3 script/trash_mover.py --dev --api http://localhost:8080`
(add `--dry-run` to only look).

---

## Gotchas

- **Password login does not work in the container.** `POST /api/login` shells
  out to `su` (`src/auth.rs`); there are no local accounts and the UID is
  unprivileged. Configure OIDC or nobody can log in. `OIDC_REDIRECT_URI` must
  match the Route host exactly and be registered in Keycloak.
- **`JWT_SECRET`** — `duapi` exits at startup if unset.
- **Random UID** — `restricted-v2` overrides `USER 1001` with a random UID in
  group 0. The Dockerfile's `chgrp -R 0 /app && chmod -R g=u /app` is what keeps
  `/app` readable. Do not remove it.
- **TLS** terminates at the Route. Leave `TLS_CERT`/`TLS_KEY` unset; setting
  only one is fatal.
- **Port 8080** is the default and is above 1024 — no SCC change needed.
- **Trash claim 401** — `TRASH_TOKEN` is unset in the pod or differs from
  `/etc/dutopia/trash.token` on the cron node.
- **`CORS_ORIGIN`** stays unset while `duapi` serves the SPA itself.
- **Memory** — `db.rs` opens `max(num_cpus, 4)` connections at 64 MiB cache
  each. Raise the 1Gi limit together with the CPU limit.
- **Private Keycloak CA** — `reqwest` uses bundled webpki roots. Mount the
  corporate bundle and set `OIDC_CA_CERT`.
- **The SPA builds to `browser/public/`**, not `browser/build/`. The CI
  workflow uploads `browser/build/**`, so that artifact is empty and unusable
  here.
- **Air-gapped** — `cargo vendor --versioned-dirs ./vendor` on a connected
  machine, then build with `--offline`. (`script/vendor.sh` is stale, it does
  `cd rs`.)
