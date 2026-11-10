# Keycloak / OIDC authentication on OpenShift

<!-- doc/keycloak-oidc-setup.md -->

Runbook for the OIDC single sign-on used by the production `dutopia`
deployment in `peasd-ssdapps-dev` on the OCPB cluster. The IdP is the
corporate Keycloak (neosso); duapi implements the OIDC Authorization Code
flow with PKCE in `src/bin/duapi/oidc.rs`.

For how the flow works end-to-end (endpoints, token verification, state
cookie), see `doc/handbook.md` §3.1. This file only covers the production
configuration and how to change it.

## What the IdP side provides

The Keycloak admins supply three values:

| Item | Production value |
|---|---|
| Client ID | `dutopia` |
| Realm / discovery URL | `https://neosso-test.apps.ocpx.enp.aramco.com.sa/realms/neoai` |
| Client secret | held in `OIDC_CLIENT_SECRET` on the deployment (not written here) |

The client must have **Standard flow** enabled, PKCE `S256` allowed, and a
**Valid Redirect URI** registered for
`https://dutopia.apps.ocpb.enp.aramco.com.sa/api/auth/callback`.

## Environment variables on the deployment

| Variable | Value |
|---|---|
| `OIDC_ISSUER` | `https://neosso-test.apps.ocpx.enp.aramco.com.sa/realms/neoai` |
| `OIDC_CLIENT_ID` | `dutopia` |
| `OIDC_CLIENT_SECRET` | the client secret (already set) |
| `OIDC_REDIRECT_URI` | `https://dutopia.apps.ocpb.enp.aramco.com.sa/api/auth/callback` |
| `OIDC_CA_CERT` | `/etc/oidc-ca/ca-bundle.crt` |

Notes:

- `OIDC_ISSUER` is the **realm URL** only. duapi appends
  `/.well-known/openid-configuration` itself — do not paste the full
  discovery URL, or discovery will be fetched from
  `.../openid-configuration/.well-known/openid-configuration`.
- `OIDC_REDIRECT_URI` must match the Keycloak client's registered redirect
  URI **exactly** (scheme, host, path). The callback route in duapi is
  `/api/auth/callback`, not `/callback`.
- Optional knobs (defaults are fine): `OIDC_SCOPES`
  (`openid profile email`), `OIDC_USERNAME_CLAIM`
  (`preferred_username`), `OIDC_POST_LOGIN_REDIRECT` (`/`),
  `OIDC_API_AUDIENCES` (comma-separated; defaults to the client ID).

## The CA bundle (why `OIDC_CA_CERT` is needed)

`reqwest` uses bundled webpki roots, which do not include the Aramco
internal CAs. The neosso route serves a certificate issued by
`Saudi Aramco 2014 Issuing CA 1`, so without a custom trust root the pod
fails at boot with:

```
Warning: OIDC init failed, password auth only: ... invalid peer certificate: UnknownIssuer
```

The fix mounts the namespace's existing `config-trusted-cabundle`
ConfigMap (the OCP-brokered corporate CA bundle, ~150 certs) at
`/etc/oidc-ca`:

```bash
oc set volume deploy/dutopia --add --name=oidc-ca --type=configmap \
   --configmap-name=config-trusted-cabundle --mount-path=/etc/oidc-ca
```

Before mounting a different bundle, verify it actually covers the IdP:

```bash
oc get cm config-trusted-cabundle -o 'jsonpath={.data.ca-bundle\.crt}' > /tmp/ca.pem
openssl s_client -connect neosso-test.apps.ocpx.enp.aramco.com.sa:443 \
   -servername neosso-test.apps.ocpx.enp.aramco.com.sa </dev/null 2>/dev/null \
   | sed -n '/BEGIN CERT/,/END CERT/p' > /tmp/server.pem
openssl verify -CAfile /tmp/ca.pem /tmp/server.pem   # must print: OK
```

## Applying the configuration

Everything is plain deployment env/volumes; no image change is involved.

