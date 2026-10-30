// rs/src/bin/dudb/main.rs
use anyhow::{Context, Result};
use clap::{ColorChoice, Parser};
use colored::Colorize;
use dutopia::util::{parse_age_pair, parse_file_hint, print_about, AgeCfg};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::time::Instant;

mod extsort;
mod folder_pack;
use folder_pack::ListRule;
mod ingest;
mod ingest_raw;
mod raw_path;
mod schema;

#[derive(Parser, Debug)]
#[command(
    version,
    color = ColorChoice::Auto,
    about = "Build SQLite index from a dusum-aggregated CSV"
)]
struct Args {
    /// Input dusum CSV file path
    input: PathBuf,
    /// Output SQLite DB file path (defaults to <input_stem>.db)
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Raw duscan CSV: also index each folder's files (totals plus its
    /// largest files) so duapi serves /api/files from the DB instead of the
    /// live filesystem
    #[arg(long, value_name = "FILE", conflicts_with = "no_files")]
    raw: Option<PathBuf>,
    /// Folders only, on purpose: duapi lists no files at all (instead of
    /// reading the live filesystem, as it does for a DB built without --raw)
    #[arg(long)]
    no_files: bool,
    /// Age buckets in days as YOUNG,OLD for per-file rows; must match the
    /// values dusum ran with (defaults to 60,600)
    #[arg(long, value_parser = parse_age_pair, value_name = "YOUNG,OLD")]
    age: Option<(i64, i64)>,
    /// Scratch directory for the --raw sort runs (local disk; needs roughly
    /// 1/4 of the raw CSV size free). Defaults to the system temp dir
    #[arg(long, value_name = "DIR", env = "DUDB_TMPDIR")]
    tmp_dir: Option<PathBuf>,
    /// Memory for the --raw sort in MiB (two buffers of half each)
    #[arg(long, value_name = "MIB", default_value_t = 8192)]
    sort_mem: usize,
    /// --raw lists only the N largest files per folder, owner and age
    /// bucket; the rest only count in the folder totals (0 = no limit)
    #[arg(long, value_name = "N", default_value_t = 100)]
    max_files: usize,
    /// --raw lists only files of at least this size (e.g. 100k, 1M, 1G);
    /// smaller ones only count in the folder totals
    #[arg(long, value_name = "SIZE", default_value = "1M", value_parser = parse_size)]
    min_file_size: u64,
    /// Overwrite an existing DB instead of failing
    #[arg(long)]
    rebuild: bool,
}

