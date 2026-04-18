// rs/src/bin/duapi/mcp.rs
//
// Hand-rolled MCP (Model Context Protocol) endpoint mounted at `/api/mcp`.
// Implements the minimum surface needed for tool-calling clients over the
// MCP HTTP transport: `initialize`, `tools/list`, `tools/call`, and the
// `notifications/initialized` notification (which is acked with 204).
//
// Auth: reuses duapi's JWT `Claims` extractor. Non-admin callers can only
// query their own data (mirrors `handler.rs:179-188`). Cross-user analytics
// (`top_consumers`, `largest_folders`, `cold_data`) require admin.
use axum::{Json, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use dutopia::auth::Claims;
use dutopia::{analytic, db, item};

use crate::query::{normalize_path, parse_users_csv};
use crate::{get_db, get_users};

const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

#[derive(Deserialize)]
pub struct JrpcReq {
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Serialize)]
struct JrpcOk {
    jsonrpc: &'static str,
    id: Value,
    result: Value,
}

#[derive(Serialize)]
struct JrpcErr {
    jsonrpc: &'static str,
    id: Value,
    error: JrpcErrBody,
}

#[derive(Serialize)]
struct JrpcErrBody {
    code: i32,
    message: String,
}

pub async fn handler(claims: Claims, Json(req): Json<JrpcReq>) -> impl IntoResponse {
    if req.jsonrpc != "2.0" {
        let id = req.id.unwrap_or(Value::Null);
        return Json(jrpc_err(id, -32600, "Invalid Request: jsonrpc must be '2.0'"))
            .into_response();
    }

    // Notifications (no `id`) get a 204 with no body. The only one we expect
    // is `notifications/initialized`; any other notification is silently
    // accepted to stay forward-compatible.
    let id = match req.id {
        None => return StatusCode::NO_CONTENT.into_response(),
        Some(v) => v,
    };

    let result = match req.method.as_str() {
        "initialize" => Ok(handle_initialize()),
        "tools/list" => Ok(handle_tools_list()),
        "tools/call" => handle_tools_call(&claims, req.params.unwrap_or(Value::Null)).await,
        other => {
            tracing::warn!(method = %other, "MCP method not found");
            return Json(jrpc_err(id, -32601, &format!("method not found: {other}")))
                .into_response();
        }
    };

    match result {
        Ok(value) => Json(jrpc_ok(id, value)).into_response(),
        Err(msg) => {
            tracing::warn!(err = %msg, "MCP tools/call error");
            Json(jrpc_err(id, -32000, &msg)).into_response()
        }
    }
}

fn jrpc_ok(id: Value, result: Value) -> JrpcOk {
    JrpcOk { jsonrpc: "2.0", id, result }
}

fn jrpc_err(id: Value, code: i32, message: &str) -> JrpcErr {
    JrpcErr {
        jsonrpc: "2.0",
        id,
        error: JrpcErrBody { code, message: message.to_string() },
    }
}

fn handle_initialize() -> Value {
    json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "duapi-mcp",
            "version": env!("CARGO_PKG_VERSION")
        }
    })
}

fn handle_tools_list() -> Value {
    json!({ "tools": tools_catalog() })
}

fn tools_catalog() -> Vec<Value> {
    vec![
        json!({
            "name": "list_users",
            "description": "List usernames present in the scan. Non-admin callers see only themselves.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        }),
        json!({
            "name": "list_folders",
            "description": "Immediate child folders of `path` with per-user / per-age stats. Empty path returns platform roots.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path":  { "type": "string", "description": "Native folder path; omit or empty for platform roots." },
                    "users": { "type": "array", "items": { "type": "string" } },
                    "age":   { "type": "integer", "minimum": 0, "maximum": 2 }
                },
                "additionalProperties": false
            }
        }),
        json!({
            "name": "list_files",
            "description": "Files directly inside `path` (live filesystem read, not the scan DB).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path":  { "type": "string" },
                    "users": { "type": "array", "items": { "type": "string" } },
                    "age":   { "type": "integer", "minimum": 0, "maximum": 2 },
                    "limit": { "type": "integer", "minimum": 1 }
                },
                "required": ["path"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "top_consumers",
            "description": "Top N users by disk usage. Optional path narrows to that subtree. Admin only.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path":  { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "default": 10 }
                },
                "additionalProperties": false
            }
        }),
        json!({
            "name": "largest_folders",
            "description": "Top N immediate-child folders under `path` by disk usage. Admin only.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path":  { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "default": 10 }
                },
                "additionalProperties": false
            }
        }),
        json!({
            "name": "cold_data",
            "description": "Folders dominated by age-2 (>600 days) data with negligible recent (age-0) activity. Admin only.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path":  { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "default": 50 }
                },
                "additionalProperties": false
            }
        }),
        json!({
            "name": "summary",
            "description": "Totals (count, size, disk, atime/mtime range) for (path, users, age).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path":  { "type": "string" },
                    "users": { "type": "array", "items": { "type": "string" } },
                    "age":   { "type": "integer", "minimum": 0, "maximum": 2 }
                },
                "additionalProperties": false
            }
        }),
    ]
}

