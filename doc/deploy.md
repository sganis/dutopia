# Deployment Guide

## Environment Variables
- `JWT_SECRET` (required)
- `ADMIN_GROUP` (optional)
- `PORT` (default 8080)
- `STATIC_DIR` (defaults to `./public` next to binary)
- `BASE_PATH` (restrict API paths)
- `CORS_ORIGIN` (allowed UI origin)
- `TLS_CERT`, `TLS_KEY` (enable HTTPS)
- `REQUEST_TIMEOUT_SECS` (default 30)
- `RATE_LIMIT_PER_MIN` (default 300)
- `MAX_PAGE_SIZE` (default 2000)

## Build Artifacts
- Rust binaries: `rs/target/release/`
- UI build: `browser/build/` (copy to the static directory used by `duapi`)

## Docker (example)
1. Build Rust + UI in a multi-stage Dockerfile.
2. Copy `duapi` and `browser/build/` into a slim runtime image.
3. Set `STATIC_DIR=/app/public` and `JWT_SECRET` at runtime.

## Systemd (example)
Create a service that runs:
```
ExecStart=/opt/dutopia/duapi /opt/dutopia/data.sum.csv --port 8000
Environment=JWT_SECRET=... BASE_PATH=/data
```

## Reverse Proxy
Use nginx or caddy to terminate TLS and forward to `duapi` on localhost. Ensure CORS is set to the UI origin when hosting separately.

## Security Hardening
- Run `duapi` as a non-root user.
- Set strong `JWT_SECRET` and rotate periodically.
- Restrict `BASE_PATH` to a dedicated mount.
- Enable TLS in production.
