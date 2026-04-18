// desktop/src-tauri/src/cmd/scan.rs
//
// Sequentially spawns duscan → dusum → dudb, emitting 'scan-progress' events
// at each stage transition. The DB produced by dudb replaces the currently
// open pool in AppState.

use crate::state::AppState;
use dutopia::db::open_pool;
use serde::Serialize;
use std::path::PathBuf;
use std::process::Stdio;
use tauri::{AppHandle, Emitter, State};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

#[derive(Serialize, Clone)]
pub struct ScanProgress {
    pub stage: String,
    pub percent: u8,
    pub message: Option<String>,
}

fn emit(app: &AppHandle, stage: &str, percent: u8, message: Option<String>) {
    let _ = app.emit(
        "scan-progress",
        ScanProgress {
            stage: stage.to_string(),
            percent,
            message,
        },
    );
}

/// Resolve the path to a bundled binary. Tauri's externalBin machinery
/// copies the `.exe` next to the main app binary (stripped of the
/// target-triple suffix).
fn bin_path(name: &str) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe
        .parent()
        .ok_or_else(|| "current exe has no parent".to_string())?;
    let ext = if cfg!(windows) { ".exe" } else { "" };
    let candidate = dir.join(format!("{name}{ext}"));
    if !candidate.exists() {
        return Err(format!(
            "binary {name} not found at {}",
            candidate.display()
        ));
    }
    Ok(candidate)
}

/// Spawn a child process and stream its stdout+stderr line-by-line through
/// `on_line`. Updates `state.scan_pid` so cancel_scan can kill it.
async fn run<F: FnMut(&str)>(
    app: &AppHandle,
    state: &AppState,
    program: PathBuf,
    args: Vec<String>,
    mut on_line: F,
) -> Result<(), String> {
    tracing::info!(program = %program.display(), args = ?args, "spawning");

    let mut cmd = Command::new(&program);
    cmd.args(&args);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    // On Windows, don't pop up a console window for the child.
    #[cfg(windows)]
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW

    let mut child = cmd.spawn().map_err(|e| {
        format!("failed to spawn {}: {}", program.display(), e)
    })?;

    if let Some(pid) = child.id() {
        *state.scan_pid.lock().unwrap() = Some(pid);
    }

    let stdout = child.stdout.take().ok_or("no stdout pipe")?;
    let stderr = child.stderr.take().ok_or("no stderr pipe")?;
    let mut out_lines = BufReader::new(stdout).lines();
    let mut err_lines = BufReader::new(stderr).lines();

    loop {
        tokio::select! {
            line = out_lines.next_line() => {
                match line {
                    Ok(Some(l)) => on_line(&l),
                    _ => break,
                }
            }
            line = err_lines.next_line() => {
                match line {
                    Ok(Some(l)) => on_line(&l),
                    _ => {}
                }
            }
        }
    }

    // Drain any remaining stderr lines after stdout closed.
    while let Ok(Some(l)) = err_lines.next_line().await {
        on_line(&l);
    }

    let status = child.wait().await.map_err(|e| e.to_string())?;
    *state.scan_pid.lock().unwrap() = None;

    if !status.success() {
        let _ = app.emit(
            "scan-progress",
            ScanProgress {
                stage: "".into(),
                percent: 0,
                message: Some(format!("{} failed: {}", program.display(), status)),
            },
        );
        return Err(format!("{} exited with {}", program.display(), status));
    }
    Ok(())
}

