// rs/src/bin/duscan/main.rs
use anyhow::{Context, Result};
use std::fs::{self, File};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use chrono::Local;
use clap::{ColorChoice, Parser};
use colored::Colorize;
use crossbeam::channel::unbounded;

use dutopia::util::{
    format_duration, get_hostname, human_bytes, human_count, parse_file_hint, print_about,
    progress_bar, strip_verbatim_prefix,
};

mod csv;
mod merge;
mod row;
mod worker;

use merge::{merge_shards, OutputFormat};
use worker::{worker, Config, Progress, Stats, Task};

#[derive(Parser, Debug)]
#[command(
    version,
    author,
    color = ColorChoice::Auto,
    about = "Scan filesystem and gather file metadata into CSV or binary output"
)]
struct Args {
    /// Folders to scan (required, one or more)
    folders: Vec<String>,
    /// Output path (default: folder.csv or folder.zst if --bin)
    #[arg(short, long, value_name = "PATH")]
    output: Option<PathBuf>,
    /// Number of worker (default: 2xCPU, capped to 48)
    #[arg(short, long, value_name = "N")]
    workers: Option<usize>,
    /// Skip any folder whose full path contains this substring
    #[arg(short, long, value_name = "SUBSTR")]
    skip: Option<String>,
    /// Write a binary .zst compressed file instead of .csv
    #[arg(short, long)]
    bin: bool,
    /// Zero the ATIME field in outputs (CSV & BIN) for testing
    #[arg(long = "no-atime")]
    no_atime: bool,
    /// Total files hint (e.g. 750m, 1.2b). Used for % progress
    #[arg(long = "files-hint", value_name = "N")]
    files_hint: Option<String>,
    /// Do not report progress
    #[arg(short, long)]
    quiet: bool,
    /// Verbose output: print errors (-v) or errors and paths (-vv)
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

fn main() -> Result<()> {
    print_about();

    let args = Args::parse();

    if args.folders.is_empty() {
        anyhow::bail!("At least one folder must be specified");
    }

    let out_fmt = if args.bin {
        OutputFormat::Bin
    } else {
        OutputFormat::Csv
    };

    if args.no_atime {
        eprintln!(
            "{}",
            "ATIME will be written as 0 and lines sorted for reproducible output.".yellow()
        );
    }

    // Canonicalize all root folders
    let mut roots = Vec::new();
    for folder in &args.folders {
        let root = fs::canonicalize(folder)
            .with_context(|| format!("Failed to canonicalize folder: {}", folder))?;
        roots.push(root);
    }

    // Create a combined name for default output
    let combined_name = if roots.len() == 1 {
        let root_normalized = strip_verbatim_prefix(&roots[0]);
        #[cfg(windows)]
        {
            root_normalized
                .to_string_lossy()
                .replace('\\', "-")
                .replace(':', "")
        }
        #[cfg(not(windows))]
        {
            root_normalized
                .to_string_lossy()
                .trim_start_matches('/')
                .replace('/', "-")
        }
    } else {
        format!("stats_{}", roots.len())
    };

    // Decide default output by out_fmt
    let final_path: PathBuf = match args.output {
        Some(p) => {
            if p.is_absolute() {
                p
            } else {
                std::env::current_dir()?.join(p)
            }
        }
        None => {
            let ext = match out_fmt {
                OutputFormat::Csv => "csv",
                OutputFormat::Bin => "zst",
            };
            std::env::current_dir()?.join(format!("{combined_name}.{ext}"))
        }
    };

    // Ensure the output directory exists and is writable
    let out_dir: PathBuf = final_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or(std::env::current_dir()?);

    if !out_dir.exists() {
        anyhow::bail!("Output directory does not exist: {}", out_dir.display());
    }

    if !out_dir.is_dir() {
        anyhow::bail!("Output path is not a directory: {}", out_dir.display());
    }

    // Check write access by trying to create a temp file
    let testfile = out_dir.join(".dutopia_write_test");
    File::create(&testfile)
        .with_context(|| format!("No write access to directory {}", out_dir.display()))?;
    let _ = fs::remove_file(&testfile);

    let workers = args
        .workers
        .unwrap_or_else(|| (num_cpus::get() * 2).max(4).min(48));
    let cmd: Vec<String> = std::env::args().collect();
    let now = Local::now();
    let hostname = get_hostname();
    let pid = std::process::id();

    println!(
        "Local time   : {}",
        now.format("%Y-%m-%d %H:%M:%S").to_string()
    );
    println!("Host         : {}", hostname);
    println!("Process ID   : {}", pid);
    println!("Command      : {}", cmd.join(" "));

    for (i, root) in roots.iter().enumerate() {
        let root_normalized = strip_verbatim_prefix(root);
        println!("Input {}      : {}", i + 1, root_normalized.display());
    }

    println!("Output       : {}", &final_path.display());
    println!("Temp dir     : {}", out_dir.display());
    println!("Workers      : {}", workers);

    if args.verbose > 0 {
        println!("Verbose      : Level {}", args.verbose);
    }

    // ---- work queue + inflight counter ----
    let (tx, rx) = unbounded::<Task>();
    let inflight = Arc::new(AtomicUsize::new(0));

    let progress = Arc::new(Progress::default());
    let reporting_done = Arc::new(AtomicBool::new(false));
    let mut reporter_join: Option<JoinHandle<()>> = None;

    if !args.quiet {
        let hinted_files = args
            .files_hint
            .as_deref()
            .and_then(|s| parse_file_hint(s));

        if let Some(total_files) = hinted_files {
            println!(
                "Files hint   : {} (from --files-hint)",
                human_count(total_files)
            );
        }

        let progress_for_reporter = progress.clone();
        let reporting_done = reporting_done.clone();
        let start_for_reporter = Instant::now();

        reporter_join = Some(thread::spawn(move || {
            let mut last_pct = 0.0;
            loop {
                if reporting_done.load(Relaxed) {
                    break;
                }
                let f = progress_for_reporter.files.load(Relaxed);
                let elapsed = start_for_reporter.elapsed().as_secs_f64().max(0.001);
                let rate_f = human_count((f as f64 / elapsed) as u64);

                if let Some(total) = hinted_files {
                    let mut pct = ((f as f64 / total as f64) * 100.0).min(100.0);
                    if pct < last_pct {
                        pct = last_pct;
                    }
                    last_pct = pct;
                    let bar = progress_bar(pct.into(), 25);
                    eprint!(
                        "\r    {} {} {:>3}% | {} files [{} f/s]        \r",
                        "Progress".bright_cyan(),
                        bar,
                        pct as u32,
                        human_count(f),
                        rate_f
                    );
                } else {
                    eprint!(
                        "\r    {} : {} files [{} f/s]        \r",
                        "Progress".bright_cyan(),
                        human_count(f),
                        rate_f
                    );
                }
                thread::sleep(Duration::from_millis(1000));
            }
            eprint!("\r{}", " ".repeat(120));
        }));
    }

    let start_time = Instant::now();

    // seed all root folders
    for root in roots {
        inflight.fetch_add(1, Relaxed);
        tx.send(Task::Dir(root)).expect("enqueue root");
    }

    // shutdown detection with stronger memory ordering and double-check
    {
        let tx = tx.clone();
        let inflight = inflight.clone();
        thread::spawn(move || {
            let mut consecutive_zeros = 0;
            loop {
                let current_inflight = inflight.load(std::sync::atomic::Ordering::SeqCst);
                if current_inflight == 0 {
                    consecutive_zeros += 1;
                    if consecutive_zeros >= 5 {
                        for _ in 0..workers {
                            let _ = tx.send(Task::Shutdown);
                        }
                        break;
                    }
                } else {
                    consecutive_zeros = 0;
                }
                thread::sleep(std::time::Duration::from_millis(100));
            }
        });
    }

    let cfg = Config {
        skip: args.skip,
        out_fmt,
        no_atime: args.no_atime,
        progress: (!args.quiet).then(|| progress.clone()),
        pid,
        verbose: args.verbose,
    };

    // ---- spawn workers ----
    let mut joins = Vec::with_capacity(workers);
    for tid in 0..workers {
        let rx = rx.clone();
        let tx = tx.clone();
        let inflight = inflight.clone();
        let out_dir = out_dir.clone();
        let cfg = cfg.clone();
        joins.push(thread::spawn(move || {
            worker(tid, rx, tx, inflight, out_dir, cfg)
        }));
    }
    drop(tx);

    // ---- gather stats ----
    let mut total = Stats::default();
    for j in joins {
        match j.join() {
            Ok(s) => {
                total.files += s.files;
                total.errors += s.errors;
                total.bytes += s.bytes;
            }
            Err(_) => {
                eprintln!("{}", "Error: a worker thread panicked".red());
                total.errors += 1;
            }
        }
    }
    // measure speed before merging
    let elapsed = start_time.elapsed().as_secs_f64().max(0.001);
    let speed = ((total.files as f64) / elapsed) as u32;

    // ---- merge shards ----
    let sort_csv = args.no_atime && matches!(out_fmt, OutputFormat::Csv);
    merge_shards(&out_dir, &final_path, workers, out_fmt, sort_csv, pid)?;

    if let Some(h) = reporter_join.take() {
        reporting_done.store(true, Relaxed);
        let _ = h.join();
    }

    let elapsed_str = format_duration(start_time.elapsed());

    println!("\rTotal files  : {}", total.files);
    println!("Total errors : {}", total.errors);
    println!("Total disk   : {}", human_bytes(total.bytes));
    println!("Elapsed time : {}", elapsed_str);
    println!("Files/s      : {:.2}", speed);
    println!("{}", "-".repeat(44).bright_cyan());
    println!("Done.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_args_debug() {
        let args = Args {
            folders: vec!["folder1".to_string(), "folder2".to_string()],
            output: Some("output.csv".into()),
            workers: Some(8),
            skip: Some("skip_pattern".to_string()),
            bin: false,
            no_atime: true,
            files_hint: Some("1000".to_string()),
            quiet: false,
            verbose: 0,
        };

        let debug_str = format!("{:?}", args);
        assert!(debug_str.contains("folder1"));
        assert!(debug_str.contains("output.csv"));
        assert!(debug_str.contains("skip_pattern"));
    }
}
