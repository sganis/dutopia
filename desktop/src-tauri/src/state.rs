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

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn tmp_state() -> (AppState, PathBuf) {
        // Per-test dir so parallel tests don't race on recent-paths.json.
        let base = env::temp_dir().join(format!(
            "dutopia-test-{}-{}",
            std::process::id(),
            rand_suffix()
        ));
        std::fs::create_dir_all(&base).unwrap();
        (AppState::new(base.clone()), base)
    }

    fn rand_suffix() -> String {
        // Monotonic-ish unique suffix without pulling in a dep.
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("{nanos}")
    }

    #[test]
    fn path_key_case_behavior() {
        #[cfg(windows)]
        assert_eq!(path_key("C:\\Users"), path_key("c:\\users"));
        #[cfg(not(windows))]
        assert_ne!(path_key("/Users"), path_key("/users"));
    }

    #[test]
    fn db_and_csv_paths_join_to_app_dir() {
        let (s, base) = tmp_state();
        assert_eq!(s.db_path(), base.join("scan.db"));
        assert_eq!(s.scan_csv_path(), base.join("scan.csv"));
        assert_eq!(s.sum_csv_path(), base.join("sum.csv"));
        assert_eq!(s.recent_paths_file(), base.join("recent-paths.json"));
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn read_recent_paths_returns_empty_when_missing() {
        let (s, base) = tmp_state();
        assert!(s.read_recent_paths().is_empty());
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn write_dedupes_and_truncates_to_five() {
        let (s, base) = tmp_state();
        let paths: Vec<String> = ["a", "b", "c", "d", "e", "f", "a"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        s.write_recent_paths(&paths).unwrap();
        assert_eq!(s.read_recent_paths(), vec!["a", "b", "c", "d", "e"]);
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn push_prepends_new_and_dedupes() {
        let (s, base) = tmp_state();
        s.write_recent_paths(&["a".into(), "b".into(), "c".into()])
            .unwrap();
        // Push duplicates `b` (should move to front) plus new `x`.
        s.push_recent_paths(&["x".into(), "b".into()]).unwrap();
        assert_eq!(s.read_recent_paths(), vec!["x", "b", "a", "c"]);
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn push_truncates_combined_list_to_five() {
        let (s, base) = tmp_state();
        s.write_recent_paths(&[
            "a".into(),
            "b".into(),
            "c".into(),
            "d".into(),
            "e".into(),
        ])
        .unwrap();
        s.push_recent_paths(&["x".into(), "y".into()]).unwrap();
        assert_eq!(s.read_recent_paths(), vec!["x", "y", "a", "b", "c"]);
        std::fs::remove_dir_all(base).ok();
    }

    #[cfg(windows)]
    #[test]
    fn push_dedupes_case_insensitively_on_windows() {
        let (s, base) = tmp_state();
        s.write_recent_paths(&["C:\\Users".into()]).unwrap();
        s.push_recent_paths(&["c:\\users".into(), "D:\\Tmp".into()])
            .unwrap();
        // First entry wins its case; the second-case duplicate is dropped.
        let got = s.read_recent_paths();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].to_ascii_lowercase(), "c:\\users");
        assert_eq!(got[1], "D:\\Tmp");
        std::fs::remove_dir_all(base).ok();
    }
}
