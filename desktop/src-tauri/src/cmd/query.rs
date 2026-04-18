// desktop/src-tauri/src/cmd/query.rs
//
// Read-side commands: users, folders, files. Thin wrappers around the
// `dutopia::db` and `dutopia::item` library modules. Paths are normalized
// through `dutopia::query::normalize_path` so the frontend can send either
// `F:` or `F:\` and both resolve to the `F:\` form the SQLite index stores.

use crate::state::AppState;
use dutopia::db::{list_children, list_users, FolderOut};
use dutopia::item::{get_items, FsItemOut};
use dutopia::query::normalize_path;
use tauri::State;

#[tauri::command]
pub async fn get_users(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let guard = state.db.read().await;
    let Some(pool) = guard.as_ref() else {
        return Ok(vec![]);
    };
    let pool = pool.clone();
    tokio::task::spawn_blocking(move || list_users(&pool).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn get_folders(
    state: State<'_, AppState>,
    path: String,
    users: Vec<String>,
    age: Option<u8>,
) -> Result<Vec<FolderOut>, String> {
    let normalized = normalize_path(&path).ok_or_else(|| format!("invalid path: {path}"))?;
    let guard = state.db.read().await;
    let Some(pool) = guard.as_ref() else {
        tracing::warn!(path = %normalized, "get_folders called but no DB pool — scan first");
        return Ok(vec![]);
    };
    let pool = pool.clone();
    let path_for_log = normalized.clone();
    let result = tokio::task::spawn_blocking(move || {
        list_children(&pool, &normalized, &users, age).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?;
    match &result {
        Ok(items) => tracing::info!(path = %path_for_log, items = items.len(), "get_folders"),
        Err(e) => tracing::error!(path = %path_for_log, err = %e, "get_folders failed"),
    }
    result
}

#[tauri::command]
pub async fn get_files(
    path: String,
    users: Vec<String>,
    age: Option<u8>,
) -> Result<Vec<FsItemOut>, String> {
    let normalized = normalize_path(&path).ok_or_else(|| format!("invalid path: {path}"))?;
    if normalized.is_empty() || normalized == "/" {
        return Ok(vec![]);
    }
    tokio::task::spawn_blocking(move || {
        get_items(&normalized, &users, age).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
