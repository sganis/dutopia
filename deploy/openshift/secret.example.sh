#!/usr/bin/env bash
# deploy/openshift/secret.example.sh — create the duapi Secret. Do not commit real values.
set -euo pipefail
# TRASH_TOKEN is shared with the trash cron (deploy/README.md, "Trash cron"):
# generate it once, put the same file at /etc/dutopia/trash.token on that node.
[ -f trash.token ] || (umask 077; openssl rand -hex 32 > trash.token)
oc create secret generic duapi-secret \
  --from-literal=JWT_SECRET="$(openssl rand -hex 32)" \
  --from-literal=TRASH_TOKEN="$(cat trash.token)" \
  --from-literal=OIDC_CLIENT_SECRET="<keycloak-client-secret>"
