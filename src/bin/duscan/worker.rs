// rs/src/bin/duscan/worker.rs
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::Arc;

use crossbeam::channel::{Receiver, Sender};
use zstd::stream::write::Encoder as ZstdEncoder;

use dutopia::util::{get_hostname, should_skip};

use crate::csv::{write_row_bin, write_row_csv};
use crate::merge::OutputFormat;
use crate::row::{row_from_metadata, stat_row};

const FILE_CHUNK: usize = 2048;
const FLUSH_BYTES: usize = 4 * 1024 * 1024;

#[derive(Default)]
pub struct Progress {
    pub files: AtomicU64,
}

#[derive(Debug)]
pub struct FileItem {
    pub name: OsString,
    pub md: fs::Metadata,
}

pub enum Task {
    Dir(PathBuf),
    Files {
        base: Arc<PathBuf>,
        items: Vec<FileItem>,
    },
    Shutdown,
}

impl std::fmt::Debug for Task {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Task::Dir(p) => f.debug_tuple("Dir").field(p).finish(),
            Task::Files { base, items } => f
                .debug_struct("Files")
                .field("base", base)
                .field("items", items)
                .finish(),
            Task::Shutdown => write!(f, "Shutdown"),
        }
    }
}

#[derive(Default)]
pub struct Stats {
    pub files: u64,
    pub errors: u64,
    pub bytes: u64,
}

#[derive(Clone)]
pub struct Config {
    pub skip: Option<String>,
    pub out_fmt: OutputFormat,
    pub no_atime: bool,
    pub progress: Option<Arc<Progress>>,
    pub pid: u32,
    pub verbose: u8,
}

pub fn worker(
    tid: usize,
    rx: Receiver<Task>,
    tx: Sender<Task>,
    inflight: Arc<AtomicUsize>,
    out_dir: PathBuf,
    cfg: Config,
) -> Stats {
    let is_bin = cfg.out_fmt == OutputFormat::Bin;
    let hostname = get_hostname();
    let pid = cfg.pid;
    let shard_path = out_dir.join(format!("shard_{hostname}_{pid}_{tid}.tmp"));
    let file = match File::create(&shard_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!(
                "FATAL: cannot create shard file {}: {}",
                shard_path.display(),
                e
            );
            let mut stats = Stats::default();
            stats.errors += 1;
            return stats;
        }
    };
    let base = BufWriter::with_capacity(32 * 1024 * 1024, file);
    let has_progress = cfg.progress.is_some();
    let progress = cfg.progress.unwrap_or_default();
    let verbose = cfg.verbose;

    let mut writer: Box<dyn Write + Send> = if is_bin {
        let enc = match ZstdEncoder::new(base, 1) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("FATAL: cannot create zstd encoder: {}", e);
                let mut stats = Stats::default();
                stats.errors += 1;
                return stats;
            }
        };
        Box::new(enc.auto_finish())
    } else {
        Box::new(base)
    };

    let mut buf: Vec<u8> = Vec::with_capacity(32 * 1024 * 1024);
    let mut stats = Stats::default();

    while let Ok(task) = rx.recv() {
        match task {
            Task::Shutdown => break,

            Task::Dir(dir) => {
                let mut error_count = 0u64;
                if should_skip(&dir, cfg.skip.as_deref()) {
                    let _ = inflight.fetch_sub(1, Relaxed);
                    continue;
                }

                if verbose >= 2 {
                    eprintln!("[{:>2}] Processing {}", tid, dir.display());
                }

                if let Some(row) = stat_row(&dir) {
                    if is_bin {
                        write_row_bin(&mut buf, &dir, &row, cfg.no_atime);
                    } else {
                        write_row_csv(&mut buf, &dir, &row, cfg.no_atime);
                    }
                    stats.files += 1;
                } else {
                    stats.errors += 1;
                    error_count += 1;
                    if verbose >= 1 {
                        eprintln!("ERROR: Failed to stat directory: {}", dir.display());
                    }
                }

                if buf.len() >= FLUSH_BYTES {
                    if let Err(e) = writer.write_all(&buf) {
                        if verbose >= 1 {
                            eprintln!("ERROR: write failed: {}", e);
                        }
                        stats.errors += 1;
                    }
                    buf.clear();
                }

                error_count += enum_dir(&dir, &tx, &inflight, cfg.skip.as_deref(), verbose);
                stats.errors += error_count;
                inflight.fetch_sub(1, Relaxed);
                if has_progress {
                    progress.files.fetch_add(1, Relaxed);
                }
            }

            Task::Files { base, items } => {
                if should_skip(base.as_ref(), cfg.skip.as_deref()) {
                    inflight.fetch_sub(1, Relaxed);
                    continue;
                }
                let mut files = 0u64;

                for FileItem { name, md } in &items {
                    let full = base.join(name);

                    if verbose >= 2 {
                        eprintln!("[{:>2}] Processing {}", tid, full.display());
                    }

                    let row = row_from_metadata(md);
                    if is_bin {
                        write_row_bin(&mut buf, &full, &row, cfg.no_atime);
                    } else {
                        write_row_csv(&mut buf, &full, &row, cfg.no_atime);
                    }
                    stats.files += 1;
                    stats.bytes += row.blocks * 512;
                    files += 1;
                    if buf.len() >= FLUSH_BYTES {
                        if let Err(e) = writer.write_all(&buf) {
                            if verbose >= 1 {
                                eprintln!("ERROR: write failed: {}", e);
                            }
                            stats.errors += 1;
                        }
                        buf.clear();
                    }
                }
                inflight.fetch_sub(1, Relaxed);
                if has_progress {
                    progress.files.fetch_add(files, Relaxed);
                }
            }
        }
    }

    if !buf.is_empty() {
        if let Err(e) = writer.write_all(&buf) {
            if verbose >= 1 {
                eprintln!("ERROR: final write failed: {}", e);
            }
            stats.errors += 1;
        }
    }
    if let Err(e) = writer.flush() {
        if verbose >= 1 {
            eprintln!("ERROR: flush failed: {}", e);
        }
        stats.errors += 1;
    }

    stats
}