async fn handle_tools_call(claims: &Claims, params: Value) -> Result<Value, String> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing 'name'".to_string())?
        .to_string();
    let args = params.get("arguments").cloned().unwrap_or(json!({}));

    let payload: Value = match name.as_str() {
        "list_users" => tool_list_users(claims).await?,
        "list_folders" => tool_list_folders(claims, args).await?,
        "list_files" => tool_list_files(claims, args).await?,
        "top_consumers" => tool_top_consumers(claims, args).await?,
        "largest_folders" => tool_largest_folders(claims, args).await?,
        "cold_data" => tool_cold_data(claims, args).await?,
        "summary" => tool_summary(claims, args).await?,
        other => return Err(format!("unknown tool: {other}")),
    };

    let text = serde_json::to_string(&payload).unwrap_or_else(|_| "null".into());
    Ok(json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": payload,
        "isError": false
    }))
}

// ---------- Argument parsing ----------

fn parse_path_arg(args: &Value, key: &str, required: bool) -> Result<Option<String>, String> {
    match args.get(key) {
        Some(Value::String(s)) => match normalize_path(s) {
            Some(p) => Ok(Some(p)),
            None => Err(format!("invalid path: {s}")),
        },
        Some(Value::Null) | None => {
            if required {
                Err(format!("missing required arg: {key}"))
            } else {
                Ok(None)
            }
        }
        Some(_) => Err(format!("'{key}' must be a string")),
    }
}

fn parse_users_arg(args: &Value) -> Result<Vec<String>, String> {
    match args.get("users") {
        Some(Value::Array(arr)) => {
            let mut out = Vec::new();
            for v in arr {
                match v.as_str() {
                    Some(s) => {
                        let t = s.trim();
                        if !t.is_empty() {
                            out.push(t.to_string());
                        }
                    }
                    None => return Err("'users' must be an array of strings".into()),
                }
            }
            Ok(out)
        }
        Some(Value::String(s)) => Ok(parse_users_csv(s)),
        Some(Value::Null) | None => Ok(Vec::new()),
        Some(_) => Err("'users' must be an array".into()),
    }
}

fn parse_age_arg(args: &Value) -> Result<Option<u8>, String> {
    match args.get("age") {
        Some(Value::Number(n)) => {
            let i = n.as_i64().ok_or_else(|| "age must be integer".to_string())?;
            if !(0..=2).contains(&i) {
                return Err("age must be 0, 1, or 2".into());
            }
            Ok(Some(i as u8))
        }
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err("'age' must be an integer".into()),
    }
}

fn parse_limit_arg(args: &Value, default: u32) -> Result<u32, String> {
    match args.get("limit") {
        Some(Value::Number(n)) => {
            let i = n.as_i64().ok_or_else(|| "limit must be integer".to_string())?;
            if i < 1 {
                return Err("limit must be >= 1".into());
            }
            Ok(i as u32)
        }
        Some(Value::Null) | None => Ok(default),
        Some(_) => Err("'limit' must be an integer".into()),
    }
}

fn enforce_self_or_admin(claims: &Claims, requested: &[String]) -> Result<(), String> {
    if claims.is_admin {
        return Ok(());
    }
    if requested.len() == 1 && requested[0] == claims.sub {
        return Ok(());
    }
    Err("forbidden: non-admin must request only their own user".into())
}

fn require_admin(claims: &Claims) -> Result<(), String> {
    if claims.is_admin {
        Ok(())
    } else {
        Err("forbidden: admin required for cross-user analytics".into())
    }
}

// ---------- v1 wrappers ----------

async fn tool_list_users(claims: &Claims) -> Result<Value, String> {
    let users = if claims.is_admin {
        get_users().clone()
    } else {
        vec![claims.sub.clone()]
    };
    Ok(json!(users))
}

async fn tool_list_folders(claims: &Claims, args: Value) -> Result<Value, String> {
    let path = parse_path_arg(&args, "path", false)?.unwrap_or_default();
    let users = parse_users_arg(&args)?;
    let age = parse_age_arg(&args)?;
    if !claims.is_admin {
        enforce_self_or_admin(claims, &users)?;
    }
    let pool = get_db().clone();
    let users_t = users.clone();
    let path_t = path.clone();
    let res = tokio::task::spawn_blocking(move || db::list_children(&pool, &path_t, &users_t, age))
        .await
        .map_err(|e| format!("join: {e}"))?
        .map_err(|e| format!("query: {e}"))?;
    serde_json::to_value(res).map_err(|e| format!("serialize: {e}"))
}

