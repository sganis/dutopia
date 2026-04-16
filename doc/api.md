# Dutopia API Reference

Base URL: `http://<host>:<port>/api`

## Authentication
All endpoints (except `/health` and `/login`) require a Bearer token:
```
Authorization: Bearer <jwt>
```

## POST /login
Authenticate against OS user credentials.

Request:
```json
{ "username": "alice", "password": "secret" }
```
Response:
```json
{ "access_token": "...", "token_type": "Bearer" }
```
Errors: `400`, `401`.

## GET /users
Admin users receive full user list; non-admins receive only their own username.

## GET /folders
Query aggregated folders.

Query params:
- `path` (required, default `/`)
- `users` (comma-separated)
- `age` (0|1|2)
- `limit` (max page size)
- `offset`

Response: array of `{ path, users: { username: { age: stats }}}`.

## GET /files
List files within a directory (Unix only).

Query params:
- `path` (required)
- `users` (comma-separated)
- `age` (0|1|2)
- `limit` (max page size)
- `offset`

Response: array of `{ path, owner, size, accessed, modified }`.

## GET /health
Health check for load balancers/monitoring.

Response:
```json
{ "status": "ok", "version": "4.x.x", "uptime_secs": 123 }
```