```bash
oc set env deploy/dutopia \
   OIDC_ISSUER="https://neosso-test.apps.ocpx.enp.aramco.com.sa/realms/neoai" \
   OIDC_REDIRECT_URI="https://dutopia.apps.ocpb.enp.aramco.com.sa/api/auth/callback" \
   OIDC_CA_CERT="/etc/oidc-ca/ca-bundle.crt"

oc rollout status deploy/dutopia
```

(The `oc set volume` command above only needs to run once, or again if
the volume is ever removed.)

## Verify

1. Pod logs must show OIDC enabled (not "password auth only"):

   ```bash
   oc logs deploy/dutopia | grep -i "oidc\|auth mode"
   # "OIDC enabled" with issuer/client_id/redirect_uri, then
   # Auth mode: password + oidc
   ```

2. The mode endpoint through the Route advertises OIDC:

   ```bash
   curl -sk https://dutopia.apps.ocpb.enp.aramco.com.sa/api/auth/mode
   # {"mode":"oidc","login_url":"/api/auth/login"}
   ```

3. The login endpoint redirects to Keycloak with the right client and
   redirect URI:

   ```bash
   curl -sk -o /dev/null -w "%{http_code} -> %{redirect_url}\n" \
      https://dutopia.apps.ocpb.enp.aramco.com.sa/api/auth/login
   # 303 -> https://neosso-test.../protocol/openid-connect/auth?...client_id=dutopia...
   ```

4. Keycloak accepts the client (a 400 here means the client or its
   redirect URI is not registered — contact the Keycloak admins):

   ```bash
   curl -sk -o /dev/null -w "%{http_code}\n" \
      "https://neosso-test.apps.ocpx.enp.aramco.com.sa/realms/neoai/protocol/openid-connect/auth?response_type=code&client_id=dutopia&redirect_uri=https%3A%2F%2Fdutopia.apps.ocpb.enp.aramco.com.sa%2Fapi%2Fauth%2Fcallback&scope=openid&state=x&nonce=y&code_challenge=S6mMozuRUgoZtmN7ViJ_OnSSRboOmVml5tuaiczZYp4&code_challenge_method=S256"
   # 303 (redirect to the Keycloak login page)
   ```

Then log in through the browser at
https://dutopia.apps.ocpb.enp.aramco.com.sa — the Login screen renders a
**Sign in with Keycloak** button.

## Admin rights

`GLOBAL_ADMINS` (deprecated name `ADMIN_GROUP`) is a comma-separated list
matched case-insensitively against the username **or** any entry in the
token's `groups` claim (Keycloak realm roles arrive there). Current
production value lists individual usernames. To grant a whole realm role
instead, append it:

```bash
oc set env deploy/dutopia GLOBAL_ADMINS="<existing>,NeosMax"
```

## Troubleshooting

| Symptom | Cause / fix |
|---|---|
| `invalid peer certificate: UnknownIssuer` at boot | `OIDC_CA_CERT` unset, volume missing, or bundle doesn't cover the IdP — see CA section above |
| `OIDC_CLIENT_ID required` / `Client not found` | env var missing, empty, or wrapped in stray quotes (duapi strips one pair, but check `oc set env --list`) |
| 400 `invalid_request` or `unauthorized_client` from Keycloak | redirect URI mismatch — `OIDC_REDIRECT_URI` must match the client's registered Valid Redirect URI exactly |
| `mode: "password"` from `/api/auth/mode` | OIDC init failed; check boot logs for the reason |
| Login works, but user is not admin | username not in `GLOBAL_ADMINS` and no matching group claim in the token |
| State cookie errors on callback | the flow requires HTTPS (`Secure` cookie) — always go through the Route, never the plain-HTTP service |

## Secrets handling

The client secret lives in the Deployment env (`oc set env`), visible to
anyone with `get deployment` rights in the namespace. That matches the
existing `JWT_SECRET` / `ADMIN_PASSWORD` handling on this deployment, but
if that needs tightening, move all three into an Opaque Secret and switch
the Deployment to `envFrom.secretRef` (see `deploy/openshift/secret.example.sh`
for the intended shape).