async fn tool_list_files(claims: &Claims, args: Value) -> Result<Value, String> {
    let path = parse_path_arg(&args, "path", true)?.expect("required path checked");
    if path.is_empty() || path == "/" {
        return Err("path '/' or empty not allowed for list_files".into());
    }
    let users = parse_users_arg(&args)?;
    let age = parse_age_arg(&args)?;
    let limit = parse_limit_arg(&args, crate::query::max_page_size() as u32)? as usize;
    if !claims.is_admin {
        enforce_self_or_admin(claims, &users)?;
    }
    let users_t = users.clone();
    let path_t = path.clone();
    let mut items = tokio::task::spawn_blocking(move || item::get_items(path_t, &users_t, age))
        .await
        .map_err(|e| format!("join: {e}"))?
        .map_err(|e| format!("get_items: {e}"))?;
    if items.len() > limit {
        items.truncate(limit);
    }
    serde_json::to_value(items).map_err(|e| format!("serialize: {e}"))
}

// ---------- Analytics wrappers (admin only for cross-user aggregates) ----------

async fn tool_top_consumers(claims: &Claims, args: Value) -> Result<Value, String> {
    require_admin(claims)?;
    let path = parse_path_arg(&args, "path", false)?;
    let limit = parse_limit_arg(&args, 10)?;
    let pool = get_db().clone();
    let res = tokio::task::spawn_blocking(move || analytic::top_consumers(&pool, path.as_deref(), limit))
        .await
        .map_err(|e| format!("join: {e}"))?
        .map_err(|e| format!("query: {e}"))?;
    serde_json::to_value(res).map_err(|e| format!("serialize: {e}"))
}

async fn tool_largest_folders(claims: &Claims, args: Value) -> Result<Value, String> {
    require_admin(claims)?;
    let path = parse_path_arg(&args, "path", false)?;
    let limit = parse_limit_arg(&args, 10)?;
    let pool = get_db().clone();
    let res = tokio::task::spawn_blocking(move || analytic::largest_folders(&pool, path.as_deref(), limit))
        .await
        .map_err(|e| format!("join: {e}"))?
        .map_err(|e| format!("query: {e}"))?;
    serde_json::to_value(res).map_err(|e| format!("serialize: {e}"))
}

async fn tool_cold_data(claims: &Claims, args: Value) -> Result<Value, String> {
    require_admin(claims)?;
    let path = parse_path_arg(&args, "path", false)?;
    let limit = parse_limit_arg(&args, 50)?;
    let pool = get_db().clone();
    let res = tokio::task::spawn_blocking(move || analytic::cold_data(&pool, path.as_deref(), limit))
        .await
        .map_err(|e| format!("join: {e}"))?
        .map_err(|e| format!("query: {e}"))?;
    serde_json::to_value(res).map_err(|e| format!("serialize: {e}"))
}

