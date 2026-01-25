// rs/src/bin/duzip/main.rs
use anyhow::Result;
use clap::{ColorChoice, Parser};
use std::path::PathBuf;

use dutopia::util::print_about;

mod compress;
mod decompress;
mod record;

use compress::csv_to_zst;
use decompress::zst_to_csv;

#[derive(Parser, Debug)]
#[command(
    version,
    color = ColorChoice::Auto,
    about = "Convert between CSV and compressed binary (.zst) formats"
)]
struct Args {
    /// Input file (.zst or .csv)
    input: PathBuf,

    /// Output file path (default: auto-determined based on operation)
    #[arg(short, long)]
    output: Option<PathBuf>,
}

fn main() -> Result<()> {
    print_about();

    let args = Args::parse();

    let ext = args
        .input
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "csv" => csv_to_zst(&args.input, args.output.as_ref()),
        "zst" => zst_to_csv(&args.input, args.output.as_ref()),
        other => anyhow::bail!(
            "Unsupported input extension: '{}' (expected .csv, .bin, or .zst)",
            other
        ),
    }
}
