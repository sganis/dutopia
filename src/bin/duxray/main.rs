// src/bin/duxray/main.rs
//
// Storage X-ray: one-pass statistics over a duscan raw CSV
// (INODE,ATIME,MTIME,UID,GID,MODE,SIZE,DISK,PATH). Designed for simulation
// storage analysis: users generate huge numbers of files but only touch a
// tiny fraction — the goal is quantifying abandonment.
//
// Streams the CSV line by line (constant memory except a few capped maps)
// and reports size distribution (files AND bytes), age by mtime/atime/
// last-touched, abandonment by owner and project, sparse/junk files,
// permission red flags, path stats and top-N rankings.
//
// The Python prototype lived in the release dir; this is the Rust port.

use anyhow::{Context, Result};
use clap::{ColorChoice, Parser};
use std::path::PathBuf;
use std::time::Instant;

mod report;
mod stats;



#[derive(Parser, Debug)]
#[command(
    version,
    color = ColorChoice::Auto,
    about = "Storage X-ray: one-pass statistics over a duscan raw CSV (size, age, abandonment)"
)]
struct Args {
    /// Input CSV file path
    input: PathBuf,
    /// Read only first N data rows (0 = all)
    #[arg(long, default_value_t = 0)]
    limit: u64,
    /// Status line every N files (0 = never)
    #[arg(long, default_value_t = 10_000_000)]
    progress: u64,
    /// How many entries to show in top lists
    #[arg(short, long, default_value_t = 15)]
    top: usize,
    /// Dedup rows sharing an inode (hardlink analysis). Inode fields may be
    /// plain POSIX numbers or GPFS `fsid-inode` pairs; dedup is scoped per
    /// filesystem so combined CSVs are handled correctly. Costs ~16 bytes
    /// of RAM per regular file (~14 GB for 900M files) and roughly halves
    /// scan speed. Off by default.
    #[arg(long, default_value_t = false)]
    hardlinks: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let now = chrono::Utc::now().timestamp();
    let started = Instant::now();

    let (stats, header_ok) =
        stats::scan(&args.input, now, args.limit, args.progress, args.hardlinks)
        .with_context(|| format!("scanning {}", args.input.display()))?;
    if !header_ok {
        eprintln!("warning: unexpected header");
    }
    report::print_report(&stats, &args.input, now, args.top);
    eprintln!("elapsed: {:.1}s", started.elapsed().as_secs_f64());
    Ok(())
}
