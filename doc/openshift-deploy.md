# OpenShift Deployment (TFS → BuildConfig)

This is the production flow. The pod never builds anything — all build
artifacts (`cloud/duapi` binary + `cloud/public/` SPA) are committed to git,
and the OpenShift BuildConfig just runs the root `Dockerfile`, which is
`FROM ubi9` + `COPY cloud .`.

```
bump version → cargo build → bun run build → copy to cloud/ → commit → push (TFS)
    → oc start-build dutopia (peasd-ssdapps-dev) → imagestream dutopia:latest → rollout
```

## One-time facts

- TFS remote: `ssh://eccdevops:22/PEASD_SSD/Stream/_git/dutopia`, branch `master`
- OpenShift project: `peasd-ssdapps-dev`, server `https://api.ocpb.enp.aramco.com.sa:6443`
- `oc` binary: `/red/ssd/usr/ganissa/oc/oc`
- bun binary: `/red/ssd/appl/rh8/bin/bun`
- BuildConfig `dutopia` → ImageStreamTag `dutopia:latest`; Deployment `dutopia`,
  Route `dutopia.apps.ocpb.enp.aramco.com.sa` (edge TLS), PVC `dutopia-pvc` (30Gi,
  mounted at `/data`, holds `data.db`)
- Version lives in `Cargo.toml` AND `browser/package.json` — bump both to the
  same patch number.

## Deploy steps

```bash
# 0. Bump version in Cargo.toml and browser/package.json (same number)

# 1. Build backend
cargo build --release

# 2. Build frontend
cd browser && bun run build && cd ..

# 3. Refresh committed artifacts
rm -rf cloud/public
cp -r browser/public cloud/public
cp target/release/duapi cloud/duapi

# 4. Commit and push
git add -A && git commit -m "<message>" && git push origin master

# 5. Trigger the OpenShift build
oc start-build dutopia -n peasd-ssdapps-dev
oc logs -f build/dutopia-<n> -n peasd-ssdapps-dev   # optional, watch it
oc rollout status deploy/dutopia -n peasd-ssdapps-dev
```

## Notes

- The image rollout picks up the new `dutopia:latest` automatically once the
  build completes (Deployment references the ImageStreamTag).
- DB rebuilds are separate from code deploys: `dudb` runs on a Linux host in
  the identity domain, then `oc rsync` the `data.db` into the PVC and
  `oc rollout restart deploy/dutopia` (details in `deploy/README.md` §5–7).
- `deploy/README.md` describes an alternative podman-push flow that is NOT
  what we use; `deploy/openshift/duapi.yaml` is a template, not the live
  deployment (the live one lives in the `peasd-ssdapps-dev` project).