pub fn enum_dir(
    dir: &Path,
    tx: &Sender<Task>,
    inflight: &AtomicUsize,
    skip: Option<&str>,
    verbose: u8,
) -> u64 {
    let rd = match fs::read_dir(dir) {
        Ok(it) => it,
        Err(e) => {
            if verbose >= 1 {
                eprintln!("ERROR: {}: {}", dir.display(), e);
            }
            return 1;
        }
    };
    let mut error_count: u64 = 0;
    let mut page: Vec<FileItem> = Vec::with_capacity(FILE_CHUNK);
    let base_arc = Arc::new(dir.to_path_buf());

    for dent in rd {
        let dent = match dent {
            Ok(d) => d,
            Err(e) => {
                error_count += 1;
                if verbose >= 1 {
                    eprintln!("ERROR: entry in {}: {}", dir.display(), e);
                }
                continue;
            }
        };
        let name = dent.file_name();
        if name == OsStr::new(".") || name == OsStr::new("..") {
            continue;
        }

        let ft = match dent.file_type() {
            Ok(ft) => ft,
            Err(e) => {
                error_count += 1;
                if verbose >= 1 {
                    eprintln!("ERROR: {}: {}", dent.path().display(), e);
                }
                continue;
            }
        };

        if ft.is_dir() {
            let p = dent.path();
            if should_skip(&p, skip) {
                continue;
            }
            inflight.fetch_add(1, Relaxed);
            let _ = tx.send(Task::Dir(p));
        } else {
            let md = if ft.is_symlink() {
                match fs::symlink_metadata(dent.path()) {
                    Ok(m) => m,
                    Err(e) => {
                        error_count += 1;
                        if verbose >= 1 {
                            eprintln!("ERROR: {}: {}", dent.path().display(), e);
                        }
                        continue;
                    }
                }
            } else {
                match dent.metadata() {
                    Ok(m) => m,
                    Err(e) => {
                        error_count += 1;
                        if verbose >= 1 {
                            eprintln!("ERROR: {}: {}", dent.path().display(), e);
                        }
                        continue;
                    }
                }
            };

            page.push(FileItem { name, md });
            if page.len() == FILE_CHUNK {
                inflight.fetch_add(1, Relaxed);
                let _ = tx.send(Task::Files {
                    base: base_arc.clone(),
                    items: std::mem::take(&mut page),
                });
            }
        }
    }

    if !page.is_empty() {
        inflight.fetch_add(1, Relaxed);
        let _ = tx.send(Task::Files {
            base: base_arc,
            items: page,
        });
    }

    error_count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam::channel::unbounded;
    use dutopia::util::Row;
    use tempfile::tempdir;

    #[test]
    fn test_should_skip() {
        let p = PathBuf::from("/a/b/c/d");
        assert!(should_skip(&p, Some("b/c")));
        assert!(!should_skip(&p, Some("x")));
        assert!(!should_skip(&p, None));
    }

    #[test]
    fn test_enum_dir_with_files_and_dirs() {
        let tmp = tempdir().unwrap();
        let test_dir = tmp.path();

        fs::write(test_dir.join("file1.txt"), "content1").unwrap();
        fs::write(test_dir.join("file2.txt"), "content2").unwrap();
        fs::create_dir(test_dir.join("subdir")).unwrap();
        fs::write(test_dir.join("subdir").join("file3.txt"), "content3").unwrap();

        let (tx, rx) = unbounded();
        let inflight = Arc::new(AtomicUsize::new(0));

        let error_count = enum_dir(test_dir, &tx, &inflight, None, 0);

        assert_eq!(error_count, 0);

        let mut dir_tasks = 0;
        let mut file_tasks = 0;

        drop(tx);
        while let Ok(task) = rx.recv() {
            match task {
                Task::Dir(_) => dir_tasks += 1,
                Task::Files { items, .. } => file_tasks += items.len(),
                Task::Shutdown => break,
            }
        }

        assert!(dir_tasks >= 1);
        assert!(file_tasks >= 2);
    }

    #[test]
    fn test_enum_dir_with_skip() {
        let tmp = tempdir().unwrap();
        let test_dir = tmp.path();

        fs::create_dir(test_dir.join("skip_me")).unwrap();
        fs::create_dir(test_dir.join("keep_me")).unwrap();

        let (tx, rx) = unbounded();
        let inflight = Arc::new(AtomicUsize::new(0));

        let error_count = enum_dir(test_dir, &tx, &inflight, Some("skip_me"), 0);
        assert_eq!(error_count, 0);

        drop(tx);
        let mut found_skip = false;
        let mut found_keep = false;

        while let Ok(task) = rx.recv() {
            if let Task::Dir(path) = task {
                if path.file_name().unwrap() == "skip_me" {
                    found_skip = true;
                }
                if path.file_name().unwrap() == "keep_me" {
                    found_keep = true;
                }
            }
        }

        assert!(!found_skip);
        assert!(found_keep);
    }

    #[test]
    fn test_enum_dir_nonexistent() {
        let nonexistent = Path::new("/nonexistent/directory");
        let (tx, _rx) = unbounded();
        let inflight = Arc::new(AtomicUsize::new(0));

        let error_count = enum_dir(nonexistent, &tx, &inflight, None, 0);
        assert_eq!(error_count, 1);
    }

    #[test]
    fn test_enum_dir_with_chunking() {
        let tmp = tempdir().unwrap();
        let test_dir = tmp.path();

        for i in 0..(FILE_CHUNK + 10) {
            fs::write(test_dir.join(format!("file{}.txt", i)), "content").unwrap();
        }

        let (tx, rx) = unbounded();
        let inflight = Arc::new(AtomicUsize::new(0));

        let error_count = enum_dir(test_dir, &tx, &inflight, None, 0);
        assert_eq!(error_count, 0);

        drop(tx);

        let mut total_files = 0;
        let mut task_count = 0;

        while let Ok(task) = rx.recv() {
            if let Task::Files { items, .. } = task {
                total_files += items.len();
                task_count += 1;
            }
        }

        assert_eq!(total_files, FILE_CHUNK + 10);
        assert!(task_count > 1);
    }

    #[test]
    fn test_progress_default() {
        let progress = Progress::default();
        assert_eq!(progress.files.load(Relaxed), 0);
    }

    #[test]
    fn test_config_clone() {
        let progress = Arc::new(Progress::default());
        let config = Config {
            skip: Some("test".to_string()),
            out_fmt: OutputFormat::Csv,
            no_atime: true,
            progress: Some(progress.clone()),
            pid: 123,
            verbose: 0,
        };

        let cloned = config.clone();
        assert_eq!(cloned.skip, Some("test".to_string()));
        assert_eq!(cloned.out_fmt, OutputFormat::Csv);
        assert!(cloned.no_atime);
        assert_eq!(cloned.pid, 123);
        assert!(cloned.progress.is_some());
    }

    #[test]
    fn test_stats_default() {
        let stats = Stats::default();
        assert_eq!(stats.files, 0);
        assert_eq!(stats.errors, 0);
        assert_eq!(stats.bytes, 0);
    }

    #[test]
    fn test_file_item_debug() {
        let tmp = tempdir().unwrap();
        let test_file = tmp.path().join("test.txt");
        fs::write(&test_file, "content").unwrap();
        let metadata = fs::metadata(&test_file).unwrap();

        let item = FileItem {
            name: "test.txt".into(),
            md: metadata,
        };

        let debug_str = format!("{:?}", item);
        assert!(debug_str.contains("test.txt"));
    }

    #[test]
    fn test_task_debug() {
        let dir_task = Task::Dir("/test/path".into());
        let debug_str = format!("{:?}", dir_task);
        assert!(debug_str.contains("Dir"));
        assert!(debug_str.contains("test/path"));

        let shutdown_task = Task::Shutdown;
        let debug_str = format!("{:?}", shutdown_task);
        assert!(debug_str.contains("Shutdown"));

        let files_task = Task::Files {
            base: Arc::new("/base".into()),
            items: vec![],
        };
        let debug_str = format!("{:?}", files_task);
        assert!(debug_str.contains("Files"));
    }

    #[cfg(unix)]
    #[test]
    fn test_enum_dir_with_symlinks() {
        let tmp = tempdir().unwrap();
        let test_dir = tmp.path();

        let target_file = test_dir.join("target.txt");
        fs::write(&target_file, "target content").unwrap();

        let symlink_path = test_dir.join("link.txt");
        std::os::unix::fs::symlink(&target_file, &symlink_path).unwrap();

        let (tx, rx) = unbounded();
        let inflight = Arc::new(AtomicUsize::new(0));

        let error_count = enum_dir(test_dir, &tx, &inflight, None, 0);
        assert_eq!(error_count, 0);

        drop(tx);

        let mut found_files = 0;
        while let Ok(task) = rx.recv() {
            if let Task::Files { items, .. } = task {
                found_files += items.len();
            }
        }

        assert_eq!(found_files, 2);
    }

    #[test]
    fn test_enum_dir_skips_dot_files() {
        let tmp = tempdir().unwrap();
        let test_dir = tmp.path();

        fs::write(test_dir.join("regular.txt"), "content").unwrap();
        fs::write(test_dir.join(".hidden"), "hidden").unwrap();

        let (tx, rx) = unbounded();
        let inflight = Arc::new(AtomicUsize::new(0));

        let error_count = enum_dir(test_dir, &tx, &inflight, None, 0);
        assert_eq!(error_count, 0);

        drop(tx);

        let mut found_files = Vec::new();
        while let Ok(task) = rx.recv() {
            if let Task::Files { items, .. } = task {
                for item in items {
                    found_files.push(item.name.to_string_lossy().to_string());
                }
            }
        }

        assert!(found_files.contains(&"regular.txt".to_string()));
        assert!(found_files.contains(&".hidden".to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn test_enum_dir_permission_errors() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempdir().unwrap();
        let test_dir = tmp.path().join("no_read");
        fs::create_dir(&test_dir).unwrap();

        fs::write(test_dir.join("file.txt"), "content").unwrap();

        let mut perms = fs::metadata(&test_dir).unwrap().permissions();
        perms.set_mode(0o000);
        fs::set_permissions(&test_dir, perms).unwrap();

        let (tx, _rx) = unbounded();
        let inflight = Arc::new(AtomicUsize::new(0));

        let error_count = enum_dir(&test_dir, &tx, &inflight, None, 0);

        let mut perms = fs::metadata(&test_dir).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&test_dir, perms).unwrap();

        assert_eq!(error_count, 1);
    }

    #[test]
    fn test_config_variations() {
        let cfg1 = Config {
            skip: None,
            out_fmt: OutputFormat::Bin,
            no_atime: false,
            progress: None,
            pid: 1,
            verbose: 0,
        };

        let cfg2 = cfg1.clone();
        assert_eq!(cfg1.out_fmt, cfg2.out_fmt);
        assert_eq!(cfg1.no_atime, cfg2.no_atime);
        assert_eq!(cfg1.pid, cfg2.pid);
    }

    #[test]
    fn test_worker_simple() {
        let tmp = tempdir().unwrap();
        let test_file = tmp.path().join("test.txt");
        fs::write(&test_file, "content").unwrap();

        let (tx, rx) = unbounded();
        let inflight = Arc::new(AtomicUsize::new(0));
        let progress = Arc::new(Progress::default());

        let cfg = Config {
            skip: None,
            out_fmt: OutputFormat::Csv,
            no_atime: false,
            progress: Some(progress.clone()),
            pid: 12345,
            verbose: 0,
        };

        let metadata = fs::metadata(&test_file).unwrap();
        let file_item = FileItem {
            name: "test.txt".into(),
            md: metadata,
        };

        tx.send(Task::Files {
            base: Arc::new(tmp.path().to_path_buf()),
            items: vec![file_item],
        })
        .unwrap();

        tx.send(Task::Shutdown).unwrap();
        drop(tx);

        let (dummy_tx, _) = unbounded();

        let out_dir = tmp.path().to_path_buf();
        let stats = worker(0, rx, dummy_tx, inflight, out_dir, cfg);

        assert_eq!(stats.files, 1);
        assert_eq!(stats.errors, 0);
        assert!(progress.files.load(Relaxed) >= 1);
    }

    #[test]
    fn test_worker_with_binary_output() {
        let tmp = tempdir().unwrap();
        let test_file = tmp.path().join("test.txt");
        fs::write(&test_file, "test content").unwrap();

        let (tx, rx) = unbounded();
        let inflight = Arc::new(AtomicUsize::new(0));

        let cfg = Config {
            skip: None,
            out_fmt: OutputFormat::Bin,
            no_atime: true,
            progress: None,
            pid: 12345,
            verbose: 0,
        };

        let metadata = fs::metadata(&test_file).unwrap();
        let file_item = FileItem {
            name: "test.txt".into(),
            md: metadata,
        };

        tx.send(Task::Files {
            base: Arc::new(tmp.path().to_path_buf()),
            items: vec![file_item],
        })
        .unwrap();

        tx.send(Task::Shutdown).unwrap();
        drop(tx);

        let (dummy_tx, _) = unbounded();
        let out_dir = tmp.path().to_path_buf();
        let stats = worker(0, rx, dummy_tx, inflight, out_dir, cfg);

        assert_eq!(stats.files, 1);
        assert_eq!(stats.errors, 0);
        assert!(stats.bytes > 0);
    }

    #[test]
    fn test_worker_with_skip_pattern() {
        let tmp = tempdir().unwrap();
        let skip_dir = tmp.path().join("skip_this");
        fs::create_dir(&skip_dir).unwrap();

        let (tx, rx) = unbounded();
        let inflight = Arc::new(AtomicUsize::new(0));

        let cfg = Config {
            skip: Some("skip_this".to_string()),
            out_fmt: OutputFormat::Csv,
            no_atime: false,
            progress: None,
            pid: 12345,
            verbose: 0,
        };

        tx.send(Task::Dir(skip_dir)).unwrap();
        tx.send(Task::Shutdown).unwrap();
        drop(tx);

        let (dummy_tx, _) = unbounded();
        let out_dir = tmp.path().to_path_buf();
        let stats = worker(0, rx, dummy_tx, inflight, out_dir, cfg);

        assert_eq!(stats.files, 0);
    }

    #[test]
    fn test_worker_with_files_task_skip() {
        let tmp = tempdir().unwrap();
        let skip_base = tmp.path().join("skip_this");
        fs::create_dir(&skip_base).unwrap();
        let test_file = skip_base.join("test.txt");
        fs::write(&test_file, "content").unwrap();

        let (tx, rx) = unbounded();
        let inflight = Arc::new(AtomicUsize::new(0));

        let cfg = Config {
            skip: Some("skip_this".to_string()),
            out_fmt: OutputFormat::Csv,
            no_atime: false,
            progress: None,
            pid: 12345,
            verbose: 0,
        };

        let metadata = fs::metadata(&test_file).unwrap();
        let file_item = FileItem {
            name: "test.txt".into(),
            md: metadata,
        };

        tx.send(Task::Files {
            base: Arc::new(skip_base),
            items: vec![file_item],
        })
        .unwrap();

        tx.send(Task::Shutdown).unwrap();
        drop(tx);

        let (dummy_tx, _) = unbounded();
        let out_dir = tmp.path().to_path_buf();
        let stats = worker(0, rx, dummy_tx, inflight, out_dir, cfg);

        assert_eq!(stats.files, 0);
    }

    #[test]
    fn test_worker_stat_row_failure() {
        let tmp = tempdir().unwrap();

        let (tx, rx) = unbounded();
        let inflight = Arc::new(AtomicUsize::new(0));

        let cfg = Config {
            skip: None,
            out_fmt: OutputFormat::Csv,
            no_atime: false,
            progress: None,
            pid: 12345,
            verbose: 0,
        };

        let nonexistent = tmp.path().join("nonexistent");
        tx.send(Task::Dir(nonexistent)).unwrap();
        tx.send(Task::Shutdown).unwrap();
        drop(tx);

        let (dummy_tx, _) = unbounded();
        let out_dir = tmp.path().to_path_buf();
        let stats = worker(0, rx, dummy_tx, inflight, out_dir, cfg);

        assert_eq!(stats.files, 0);
        assert!(stats.errors >= 1);
    }

    #[test]
    fn test_constants() {
        assert_eq!(FILE_CHUNK, 2048);
        assert_eq!(FLUSH_BYTES, 4 * 1024 * 1024);
    }

    #[test]
    fn test_large_buffer_flush() {
        use crate::csv::write_row_csv;

        let mut buf = Vec::new();
        let path = Path::new("test");
        let row = Row {
            dev: 1,
            ino: 2,
            mode: 755,
            uid: 1000,
            gid: 1000,
            size: 1024,
            blocks: 2,
            atime: 1234567890,
            mtime: 1234567891,
        };

        while buf.len() < FLUSH_BYTES + 1000 {
            write_row_csv(&mut buf, path, &row, false);
        }

        assert!(buf.len() > FLUSH_BYTES);
    }

    #[test]
    fn test_integration_csv_and_binary() {
        let tmp = tempdir().unwrap();
        let test_dir = tmp.path().join("integration_test");
        fs::create_dir(&test_dir).unwrap();

        fs::write(test_dir.join("file1.txt"), "content1").unwrap();
        fs::write(test_dir.join("file_with_spaces.txt"), "content with spaces").unwrap();
        fs::write(test_dir.join("file_with_underscores.txt"), "quoted content").unwrap();

        let subdir = test_dir.join("subdir");
        fs::create_dir(&subdir).unwrap();
        fs::write(subdir.join("nested.txt"), "nested content").unwrap();

        for &(output_format, no_atime) in &[
            (OutputFormat::Csv, false),
            (OutputFormat::Csv, true),
            (OutputFormat::Bin, false),
            (OutputFormat::Bin, true),
        ] {
            let (tx, rx) = unbounded();
            let inflight = Arc::new(AtomicUsize::new(0));
            let progress = Arc::new(Progress::default());

            let cfg = Config {
                skip: None,
                out_fmt: output_format,
                no_atime,
                progress: Some(progress.clone()),
                pid: 98765,
                verbose: 0,
            };

            let files = [
                "file1.txt",
                "file_with_spaces.txt",
                "file_with_underscores.txt",
            ];
            for file_name in &files {
                let file_path = test_dir.join(file_name);
                if let Ok(metadata) = fs::metadata(&file_path) {
                    let file_item = FileItem {
                        name: (*file_name).into(),
                        md: metadata,
                    };

                    tx.send(Task::Files {
                        base: Arc::new(test_dir.clone()),
                        items: vec![file_item],
                    })
                    .unwrap();
                }
            }

            tx.send(Task::Shutdown).unwrap();
            drop(tx);

            let (dummy_tx, _) = unbounded();
            let out_dir = tmp.path().to_path_buf();
            let stats = worker(0, rx, dummy_tx, inflight, out_dir.clone(), cfg);

            assert!(stats.files >= 3);
            assert_eq!(stats.errors, 0);
            assert!(stats.bytes > 0);
            assert!(progress.files.load(Relaxed) >= 3);

            let shard_path = out_dir.join(format!("shard_{}_{}_0.tmp", get_hostname(), 98765));
            if shard_path.exists() {
                let _ = fs::remove_file(shard_path);
            }
        }
    }
}
