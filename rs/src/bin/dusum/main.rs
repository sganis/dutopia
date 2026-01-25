// rs/src/bin/dusum/main.rs
use anyhow::Result;
use chrono::Utc;
use clap::{ColorChoice, Parser};
use csv::{ReaderBuilder, Trim};
use dutopia::util::{parse_int, print_about};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

mod aggregate;
mod output;
mod stats;

use aggregate::{get_folder_ancestors, resolve_user};
use output::{count_lines, write_results, write_unknown_uids};
use stats::{age_bucket, parse_age_pair, sanitize_mtime, AgeCfg, UserStats};

// POSIX-style type masks as encoded by dutopia in MODE
#[cfg(unix)]
const S_IFMT: u32 = 0o170000;
#[cfg(unix)]
const S_IFDIR: u32 = 0o040000;

#[cfg(not(unix))]
const S_IFMT: u32 = 0o170000;
#[cfg(not(unix))]
const S_IFDIR: u32 = 0o040000;

#[derive(Parser, Debug)]
#[command(
    version,
    color = ColorChoice::Auto,
    about = "Compute summary statistics from CSV input"
)]
struct Args {
    /// Input CSV file path
    input: PathBuf,
    /// Output CSV file path (defaults to <input_stem>.sum.csv)
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Age buckets in days as YOUNG,OLD  (defaults to 60,600)
    #[arg(long, value_parser = parse_age_pair, value_name = "YOUNG,OLD")]
    age: Option<(i64, i64)>,
}

fn main() -> Result<()> {
    print_about();

    let start_time = std::time::Instant::now();
    let args = Args::parse();

    let age_cfg = AgeCfg::from_args(&args.age);
    println!(
        "Age (days)   : recent < {}, not too old < {}, old > {}",
        age_cfg.young, age_cfg.old, age_cfg.old
    );

    let output_path = args.output.clone().unwrap_or_else(|| {
        let stem = args
            .input
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output");
        PathBuf::from(format!("{}.sum.csv", stem))
    });

    let unk_path = {
        let stem = args
            .input
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output");
        PathBuf::from(format!("{}.unk.csv", stem))
    };

    let mut user_cache: HashMap<u32, String> = HashMap::new();
    let mut unk_uids: HashSet<u32> = HashSet::new();

    let total_lines = count_lines(&args.input)?;
    let data_lines = total_lines.saturating_sub(1);
    println!("Total lines  : {}", total_lines);

    let mut reader = ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .trim(Trim::None)
        .from_path(&args.input)?;

    let mut aggregated_data: HashMap<(Vec<u8>, String, u8), UserStats> = HashMap::new();
    let progress_interval = if data_lines >= 10 {
        data_lines / 10
    } else {
        0
    };

    let now_ts = Utc::now().timestamp();
    let mut seen_inodes: HashSet<Vec<u8>> = HashSet::new();

    for (index, record_result) in reader.byte_records().enumerate() {
        let record = match record_result {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Warning: Skipping malformed row {}: {}", index + 1, e);
                continue;
            }
        };

        let inode_bytes = record.get(0).unwrap_or(b"").to_vec();
        let mode = parse_int::<u32>(record.get(5));
        let is_dir = (mode & S_IFMT) == S_IFDIR;
        let raw_atime = parse_int::<i64>(record.get(1));
        let raw_mtime = parse_int::<i64>(record.get(2));
        let sanitized_atime = if is_dir {
            0
        } else {
            sanitize_mtime(now_ts, raw_atime)
        };
        let sanitized_mtime = sanitize_mtime(now_ts, raw_mtime);
        let uid = parse_int::<u32>(record.get(3));
        let user = resolve_user(uid, &mut user_cache);
        if user == "UNK" {
            unk_uids.insert(uid);
        }
        let file_size = parse_int::<u64>(record.get(6));
        let raw_disk = parse_int::<u64>(record.get(7));

        let (disk_size, linked_size) = if seen_inodes.insert(inode_bytes) {
            (raw_disk, 0)
        } else {
            (0, raw_disk)
        };

        let path_bytes = record.get(8).unwrap_or(b"");

        if user.is_empty() || path_bytes.is_empty() {
            continue;
        }

        let bucket = age_bucket(now_ts, sanitized_mtime, age_cfg);

        for folder_path in get_folder_ancestors(path_bytes) {
            let key = (folder_path, user.clone(), bucket);
            aggregated_data.entry(key).or_default().update(
                file_size,
                disk_size,
                linked_size,
                sanitized_atime,
                sanitized_mtime,
            );
        }

        if progress_interval > 0 && (index + 1) % progress_interval == 0 {
            let percent = ((index + 1) as f64 * 100.0 / data_lines.max(1) as f64).ceil() as u32;
            println!("{}%", percent.min(100));
        }
    }

    write_results(&output_path, &aggregated_data)?;
    write_unknown_uids(&unk_path, &unk_uids)?;

    let duration = start_time.elapsed();
    println!("Output       : {}", output_path.display());
    println!(
        "Unknown UIDs : {} (total: {})",
        unk_path.display(),
        unk_uids.len()
    );
    let percent_unique = if data_lines > 0 {
        ((aggregated_data.len() as f64 / data_lines as f64) * 100.0) as i32
    } else {
        0
    };
    println!(
        "Total lines  : {} ({}% of input)",
        aggregated_data.len(),
        percent_unique
    );
    println!("Elapsed time : {:.2} seconds", duration.as_secs_f64());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_trims_and_defaults() {
        assert_eq!(parse_int::<u32>(Some(b" 42 ")), 42);
        assert_eq!(parse_int::<u32>(Some(b"-1")), 0);
        assert_eq!(parse_int::<u32>(Some(b"4294967296")), 0);
        assert_eq!(parse_int::<u32>(Some(b"  100 ")), 100);
        assert_eq!(parse_int::<i64>(Some(b" +7 ")), 7);
        assert_eq!(parse_int::<i64>(Some(b" foo ")), 0);
        assert_eq!(parse_int::<u32>(None), 0);
    }
}
