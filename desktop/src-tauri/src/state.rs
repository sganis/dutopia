// desktop/src-tauri/src/state.rs
//
// Shared application state: the path to the app-data directory (where scan
// outputs live), the currently-open SQLite pool (if a scan has completed),
// and a handle to any in-flight scan process so it can be cancelled.

use dutopia::db::DbPool;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};
use tokio::sync::RwLock;

pub struct AppState {
    pub app_data_dir: PathBuf,
    pub db: RwLock<Option<DbPool>>,
    /// Handle to the currently-running scan child process, if any.
    /// `Mutex<Option<u32>>` holds the OS pid so we can kill it cross-platform.
    pub scan_pid: Mutex<Option<u32>>,
}

impl AppState {
    pub fn new(app_data_dir: PathBuf) -> Self {
        Self {
            app_data_dir,
            db: RwLock::new(None),
            scan_pid: Mutex::new(None),
        }
    }

    pub fn db_path(&self) -> PathBuf {
        self.app_data_dir.join("scan.db")
    }
    pub fn scan_csv_path(&self) -> PathBuf {
        self.app_data_dir.join("scan.csv")
    }
    pub fn sum_csv_path(&self) -> PathBuf {
        self.app_data_dir.join("sum.csv")
    }
    pub fn recent_paths_file(&self) -> PathBuf {
        self.app_data_dir.join("recent-paths.json")
    }
    pub fn read_recent_paths(&self) -> Vec<String> {
        let file = self.recent_paths_file();
        let bytes = match std::fs::read(&file) {
            Ok(b) => b,
            Err(_) => return vec![],
        };
        serde_json::from_slice::<Vec<String>>(&bytes).unwrap_or_default()
    }
    /// Merge `new_paths` into the front of the recent list (MRU), dedupe
    /// while preserving first-seen order, truncate to 5, and persist.
    pub fn push_recent_paths(&self, new_paths: &[String]) -> std::io::Result<()> {
        let mut out: Vec<String> = Vec::with_capacity(5);
        let mut seen = std::collections::HashSet::new();
        for p in new_paths.iter().chain(self.read_recent_paths().iter()) {
            let key = path_key(p);
            if seen.insert(key) {
                out.push(p.clone());
                if out.len() == 5 {
                    break;
                }
            }
        }
        self.write_recent_paths(&out)
    }
    /// Replace the recent paths list verbatim (still deduped + truncated).
    pub fn write_recent_paths(&self, paths: &[String]) -> std::io::Result<()> {
        let mut out: Vec<String> = Vec::with_capacity(5);
        let mut seen = std::collections::HashSet::new();
        for p in paths {
            let key = path_key(p);
            if seen.insert(key) {
                out.push(p.clone());
                if out.len() == 5 {
                    break;
                }
            }
        }
        let file = self.recent_paths_file();
        let bytes = serde_json::to_vec(&out).unwrap_or_else(|_| b"[]".to_vec());
        std::fs::write(file, bytes)
    }
}

/// Dedupe key. On Windows, paths are case-insensitive — fold so
/// `C:\Users` and `c:\users` collapse to one entry.
fn path_key(p: &str) -> String {
    #[cfg(windows)]
    {
        p.to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        p.to_string()
    }
}

/// Initialize `AppState`: create the app-data directory, and try to open an
/// existing DB if one is there from a previous run.
pub fn init(handle: &AppHandle) -> anyhow::Result<AppState> {
    let dir = handle
        .path()
        .app_data_dir()
        .map_err(|e| anyhow::anyhow!("no app_data_dir: {e}"))?;
    std::fs::create_dir_all(&dir)?;
    let state = AppState::new(dir);
    let db_path = state.db_path();
    if db_path.exists() {
        match dutopia::db::open_pool(&db_path) {
            Ok(pool) => {
                *state.db.blocking_write() = Some(pool);
                tracing::info!(path = %db_path.display(), "opened existing DB");
            }
            Err(e) => tracing::warn!(err = %e, "existing DB could not be opened"),
        }
    }
    Ok(state)
}
