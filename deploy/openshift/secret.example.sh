#!/usr/bin/env bash
# deploy/openshift/secret.example.sh — create the duapi Secret. Do not commit real values.
set -euo pipefail
oc create secret generic duapi-secret \
  --from-literal=JWT_SECRET="$(openssl rand -hex 32)" \
  --from-literal=OIDC_CLIENT_SECRET="<keycloak-client-secret>"
