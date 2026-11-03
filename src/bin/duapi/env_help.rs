// rs/src/bin/duapi/env_help.rs

/// Environment variable reference shown by `-h`/`--help`. Keep in sync with
/// the vars read in main.rs, oidc.rs, email.rs, query.rs, trash.rs and
/// src/auth.rs.
pub const ENV_HELP: &str = r#"Environment variables (also read from a .env file in the working directory):

Required:
  JWT_SECRET              Secret used to sign the internal session JWTs.
                          duapi refuses to start without it.

Server (CLI flags take precedence over these):
  DB_PATH                 SQLite database path (same as the positional argument)
  STATIC_DIR              UI folder (same as --static-dir)
  PORT                    Listen port (same as --port; default: 8080)
  TLS_CERT, TLS_KEY       PEM certificate and private key paths (same as
                          --tls-cert/--tls-key). Set BOTH to serve HTTPS;
                          omit both to serve plain HTTP. No other certificate
                          is required to run duapi.
  CORS_ORIGIN             Allowed CORS origin (same as --cors-origin),
                          e.g. http://localhost:5173

Authentication:
  Local login (POST /api/login) is always enabled and verifies credentials
  against the host's local accounts (Linux: su/PAM, macOS: dscl). It needs
  no certificates and no OIDC configuration.

  GLOBAL_ADMINS           Comma-separated names granted global admin.
                          Matches the username on local logins; matches the
                          username or any Keycloak group/realm-role on OIDC
                          logins. ADMIN_GROUP (deprecated) is honored as a
                          fallback.
  STORAGE_ADMINS          Semi-colon-separated storage admins with scoped
                          roots, e.g. "user1:/red/peddn7,/red/peddn8;user2:
                          /red/ssd". Full admin inside each root, self-only
                          outside.
  ADMIN_PASSWORD          This password logs any username in as global
                          admin (container mode: no local accounts, no IdP).
                          Active in release builds; keep it secret.
  FAKE_USER,
  FAKE_PASSWORD           Windows only: credentials for the fake
                          authenticator (defaults: current user / "admin").

OIDC / Keycloak single sign-on (optional; enabled only when OIDC_ISSUER is set):
  OIDC_ISSUER             Issuer URL, e.g. https://kc.example.com/realms/main
  OIDC_CLIENT_ID          Client id (required when OIDC_ISSUER is set)
  OIDC_CLIENT_SECRET      Client secret (required when OIDC_ISSUER is set)
  OIDC_REDIRECT_URI       Callback URL (required when OIDC_ISSUER is set),
                          e.g. https://host:8443/api/auth/callback
  OIDC_SCOPES             Requested scopes (default: "openid profile email")
  OIDC_USERNAME_CLAIM     ID-token claim used as the username
                          (default: preferred_username)
  OIDC_POST_LOGIN_REDIRECT
                          Path to land on after login (default: /)
  OIDC_CA_CERT            Path to a PEM CA bundle trusted for backchannel
                          calls to the issuer. Only needed when Keycloak
                          serves a self-signed or private-CA certificate.
  OIDC_API_AUDIENCES      Comma-separated accepted `aud` values for RS256
                          bearer access tokens (default: OIDC_CLIENT_ID)

Filesystems and trash:
  FILESYSTEMS             Comma-separated mount points duscan scans, e.g.
                          "/red/nas1,/red/nas2". The home page lists them
                          instead of `/`, and only paths inside them can be
                          trashed (into <fs>/.trash by the nightly cron).
  TRASH_DIR               Folder of the trash queue, queue.json
                          (default: /data/trash)
  TRASH_TOKEN             Bearer token the trash cron sends to
                          POST /api/trash/claim. Claim is refused when unset.

Request limits:
  REQUEST_TIMEOUT_SECS    Per-request timeout in seconds (default: 30)
  MAX_BODY_BYTES          Max request body size in bytes (default: 65536)
  MAX_PAGE_SIZE           Max rows per /api/files page (default: 2000)

Email notifications (POST /api/cleanup/notify; disabled unless all are set):
  SMTP_HOST, SMTP_USER, SMTP_PASSWORD, SMTP_FROM
  SMTP_PORT               SMTP port (default: 587)
  SMTP_TLS                Use STARTTLS: 1/true/yes/on (default: on)
  MAIL_DOMAIN             Recipient domain: mail is sent to
                          <username>@MAIL_DOMAIN"#;
