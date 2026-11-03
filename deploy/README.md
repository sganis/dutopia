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

### First install

Edit, in `deploy/openshift/`:

- `duapi.yaml`: the image (the tag you pushed in step 4) and the Route host.
- `config.example.yaml`: the OIDC values, `GLOBAL_ADMINS`, and `FILESYSTEMS`:
  the mount points duscan scans, comma-separated, exactly as they appear in
  the UI (e.g. `/red/nas1,/red/nas2`). The home page lists them and the trash
  works only inside them. Leave it unset and there is no trash.

```bash
oc new-project dutopia
./deploy/openshift/secret.example.sh      # JWT_SECRET, TRASH_TOKEN (saved to ./trash.token), OIDC secret
oc apply -f deploy/openshift/config.example.yaml
oc apply -f deploy/openshift/duapi.yaml
oc rollout status deploy/duapi
```

Keep `trash.token` (git ignores it) for the trash cron and in your password
manager; it is the same value as the Secret's `TRASH_TOKEN`.

### Upgrade an existing deployment

```bash
# 1. New image (steps 2-4), then point the Deployment at it:
oc set image deploy/duapi duapi=registry.corp.local/dutopia/duapi:<new-tag>

# 2. Config: add FILESYSTEMS (and any other new keys) and re-apply.
oc apply -f deploy/openshift/config.example.yaml

# 3. Secret: secret.example.sh only creates; add the trash token to the
#    existing Secret instead.
[ -f trash.token ] || (umask 077; openssl rand -hex 32 > trash.token)
oc set data secret/duapi-secret --from-file=TRASH_TOKEN=trash.token

# 4. One pod at a time (the trash queue has a single writer). Switching an
#    existing Deployment to Recreate with `oc apply` fails on the leftover
#    rollingUpdate block, so patch it once:
oc patch deploy/duapi -p '{"spec":{"strategy":{"type":"Recreate","rollingUpdate":null}}}'

# 5. ConfigMap and Secret are read at pod start:
oc rollout restart deploy/duapi
oc rollout status deploy/duapi
```

Then upgrade the trash cron on its node(s) ([TRASH.md](TRASH.md)): always
duapi first, the cron second.

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

With `FILESYSTEMS` set, the log also shows:

```
FILESYSTEMS=/red/nas1,/red/nas2
Trash queue: /data/trash/queue.json (claim enabled)
```

`(claim disabled: TRASH_TOKEN not set)` means the Secret lacks `TRASH_TOKEN`
(step 6, upgrade 3). Then check from anywhere, and that the pod can write the
queue on the PVC:

```bash
curl -s https://dutopia.apps.ocp.corp.local/api/health
# {"filesystems":["/red/nas1","/red/nas2"],"trash_undo_hours":24,...}
oc exec deploy/duapi -- sh -c 'mkdir -p /data/trash && touch /data/trash/.w && rm /data/trash/.w && echo writable'
```

In the UI the home page lists the filesystems; trash something, then
`oc exec deploy/duapi -- cat /data/trash/queue.json` shows it.

## 9. Trash cron

The trash icon only queues items; a root cron on a node that mounts the
filesystems moves them to `<fs>/.trash` after the 24h undo window and purges
them after 30 days. Install, dry run, schedule, rerun, restore and
troubleshooting: **[TRASH.md](TRASH.md)**.

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
