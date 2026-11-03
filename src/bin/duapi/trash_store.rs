// rs/src/bin/duapi/trash_store.rs
//
// The trash's two JSON files in $TRASH_DIR (default /data/trash, the PVC):
//   queue.json    entries waiting for the nightly cron (trash.rs)
//   history.json  what happened to each one: claimed, moved, skipped, error,
//                 kept (trash_history.rs)
// Both are guarded by one process-wide mutex and replaced by tmp + rename,
// so a claim, a report or a Keep updates them together or not at all.

use anyhow::Context;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{de::DeserializeOwned, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::trash::Entry;
use crate::trash_history::Outcome;

pub const QUEUE_FILE: &str = "queue.json";
pub const HISTORY_FILE: &str = "history.json";

pub fn trash_dir() -> PathBuf {
    std::env::var("TRASH_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/data/trash"))
}

/// Load the queue (missing file = empty).
pub fn load(dir: &Path) -> anyhow::Result<Vec<Entry>> {
    load_json(dir, QUEUE_FILE)
}

/// Replace the queue atomically (write tmp, fsync, rename).
#[cfg(test)]
pub fn save(dir: &Path, entries: &[Entry]) -> anyhow::Result<()> {
    write_json(dir, QUEUE_FILE, entries, true)
}

/// Load a JSON array from `dir/name` (missing file = empty).
fn load_json<T: DeserializeOwned>(dir: &Path, name: &str) -> anyhow::Result<Vec<T>> {
    let file = dir.join(name);
    match std::fs::read_to_string(&file) {
        Ok(text) => serde_json::from_str(&text)
            .with_context(|| format!("parsing {}", file.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", file.display())),
    }
}

/// The history is a log: when it cannot be parsed, set it aside as
/// history.json.corrupt-<unix secs> and start a new one, rather than failing
/// every claim, report and Keep from then on. (A corrupt queue still fails:
/// it holds work nobody else has a copy of.)
fn load_history(dir: &Path) -> anyhow::Result<Vec<Outcome>> {
    match load_json(dir, HISTORY_FILE) {
        Ok(h) => Ok(h),
        Err(e) if e.chain().any(|c| c.is::<serde_json::Error>()) => {
            let aside = dir.join(format!("{HISTORY_FILE}.corrupt-{}", chrono::Utc::now().timestamp()));
            tracing::error!(err = %format!("{e:#}"), aside = %aside.display(), "trash history unreadable, set aside");
            std::fs::rename(dir.join(HISTORY_FILE), &aside)
                .with_context(|| format!("setting aside {}", aside.display()))?;
            Ok(Vec::new())
        }
        Err(e) => Err(e),
    }
}

/// Replace `dir/name` atomically (write tmp, fsync, rename). The queue is
/// pretty (people read it); the history, up to 50k rows, is compact.
fn write_json<T: Serialize>(dir: &Path, name: &str, items: &[T], pretty: bool) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let file = dir.join(name);
    let tmp = dir.join(format!("{name}.tmp"));
    {
        let mut f = std::fs::File::create(&tmp)
            .with_context(|| format!("creating {}", tmp.display()))?;
        if pretty {
            serde_json::to_writer_pretty(&mut f, items)?;
        } else {
            serde_json::to_writer(&mut f, items)?;
        }
        f.write_all(b"\n")?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &file)
        .with_context(|| format!("renaming {} to {}", tmp.display(), file.display()))
}

/// One writer at a time on the trash files (one duapi pod).
static LOCK: Mutex<()> = Mutex::new(());

/// What a `with_both` closure changed, so only those files are rewritten.
#[derive(Default)]
pub struct Changed {
    pub queue: bool,
    pub history: bool,
}

/// Run `f` on the loaded queue under the lock, in a blocking task; save the
/// queue when `f` reports a change. IO errors become a 500 response.
pub async fn with_queue<T, F>(f: F) -> Result<T, Response>
where
    F: FnOnce(&mut Vec<Entry>) -> (T, bool) + Send + 'static,
    T: Send + 'static,
{
    with_both(move |q, _h| {
        let (out, changed) = f(q);
        (out, Changed { queue: changed, history: false })
    })
    .await
}

/// `with_queue` over the queue and the history together.
pub async fn with_both<T, F>(f: F) -> Result<T, Response>
where
    F: FnOnce(&mut Vec<Entry>, &mut Vec<Outcome>) -> (T, Changed) + Send + 'static,
    T: Send + 'static,
{
    let dir = trash_dir();
    let res = tokio::task::spawn_blocking(move || -> anyhow::Result<T> {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut queue = load(&dir)?;
        let mut history = load_history(&dir)?;
        let (out, changed) = f(&mut queue, &mut history);
        if changed.history {
            write_json(&dir, HISTORY_FILE, &history, false)?;
        }
        if changed.queue {
            write_json(&dir, QUEUE_FILE, &queue, true)?;
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
