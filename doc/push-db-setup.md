# push-db.sh: service account and long-lived token setup

<!-- release/v4.0.29/push-db-setup.md -->

This is the runbook for the credentials used by `push-db.sh`, the cron job
that copies a freshly built `all.sum.db` into the running dutopia pod on
OpenShift and restarts it.

## What push-db.sh needs

`push-db.sh` does three things against the cluster:

1. `oc get pods -n peasd-ssdapps-dev -l app=dutopia` — find the dutopia pod
2. `oc cp <db> peasd-ssdapps-dev/<pod>:/data/data.db` — copy the DB
   (internally uses `pods/exec` + tar)
3. `oc delete pod` — restart the pod so duapi reopens the new DB

It authenticates with a dedicated kubeconfig, `~/.kube/cron-config`, which
holds a **long-lived service-account token**. Ordinary `oc login` tokens
expire (~24 h), which is why a cron job cannot use them. The kubeconfig
points at a service account named `cron-copier` in the project
`peasd-ssdapps-dev` on the OCPB cluster.

Everything below runs on the scan/build host (this machine) unless it says
otherwise. You need one interactive `oc login` (any user with
`project-admin` or better on `peasd-ssdapps-dev`) to bootstrap; afterwards
the cron job is self-sufficient.

## 1. Create the service account

```bash
oc create serviceaccount cron-copier -n peasd-ssdapps-dev
```

## 2. Grant it the minimum RBAC

`oc cp` needs `pods/exec`, the pod lookup needs `get/list`, the restart
needs `delete`. Nothing else — the account cannot read secrets, logs or
other projects.

```bash
oc apply -n peasd-ssdapps-dev -f - <<'EOF'
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: cron-copier
rules:
- apiGroups: [""]
  resources: ["pods"]
  verbs: ["get", "list", "delete"]
- apiGroups: [""]
  resources: ["pods/exec"]
  verbs: ["create", "get"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: cron-copier
subjects:
- kind: ServiceAccount
  name: cron-copier
roleRef:
  kind: Role
  name: cron-copier
  apiGroup: rbac.authorization.k8s.io
EOF
```

## 3. Mint the long-lived token

`oc create token cron-copier` produces a **bound** token that expires in
1 h by default — wrong tool for a cron job. The long-lived kind is a
service-account token *secret* annotated with the SA name. These do not
expire; they last until the secret is deleted.

```bash
oc apply -n peasd-ssdapps-dev -f - <<'EOF'
apiVersion: v1
kind: Secret
metadata:
  name: cron-copier-token
  annotations:
    kubernetes.io/service-account.name: cron-copier
type: kubernetes.io/service-account-token
EOF
```

The control plane populates the secret with `token`, `ca.crt` and the
namespace. Extract both:

```bash
TOKEN=$(oc get secret cron-copier-token -n peasd-ssdapps-dev \
    -o jsonpath='{.data.token}' | base64 -d)

oc get secret cron-copier-token -n peasd-ssdapps-dev \
    -o jsonpath='{.data.ca\.crt}' | base64 -d > ~/.kube/ocpb-chain.pem
```

Verify the token works before wiring it into a kubeconfig:

```bash
oc --token "$TOKEN" --server https://api.ocpb.enp.aramco.com.sa:6443 \
   --certificate-authority ~/.kube/ocpb-chain.pem whoami
# expected: system:serviceaccount:peasd-ssdapps-dev:cron-copier
```

## 4. Build the cron kubeconfig

Write `~/.kube/cron-config` with the token inlined. Do not paste the token
into shell history — use the variable:

```bash
umask 077   # file must be 600, it holds the token

cat > ~/.kube/cron-config <<EOF
apiVersion: v1
kind: Config
clusters:
- cluster:
    certificate-authority: $HOME/.kube/ocpb-chain.pem
    server: https://api.ocpb.enp.aramco.com.sa:6443
  name: api-ocpb-enp-aramco-com-sa:6443
contexts:
- context:
    cluster: api-ocpb-enp-aramco-com-sa:6443
    namespace: peasd-ssdapps-dev
    user: system:serviceaccount:peasd-ssdapps-dev:cron-copier/api-ocpb-enp-aramco-com-sa:6443
  name: /api-ocpb-enp-aramco-com-sa:6443/system:serviceaccount:peasd-ssdapps-dev:cron-copier
current-context: /api-ocpb-enp-aramco-com-sa:6443/system:serviceaccount:peasd-ssdapps-dev:cron-copier
preferences: {}
users:
- name: system:serviceaccount:peasd-ssdapps-dev:cron-copier/api-ocpb-enp-aramco-com-sa:6443
  user:
    token: $TOKEN
EOF

chmod 600 ~/.kube/cron-config
unset TOKEN
```

Notes:

- The `certificate-authority` path must be **absolute** — cron's `$HOME`
  is the same user's home, but a literal `$HOME` at write time is expanded
  by the heredoc, so the file ends up with the resolved path.
- The odd-looking cluster/user names are just what `oc login` normally
  generates; any unique strings work as long as context, cluster and user
  references match. The layout above mirrors the current working config.

## 5. Verify

Exactly what `push-db.sh` does, minus the copy:

```bash
export KUBECONFIG=~/.kube/cron-config
oc whoami
# system:serviceaccount:peasd-ssdapps-dev:cron-copier

oc get pods -n peasd-ssdapps-dev -l app=dutopia
# NAME                     READY   STATUS    RESTARTS
# dutopia-5d8f...  1/1     Running   0
```

If `oc get pods` returns a forbidden error, the RoleBinding in step 2 is
missing or in the wrong namespace.

## 6. Rotation and expiry

- The secret-based token **never expires** — that is the point. If the
  cluster was upgraded from a version that disabled secret-based tokens
  (`legacyServiceAccountTokenCleanUp`), re-apply the Secret and confirm
  `.data.token` is populated.
- To rotate: `oc delete secret cron-copier-token -n peasd-ssdapps-dev`,
  re-apply the Secret from step 3, and rewrite `~/.kube/cron-config`.
- The token grants pod copy/delete power in `peasd-ssdapps-dev` only.
  Treat `~/.kube/cron-config` (600) and any copy of the token as a
  credential.

## 7. Wiring into cron

`push-db.sh` sets `KUBECONFIG=$HOME/.kube/cron-config` itself, so no cron
environment is needed beyond a normal user crontab entry:

```cron
30 2 * * 0 /red/peddn8/ssd/usr/ganissa/dutopia/release/v4.0.29/push-db.sh
```

Failure modes land in `push-db.log` next to the script:

- `ERROR: no hay pod dutopia corriendo` — deployment down or RBAC list
  broken.
- `ERROR: token invalido` — secret deleted/rotated but kubeconfig not
  updated, or the SA was removed.
- `oc cp` errors in the log — `pods/exec` permission missing, or `/data`
  in the pod is not writable.
