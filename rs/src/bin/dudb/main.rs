// rs/src/bin/dudb/main.rs
use anyhow::{Context, Result};
use clap::{ColorChoice, Parser};
use colored::Colorize;
use dutopia::util::print_about;
use rusqlite::Connection;
use std::path::PathBuf;

mod ingest;
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
    let stats = ingest::ingest_csv(&mut conn, &args.input, data_lines, |processed| {
        let pct = ((processed as f64 / data_lines.max(1) as f64) * 100.0).round() as u32;
        println!("{}%", pct.min(100));
    })?;

    println!("Building indexes...");
    schema::create_indexes(&conn)?;
    println!("Running ANALYZE...");
    conn.execute_batch("ANALYZE;")?;

    let source_mtime = std::fs::metadata(&args.input)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
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
    println!("Elapsed time : {:.2} seconds", elapsed.as_secs_f64());
    Ok(())
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