#[tauri::command]
pub async fn scan(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<String, String> {
    if paths.is_empty() {
        return Err("No paths provided".to_string());
    }
    let original_paths = paths.clone();

    let state_ref: &AppState = state.inner();
    let scan_csv = state_ref.scan_csv_path();
    let sum_csv = state_ref.sum_csv_path();
    let db_path = state_ref.db_path();

    // Drop the current pool so dudb can overwrite the DB file.
    {
        let mut guard = state_ref.db.write().await;
        *guard = None;
    }

    // Clean up any leftover intermediate files from a previous scan. If they
    // exist, appending a new scan's output to the same name would mix stale
    // data into the aggregation. Ignore errors — missing files are fine.
    for p in [&scan_csv, &sum_csv] {
        if p.exists() {
            if let Err(e) = std::fs::remove_file(p) {
                tracing::warn!(path = %p.display(), err = %e, "failed to remove stale CSV");
            }
        }
    }

    let duscan = bin_path("duscan")?;
    let dusum = bin_path("dusum")?;
    let dudb = bin_path("dudb")?;

    // Stage 1: duscan
    emit(&app, "scan", 5, Some(format!("Scanning {}…", paths.join(", "))));
    let mut duscan_args: Vec<String> =
        vec!["-o".into(), scan_csv.display().to_string(), "--quiet".into()];
    duscan_args.extend(paths.into_iter());
    run(&app, state_ref, duscan, duscan_args, |line| {
        if !line.is_empty() {
            emit(&app, "scan", 15, Some(line.to_string()));
        }
    })
    .await?;

    // Stage 2: dusum
    emit(&app, "sum", 45, Some("Aggregating…".into()));
    run(
        &app,
        state_ref,
        dusum,
        vec![
            scan_csv.display().to_string(),
            "-o".into(),
            sum_csv.display().to_string(),
        ],
        |line| {
            if !line.is_empty() {
                emit(&app, "sum", 60, Some(line.to_string()));
            }
        },
    )
    .await?;

    // Stage 3: dudb --rebuild (always replaces per product decision).
    // Pre-delete the DB files ourselves — on Windows, r2d2's pool-drop may
    // not release file handles synchronously, and dudb's own remove_file
    // ignores errors, so the rebuild could silently no-op and hit a UNIQUE
    // violation on re-ingest.
    for suffix in ["", "-wal", "-shm"] {
        let p = PathBuf::from(format!("{}{}", db_path.display(), suffix));
        if p.exists() {
            if let Err(e) = std::fs::remove_file(&p) {
                tracing::warn!(path = %p.display(), err = %e, "failed to remove old DB file; dudb will retry");
            }
        }
    }
    emit(&app, "index", 80, Some("Building index…".into()));
    run(
        &app,
        state_ref,
        dudb,
        vec![
            sum_csv.display().to_string(),
            "-o".into(),
            db_path.display().to_string(),
            "--rebuild".into(),
        ],
        |line| {
            if !line.is_empty() {
                emit(&app, "index", 92, Some(line.to_string()));
            }
        },
    )
    .await?;

    // Open the fresh DB.
    let pool = tokio::task::spawn_blocking({
        let db_path = db_path.clone();
        move || open_pool(&db_path)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    *state_ref.db.write().await = Some(pool);
    tracing::info!(path = %db_path.display(), "scan done — db pool reopened");

    // Merge the scanned paths into the MRU list.
    if let Err(e) = state_ref.push_recent_paths(&original_paths) {
        tracing::warn!(err = %e, "failed to persist recent paths");
    }

    emit(&app, "index", 100, Some("Done".into()));
    Ok(db_path.display().to_string())
}

#[tauri::command]
pub fn get_recent_paths(state: State<'_, AppState>) -> Vec<String> {
    state.read_recent_paths()
}

#[tauri::command]
pub fn set_recent_paths(state: State<'_, AppState>, paths: Vec<String>) -> Result<(), String> {
    state.write_recent_paths(&paths).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cancel_scan(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let pid = state.scan_pid.lock().unwrap().take();
    if let Some(pid) = pid {
        kill_pid(pid);
    }
    let _ = app.emit(
        "scan-progress",
        ScanProgress {
            stage: "".into(),
            percent: 0,
            message: Some("Cancelled".into()),
        },
    );
    Ok(())
}

#[cfg(windows)]
fn kill_pid(pid: u32) {
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F", "/T"])
        .status();
}

#[cfg(not(windows))]
fn kill_pid(pid: u32) {
    let _ = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status();
}
