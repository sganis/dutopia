# Dutopia MCP

`duapi` exposes its filesystem analytics via the Model Context Protocol on
`POST /api/mcp`, so any MCP-capable client (Claude Code, Cursor, agentic
harnesses, sibling projects such as neos) can call dutopia tools without
re-implementing the REST client and response-shape knowledge.

## Architecture

MCP is served from the same `duapi` binary as the REST API — no second
process, no second port. Choosing this over a separate `dumcp` binary
follows from the deployment target (OpenShift): every client is across the
network, so stdio (the alternative MCP transport) gives nothing, and the
existing pod already terminates TLS, validates JWTs, and rate-limits.

Folding into `duapi` means:

- One image, one cert, one JWT secret, one Route.
- `Claims` extractor at `handler.rs:179-188` enforces the same admin/self
  rules on MCP calls as on REST calls.
- `MAX_BODY_BYTES` and `REQUEST_TIMEOUT_SECS` apply to MCP traffic without
  duplicate knobs.

## Transport

HTTP only. Streamable HTTP transport, single endpoint at `/api/mcp`,
JSON-RPC 2.0 request/response.

| Method                       | Behavior                                       |
|------------------------------|------------------------------------------------|
| `initialize`                 | Returns protocol version + `serverInfo`.       |
| `tools/list`                 | Returns the catalog below.                     |
| `tools/call`                 | Invokes one tool. Result wrapped in MCP envelope (`content[]` text + `structuredContent`). |
| `notifications/initialized`  | Acked with HTTP 204; no body returned.         |

Unknown methods return JSON-RPC error `-32601`. Tool execution errors
return `-32000` with a human-readable message.

## Authentication

Bearer JWT, identical to REST. Obtain a token from `POST /api/login`, pass
it as `Authorization: Bearer <token>` on every MCP request.

Authorization rules:

- **Self-only tools** (`list_users`, `list_folders`, `list_files`,
  `summary`) — non-admin callers must either omit `users` (which then
  defaults to "all") *and* be admin, or pass `users: [<self>]`. Mirrors
  the REST filter at `handler.rs:179-188`.
- **Admin-only tools** (`top_consumers`, `largest_folders`, `cold_data`)
  — refuse non-admin callers entirely. These are cross-user aggregates
  that would leak co-tenant data.

## Tool catalog

All tools are **read-only**. Destructive actions (the `/api/cleanup/*`
endpoints — emails, scripts) are intentionally not exposed via MCP.
Approval and audit belong in the calling host, not here.

### v1 wrappers

Direct call-throughs to existing REST endpoints:

| Tool           | Backs              | Args                                  | Authz       |
|----------------|--------------------|---------------------------------------|-------------|
| `list_users`   | `GET /api/users`   | —                                     | self/admin  |
| `list_folders` | `GET /api/folders` | `path?`, `users[]?`, `age?`           | self/admin  |
| `list_files`   | `GET /api/files`   | `path`, `users[]?`, `age?`, `limit?`  | self/admin  |

### Analytics

New SQL on top of the existing `paths`/`stats`/`users` schema:

| Tool              | Meaning                                                            | Args                       |
|-------------------|--------------------------------------------------------------------|----------------------------|
| `top_consumers`   | Top N users by disk; defaults to platform roots when no `path`.    | `path?`, `limit?` (10)     |
| `largest_folders` | Top N immediate-child folders by `disk_bytes` under `path`.        | `path?`, `limit?` (10)     |
| `cold_data`       | Folders where age-2 disk > 90% of total **and** age-0 disk < 5%.   | `path?`, `limit?` (50)     |
| `summary`         | Totals (count, size, disk, linked) plus atime/mtime range.         | `path?`, `users[]?`, `age?`|

`age` buckets follow the rest of dutopia: `0` (<60d), `1` (60–600d),
`2` (>600d).

**Path scoping for analytics.** When a `path` is supplied, queries scope
to that exact node — `dusum` has already rolled up the subtree, so a
folder's `stats` row already includes its descendants. When `path` is
omitted, queries scope to *platform roots* (children of the synthetic root
row, `full_path = ''`): on Linux that is `/`, on Windows the union of
drive roots and UNC server entries. This avoids the double-counting that
would occur if queries summed every ancestor row in the table.

### Reserved (not yet implemented)

