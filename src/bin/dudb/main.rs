// rs/src/bin/dudb/main.rs
use anyhow::{Context, Result};
use clap::{ColorChoice, Parser};
use colored::Colorize;
use dutopia::util::{parse_age_pair, print_about, AgeCfg};
use rusqlite::Connection;
use std::path::PathBuf;

mod ingest;
mod ingest_raw;
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
    /// Raw duscan CSV: also ingest per-file rows so duapi serves /api/files
    /// from the DB instead of the live filesystem
    #[arg(long, value_name = "FILE")]
    raw: Option<PathBuf>,
    /// Age buckets in days as YOUNG,OLD for per-file rows; must match the
    /// values dusum ran with (defaults to 60,600)
    #[arg(long, value_parser = parse_age_pair, value_name = "YOUNG,OLD")]
    age: Option<(i64, i64)>,
    /// Overwrite an existing DB instead of failing
    #[arg(long)]
    rebuild: bool,
}

fn main() -> Result<()> {
    print_about();
    let args = Args::parse();

    if !args.input.exists() {
        eprintln!(
            "{}",
            format!("Error: input CSV not found: {}", args.input.display()).red()
        );
        std::process::exit(1);
    }
    if let Some(raw) = args.raw.as_deref().filter(|r| !r.exists()) {
        eprintln!(
            "{}",
            format!("Error: raw CSV not found: {}", raw.display()).red()
        );
        std::process::exit(1);
    }

    let output = args.output.clone().unwrap_or_else(|| {
        let stem = args
            .input
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output");
        PathBuf::from(format!("{}.db", stem))
    });

    if output.exists() {
        if !args.rebuild {
            eprintln!(
                "{}",
                format!(
                    "Error: {} already exists. Use --rebuild to overwrite.",
                    output.display()
                )
                .red()
            );
            std::process::exit(1);
        }
        remove_db_files(&output)?;
    }

    let started = std::time::Instant::now();

    print!("Counting lines... ");
    std::io::Write::flush(&mut std::io::stdout()).ok();
    let total_lines = ingest::count_lines(&args.input)?;
    let data_lines = total_lines.saturating_sub(1);
    println!("done");
    println!("Total lines  : {}", total_lines);

    let mut conn = Connection::open(&output)
        .with_context(|| format!("opening {}", output.display()))?;
    schema::apply_ingest_pragmas(&conn)?;
    schema::create_tables(&conn)?;

    println!("Loading CSV into SQLite...");
    let (stats, path_cache) =
        ingest::ingest_csv(&mut conn, &args.input, data_lines, |processed| {
            let pct = ((processed as f64 / data_lines.max(1) as f64) * 100.0).round() as u32;
            println!("{}%", pct.min(100));
        })?;

    let mut file_stats = None;
    if let Some(raw) = &args.raw {
        let age_cfg = AgeCfg::from_args(&args.age);
        print!("Counting raw lines... ");
        std::io::Write::flush(&mut std::io::stdout()).ok();
        let raw_lines = ingest::count_lines(raw)?.saturating_sub(1);
        println!("done ({} lines)", raw_lines);
        println!("Loading raw CSV into SQLite (files)...");
        let fs = ingest_raw::ingest_raw_csv(&conn, raw, &path_cache, age_cfg, raw_lines, |p| {
            let pct = ((p as f64 / raw_lines.max(1) as f64) * 100.0).round() as u32;
            println!("{}%", pct.min(100));
        })?;
        let raw_mtime = file_mtime(raw);
        schema::write_files_metadata(
            &conn,
            &raw.to_string_lossy(),
            raw_mtime,
            fs.files_inserted,
            age_cfg,
        )?;
        file_stats = Some(fs);
    }

    println!("Building indexes...");
    schema::create_indexes(&conn)?;
    println!("Running ANALYZE...");
    conn.execute_batch("ANALYZE;")?;

    let source_mtime = file_mtime(&args.input);
    schema::write_metadata(
        &conn,
        &args.input.to_string_lossy(),
        source_mtime,
        stats.rows_inserted,
    )?;

    let elapsed = started.elapsed();
    println!("Output       : {}", output.display());
    println!("Stats rows   : {}", stats.rows_inserted);
    println!("Paths        : {}", stats.paths_inserted);
    println!("Users        : {}", stats.users_inserted);
    if let Some(fs) = &file_stats {
        println!("File rows    : {}", fs.files_inserted);
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
    println!("Elapsed time : {:.2} seconds", elapsed.as_secs_f64());
    Ok(())
}

fn file_mtime(path: &std::path::Path) -> i64 {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn remove_db_files(db: &PathBuf) -> Result<()> {
    let _ = std::fs::remove_file(db);
    for sibling in [
        format!("{}-wal", db.display()),
        format!("{}-shm", db.display()),
    ] {
        let _ = std::fs::remove_file(&sibling);
    }
    Ok(())
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
}