fn main() -> Result<()> {
    print_about();
    let args = Args::parse();

    if !args.input.exists() {
        fail(&format!("input CSV not found: {}", args.input.display()));
    }
    if let Some(raw) = args.raw.as_deref().filter(|r| !r.exists()) {
        fail(&format!("raw CSV not found: {}", raw.display()));
    }

    let output = args.output.clone().unwrap_or_else(|| {
        let stem = args
            .input
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output");
        PathBuf::from(format!("{}.db", stem))
    });
    if output.exists() && !args.rebuild {
        fail(&format!(
            "{} already exists. Use --rebuild to overwrite.",
            output.display()
        ));
    }

    // Build next to the output and rename at the end: a reader of the
    // current DB (duapi on the share) keeps working until the new one is
    // complete, and a failed build leaves the old DB untouched.
    let building = sibling(&output, ".building");
    remove_db_files(&building)?;

    let started = Instant::now();
    let mut phase = Instant::now();

    print!("Counting lines... ");
    std::io::Write::flush(&mut std::io::stdout()).ok();
    let total_lines = ingest::count_lines(&args.input)?;
    let data_lines = total_lines.saturating_sub(1);
    println!("done");
    println!("Total lines  : {}", total_lines);

    let mut conn = Connection::open(&building)
        .with_context(|| format!("opening {}", building.display()))?;
    schema::apply_ingest_pragmas(&conn)?;
    schema::create_tables(&conn)?;

    println!("Loading CSV into SQLite...");
    let (stats, path_cache) =
        ingest::ingest_csv(&mut conn, &args.input, data_lines, percent(data_lines as u64))?;
    done(&mut phase);

    let mut file_stats = None;
    if let Some(raw) = &args.raw {
        let age_cfg = AgeCfg::from_args(&args.age);
        print!("Counting raw lines... ");
        std::io::Write::flush(&mut std::io::stdout()).ok();
        let raw_lines = ingest::count_lines(raw)?.saturating_sub(1);
        println!("done ({} lines)", raw_lines);

        let opts = ingest_raw::SortOpts {
            tmp_dir: args.tmp_dir.clone().unwrap_or_else(std::env::temp_dir),
            run_bytes: (args.sort_mem.max(64) << 20) / 2,
        };
        println!("Sorting raw CSV (1/2) in {} ...", opts.tmp_dir.display());
        let sorted = ingest_raw::sort_raw_csv(
            &conn,
            raw,
            &path_cache,
            age_cfg,
            &opts,
            raw_lines,
            percent(raw_lines as u64),
        )?;
        drop(path_cache);
        println!(
            "Sorted {} rows into {} runs ({:.1} GiB in {})",
            sorted.rows(),
            sorted.run_count(),
            sorted.disk_bytes() as f64 / (1u64 << 30) as f64,
            sorted.dir().display()
        );
        done(&mut phase);

        println!("Packing files per folder (2/2)...");
        let total = sorted.rows();
        let rule = ListRule { cap: args.max_files, min_size: args.min_file_size };
        let fs = ingest_raw::load_sorted_runs(&conn, sorted, rule, percent(total))?;
        let raw_csv = raw.to_string_lossy();
        schema::write_files_metadata(
            &conn,
            &schema::FilesMeta {
                raw_csv: &raw_csv,
                raw_mtime: file_mtime(raw),
                total: fs.files_seen,
                listed: fs.files_listed,
                cap: args.max_files,
                min_size: args.min_file_size,
                age_cfg,
            },
        )?;
        done(&mut phase);
        file_stats = Some(fs);
    }

    if args.no_files {
        schema::write_no_files_metadata(&conn)?;
    }

    println!("Building indexes...");
    schema::create_indexes(&conn)?;
    schema::write_metadata(
        &conn,
        &args.input.to_string_lossy(),
        file_mtime(&args.input),
        stats.rows_inserted,
    )?;
    println!("Running ANALYZE...");
    schema::finalize(&conn)?;
    drop(conn);
    replace_db(&building, &output)?;
    done(&mut phase);

    println!("Output       : {}", output.display());
    println!("Stats rows   : {}", stats.rows_inserted);
    println!("Paths        : {}", stats.paths_inserted);
    println!("Users        : {}", stats.users_inserted);
    if let Some(fs) = &file_stats {
        println!("Files        : {} in {} folders", fs.files_seen, fs.folders);
        println!(
            "Files listed : {} (largest {} >= {} bytes per folder, owner and age)",
            fs.files_listed, args.max_files, args.min_file_size
        );
        if fs.skipped_non_regular > 0 {
            println!("Non-regular  : {} (skipped)", fs.skipped_non_regular);
        }
        if fs.unmatched_folders > 0 {
            println!("Unmatched    : {} (skipped)", fs.unmatched_folders);
        }
        if fs.duplicates > 0 {
            println!("Duplicates   : {} (skipped)", fs.duplicates);
        }
    }
    println!("Elapsed time : {:.2} seconds", started.elapsed().as_secs_f64());
    Ok(())
}

fn parse_size(s: &str) -> Result<u64, String> {
    parse_file_hint(s).ok_or_else(|| format!("invalid size {s:?} (e.g. 0, 100k, 1M, 1G)"))
}

fn fail(msg: &str) -> ! {
    eprintln!("{}", format!("Error: {}", msg).red());
    std::process::exit(1);
}

/// Progress printer: one line per whole percent, never repeated.
fn percent(total: u64) -> impl FnMut(u64) {
    let mut last = None;
    move |n| {
        let pct = (n as f64 / total.max(1) as f64 * 100.0).round().min(100.0) as u32;
        if last != Some(pct) {
            println!("{}%", pct);
            last = Some(pct);
        }
    }
}

fn done(phase: &mut Instant) {
    println!("  ... {:.1}s", phase.elapsed().as_secs_f64());
    *phase = Instant::now();
}

fn file_mtime(path: &Path) -> i64 {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn sibling(db: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{}", db.display(), suffix))
}

fn remove_db_files(db: &Path) -> Result<()> {
    let _ = std::fs::remove_file(db);
    for s in ["-wal", "-shm", "-journal"] {
        let _ = std::fs::remove_file(sibling(db, s));
    }
    Ok(())
}

/// Swap the finished build into place. Sidecars of the old DB go first: a
/// stale `-journal` next to the new file would be replayed into it.
fn replace_db(building: &Path, output: &Path) -> Result<()> {
    for s in ["-wal", "-shm", "-journal"] {
        let _ = std::fs::remove_file(sibling(output, s));
    }
    std::fs::rename(building, output)
        .with_context(|| format!("renaming {} to {}", building.display(), output.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_db_files_handles_missing() {
        let p = PathBuf::from("does_not_exist_dudb_test.db");
        // Should not error even if files don't exist.
        remove_db_files(&p).unwrap();
    }

    #[test]
    fn replace_db_swaps_file_and_drops_old_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("x.db");
        let building = sibling(&out, ".building");
        std::fs::write(&out, b"old").unwrap();
        std::fs::write(sibling(&out, "-journal"), b"stale").unwrap();
        std::fs::write(&building, b"new").unwrap();
        replace_db(&building, &out).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"new");
        assert!(!building.exists());
        assert!(!sibling(&out, "-journal").exists());
    }
}