Will land when history support arrives — see `deltas.md`:

| Tool               | Meaning                                            |
|--------------------|----------------------------------------------------|
| `list_scans`       | Enumerate snapshot DBs in the history dir.         |
| `list_deltas`      | Per-folder growth between two snapshots.           |
| `growth_hotspots`  | Top growers/shrinkers across a window.             |

## Response shape

`tools/call` results are wrapped in the standard MCP envelope:

```json
{
  "content": [{ "type": "text", "text": "<JSON-encoded payload>" }],
  "structuredContent": <payload>,
  "isError": false
}
```

Clients that understand `structuredContent` should prefer it; the `text`
block is a stringified copy for fallback parsers.

The `<payload>` itself uses serde types already exported by the library:

- `FolderOut { path, users: { <user>: { <age>: Age } } }` — `rs/src/db.rs`
- `Age { count, size, disk, linked, atime, mtime }` — `rs/src/db.rs`
- `FsItemOut { path, owner, size, accessed, modified }` — `rs/src/item.rs`
- `UserTotal`, `FolderTotal`, `ColdFolder`, `Summary` — `rs/src/analytic.rs`

## Usage

Two-step: get a JWT from `/api/login`, then call `/api/mcp` with the token
as a bearer header. All bodies are JSON-RPC 2.0.

### bash / WSL / OpenShift Route

```bash
TOKEN=$(curl -s -X POST http://localhost:8080/api/login \
  -H 'Content-Type: application/json' \
  -d '{"username":"alice","password":"s3cret"}' \
  | jq -r .access_token)

curl -s -X POST http://localhost:8080/api/mcp \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' \
  | jq .
```

Call a tool — `params` carries `{ name, arguments }`:

```bash
curl -s -X POST http://localhost:8080/api/mcp \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call",
       "params":{"name":"top_consumers","arguments":{"limit":5}}}' \
  | jq .
```

### Windows cmd.exe

Inner double-quotes must be escaped with `\"`:

```cmd
curl -s -X POST http://localhost:8080/api/login -H "Content-Type: application/json" -d "{\"username\":\"alice\",\"password\":\"s3cret\"}"
```

Copy `access_token` from the response, then:

```cmd
curl -s -X POST http://localhost:8080/api/mcp -H "Authorization: Bearer <TOKEN>" -H "Content-Type: application/json" -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}"
```

### Windows PowerShell

```powershell
$T = (Invoke-RestMethod -Method Post -Uri http://localhost:8080/api/login -ContentType application/json -Body '{"username":"alice","password":"s3cret"}').access_token

Invoke-RestMethod -Method Post -Uri http://localhost:8080/api/mcp -Headers @{Authorization="Bearer $T"} -ContentType application/json -Body '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | ConvertTo-Json -Depth 10
```

For OpenShift, swap `http://localhost:8080` for the Route URL (e.g.
`https://duapi.<cluster-domain>`).

### MCP Inspector

```
npx @modelcontextprotocol/inspector
```

Point it at `http://<host>/api/mcp` with a bearer token from `/api/login`
to browse the catalog and invoke tools interactively.

## File map

| File                          | Role                                                  |
|-------------------------------|-------------------------------------------------------|
| `rs/src/bin/duapi/mcp.rs`     | JSON-RPC dispatch, tool registry, schemas, authz.     |
| `rs/src/bin/duapi/main.rs`    | Mounts `/api/mcp` on the existing API router.         |
| `rs/src/analytic.rs`          | Analytics SQL (`top_consumers`, `largest_folders`, `cold_data`, `summary`) plus unit tests. |
| `rs/src/db.rs`                | `list_users`, `list_children`, fixture builder.       |
| `rs/src/item.rs`              | `get_items` — live filesystem read for `list_files`.  |
| `rs/src/auth.rs`              | `Claims` extractor (shared with REST).                |
| `rs/src/bin/duapi/query.rs`   | `normalize_path`, `parse_users_csv`.                  |

No new binary. No new top-level dependency — JSON-RPC is hand-rolled
against `serde_json` (the spec is small and avoids fighting Axum's
extractor model).

## Open questions

1. When `deltas.md` lands, does `duapi` open multiple snapshot DBs in one
   process, or proxy to sibling read-only DBs via a `--history-dir` flag?