async fn tool_summary(claims: &Claims, args: Value) -> Result<Value, String> {
    let path = parse_path_arg(&args, "path", false)?;
    let users = parse_users_arg(&args)?;
    let age = parse_age_arg(&args)?;
    if !claims.is_admin {
        enforce_self_or_admin(claims, &users)?;
    }
    let pool = get_db().clone();
    let users_t = users.clone();
    let res = tokio::task::spawn_blocking(move || analytic::summary(&pool, path.as_deref(), &users_t, age))
        .await
        .map_err(|e| format!("join: {e}"))?
        .map_err(|e| format!("query: {e}"))?;
    serde_json::to_value(res).map_err(|e| format!("serialize: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DB_POOL, TEST_DB, USERS};
    use serial_test::serial;

    fn init_db_once() {
        if DB_POOL.get().is_some() {
            return;
        }
        let temp_db = dutopia::db::test_support::build_test_db();
        let pool = dutopia::db::open_pool(&temp_db.path).expect("open_pool");
        let users = dutopia::db::list_users(&pool).expect("list_users");
        let _ = TEST_DB.set(temp_db);
        let _ = DB_POOL.set(pool);
        let _ = USERS.set(users);
    }

    fn admin() -> Claims {
        Claims { sub: "root".into(), is_admin: true, exp: 9_999_999_999usize }
    }
    fn alice() -> Claims {
        Claims { sub: "alice".into(), is_admin: false, exp: 9_999_999_999usize }
    }

    #[test]
    fn initialize_advertises_tools_capability() {
        let v = handle_initialize();
        assert_eq!(v["protocolVersion"], MCP_PROTOCOL_VERSION);
        assert!(v["capabilities"]["tools"].is_object());
        assert_eq!(v["serverInfo"]["name"], "duapi-mcp");
    }

    #[test]
    fn tools_list_advertises_all_seven_tools() {
        let v = handle_tools_list();
        let arr = v["tools"].as_array().unwrap();
        let names: Vec<&str> = arr.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names.len(), 7);
        for expected in [
            "list_users", "list_folders", "list_files",
            "top_consumers", "largest_folders", "cold_data", "summary",
        ] {
            assert!(names.contains(&expected), "missing tool {expected}");
        }
    }

    #[tokio::test]
    #[serial]
    async fn list_users_admin_returns_full_list() {
        init_db_once();
        let v = tool_list_users(&admin()).await.unwrap();
        let arr = v.as_array().unwrap();
        assert!(arr.iter().any(|u| u == "alice"));
        assert!(arr.iter().any(|u| u == "bob"));
    }

    #[tokio::test]
    #[serial]
    async fn list_users_non_admin_returns_self_only() {
        init_db_once();
        let v = tool_list_users(&alice()).await.unwrap();
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0], "alice");
    }

    #[tokio::test]
    #[serial]
    async fn list_folders_admin_returns_children() {
        init_db_once();
        let args = json!({ "path": "/" });
        let v = tool_list_folders(&admin(), args).await.unwrap();
        let arr = v.as_array().unwrap();
        assert!(arr.iter().any(|f| f["path"] == "/docs"));
    }

    #[tokio::test]
    #[serial]
    async fn list_folders_non_admin_without_user_filter_is_forbidden() {
        init_db_once();
        let args = json!({ "path": "/" });
        let err = tool_list_folders(&alice(), args).await.unwrap_err();
        assert!(err.contains("forbidden"));
    }

    #[tokio::test]
    #[serial]
    async fn list_folders_non_admin_with_self_filter_works() {
        init_db_once();
        let args = json!({ "path": "/", "users": ["alice"] });
        let v = tool_list_folders(&alice(), args).await.unwrap();
        assert!(v.is_array());
    }

    #[tokio::test]
    #[serial]
    async fn list_files_rejects_root_path() {
        init_db_once();
        let args = json!({ "path": "/" });
        let err = tool_list_files(&admin(), args).await.unwrap_err();
        assert!(err.contains("not allowed"));
    }

    #[tokio::test]
    #[serial]
    async fn top_consumers_requires_admin() {
        init_db_once();
        let err = tool_top_consumers(&alice(), json!({})).await.unwrap_err();
        assert!(err.contains("admin"));
    }

    #[tokio::test]
    #[serial]
    async fn top_consumers_admin_returns_users_sorted() {
        init_db_once();
        let v = tool_top_consumers(&admin(), json!({})).await.unwrap();
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["user"], "alice");
        assert_eq!(arr[0]["disk"], 100);
    }

    #[tokio::test]
    #[serial]
    async fn largest_folders_admin_returns_root() {
        init_db_once();
        let v = tool_largest_folders(&admin(), json!({})).await.unwrap();
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["path"], "/");
    }

    #[tokio::test]
    #[serial]
    async fn cold_data_admin_finds_docs() {
        init_db_once();
        let v = tool_cold_data(&admin(), json!({ "path": "/" })).await.unwrap();
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["path"], "/docs");
    }

    #[tokio::test]
    #[serial]
    async fn summary_admin_aggregates_root() {
        init_db_once();
        let v = tool_summary(&admin(), json!({})).await.unwrap();
        assert_eq!(v["count"], 3);
        assert_eq!(v["disk"], 150);
    }

    #[tokio::test]
    #[serial]
    async fn summary_non_admin_self_filter_works() {
        init_db_once();
        let v = tool_summary(&alice(), json!({ "users": ["alice"] })).await.unwrap();
        assert_eq!(v["count"], 2);
    }

    #[tokio::test]
    #[serial]
    async fn tools_call_unknown_tool_errors() {
        init_db_once();
        let err = handle_tools_call(&admin(), json!({ "name": "nope", "arguments": {} }))
            .await
            .unwrap_err();
        assert!(err.contains("unknown tool"));
    }

    #[tokio::test]
    #[serial]
    async fn tools_call_wraps_payload_in_envelope() {
        init_db_once();
        let v = handle_tools_call(&admin(), json!({ "name": "list_users", "arguments": {} }))
            .await
            .unwrap();
        assert_eq!(v["isError"], false);
        assert!(v["structuredContent"].is_array());
        assert!(v["content"][0]["type"] == "text");
    }
}
