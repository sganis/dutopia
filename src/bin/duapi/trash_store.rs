// rs/src/bin/duapi/trash_store.rs
//
// The trash queue: `$TRASH_DIR/queue.json` (default /data/trash, the PVC).
// It holds only what waits for the nightly cron: a claim takes items out,
// Keep takes them out, so it stays small. One process-wide mutex; replaced
// by tmp + fsync + rename, so a change lands whole or not at all.

use anyhow::Context;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::trash::Entry;

pub const QUEUE_FILE: &str = "queue.json";

pub fn trash_dir() -> PathBuf {
    std::env::var("TRASH_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/data/trash"))
}

/// Load the queue (missing file = empty).
pub fn load(dir: &Path) -> anyhow::Result<Vec<Entry>> {
    let file = dir.join(QUEUE_FILE);
    match std::fs::read_to_string(&file) {
        Ok(text) => serde_json::from_str(&text)
            .with_context(|| format!("parsing {}", file.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", file.display())),
    }
}

/// Replace the queue atomically (write tmp, fsync, rename).
pub fn save(dir: &Path, entries: &[Entry]) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let file = dir.join(QUEUE_FILE);
    let tmp = dir.join(format!("{QUEUE_FILE}.tmp"));
    {
        let mut f = std::fs::File::create(&tmp)
            .with_context(|| format!("creating {}", tmp.display()))?;
        serde_json::to_writer_pretty(&mut f, entries)?;
        f.write_all(b"\n")?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &file)
        .with_context(|| format!("renaming {} to {}", tmp.display(), file.display()))
}

/// One writer at a time on the queue (one duapi pod).
static LOCK: Mutex<()> = Mutex::new(());

/// Run `f` on the loaded queue under the lock, in a blocking task; save the
/// queue when `f` reports a change. IO errors become a 500 response.
pub async fn with_queue<T, F>(f: F) -> Result<T, Response>
where
    F: FnOnce(&mut Vec<Entry>) -> (T, bool) + Send + 'static,
    T: Send + 'static,
{
    let dir = trash_dir();
    let res = tokio::task::spawn_blocking(move || -> anyhow::Result<T> {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut queue = load(&dir)?;
        let (out, changed) = f(&mut queue);
        if changed {
            save(&dir, &queue)?;
        }
        Ok(out)
    })
    .await;
    match res {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => {
            tracing::error!(err = %format!("{e:#}"), "500 trash queue error");
            Err((StatusCode::INTERNAL_SERVER_ERROR, format!("trash queue error: {e:#}"))
                .into_response())
        }
        Err(join_err) => {
            tracing::error!(err = %join_err, "500 Task Join Error /api/trash");
            Err((StatusCode::INTERNAL_SERVER_ERROR, "task error").into_response())
        }
    }
}
