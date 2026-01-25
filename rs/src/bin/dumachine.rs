// rs/src/bin/dumachine.rs
use anyhow::{Context, Result};
use clap::{Parser, ColorChoice};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;
use chrono::{NaiveDate, TimeZone, Utc};
use dutopia::util::print_about;

const READ_BUF_SIZE: usize = 8 * 1024 * 1024;
const WRITE_BUF_SIZE: usize = 8 * 1024 * 1024;

#[derive(Parser, Debug)]
#[command(version, color = ColorChoice::Auto,
    about = "Convert DDN format to raw CSV format (inverse of duhuman)"
)]
struct Args {
    /// Input DDN file
    input: PathBuf,
    /// Output CSV (defaults to <stem>.raw.csv in the current directory)
    #[arg(short, long)]
    output: Option<PathBuf>,
}

const OUT_HEADER: &[u8] = b"INODE,ATIME,MTIME,UID,GID,MODE,SIZE,DISK,PATH\n";

fn main() -> Result<()> {
    print_about();

    let start = std::time::Instant::now();
    let args = Args::parse();
    let input = &args.input;

    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    let output = args
        .output
        .unwrap_or_else(|| PathBuf::from(format!("{}.raw.csv", stem)));

    let file = File::open(input)
        .with_context(|| format!("opening input file {}", input.display()))?;
    let mut reader = BufReader::with_capacity(READ_BUF_SIZE, file);

    let out_file = File::create(&output)
        .with_context(|| format!("creating output csv {}", output.display()))?;
    let mut writer = BufWriter::with_capacity(WRITE_BUF_SIZE, out_file);

    writer.write_all(OUT_HEADER)?;

    // Caches
    let mut time_cache: HashMap<[u8; 10], i64> = HashMap::new();
    let mut mode_cache: HashMap<[u8; 10], u32> = HashMap::new();

    // Buffers - reused per line
    let mut line_buf: Vec<u8> = Vec::with_capacity(8 * 1024);
    let mut out_buf: Vec<u8> = Vec::with_capacity(1024);

    let mut files: u64 = 0;
    let mut errors: u64 = 0;

    loop {
        line_buf.clear();
        let bytes_read = reader.read_until(b'\n', &mut line_buf)?;
        
        if bytes_read == 0 {
            break; // EOF
        }

        // Trim trailing \n and \r
        while line_buf.last() == Some(&b'\n') || line_buf.last() == Some(&b'\r') {
            line_buf.pop();
        }

        if line_buf.is_empty() {
            continue;
        }

        files += 1;

        match convert_line(&line_buf, &mut out_buf, &mut time_cache, &mut mode_cache) {
            Ok(_) => {
                writer.write_all(&out_buf)?;
                out_buf.clear();
            }
            Err(e) => {
                out_buf.clear();
                eprintln!("Line {}: {}", files, e);
                errors += 1;
            }
        }

        if files % 10_000_000 == 0 {
            eprintln!("Progress: {}M lines", files / 1_000_000);
        }
    }

    writer.flush()?;

    println!("Output       : {}", output.display());
    println!("Total files  : {}", files);
    println!("Total errors : {}", errors);
    println!("Elapsed time : {:.3} sec.", start.elapsed().as_secs_f64());
    Ok(())
}

/// Find the CSV portion after ";DIGITS!" or ";-DIGITS!" pattern
#[inline]
fn find_csv_start(line: &[u8]) -> Option<usize> {
    // Search backwards for the pattern - it's near the end
    let mut i = line.len().saturating_sub(1);
    
    while i > 0 {
        if line[i] == b'!' {
            // Check if preceded by digits (and optional minus after semicolon)
            let mut j = i - 1;
            while j > 0 && line[j].is_ascii_digit() {
                j -= 1;
            }
            // Check for optional minus sign
            if j > 0 && line[j] == b'-' {
                j -= 1;
            }
            if line[j] == b';' && j < i - 1 {
                return Some(i + 1);
            }
        }
        i -= 1;
    }
    None
}

#[inline]
fn convert_line(
    line: &[u8],
    out: &mut Vec<u8>,
    time_cache: &mut HashMap<[u8; 10], i64>,
    mode_cache: &mut HashMap<[u8; 10], u32>,
) -> Result<()> {
    let csv_start = find_csv_start(line)
        .context("no CSV data found")?;
    
    let csv_part = &line[csv_start..];
    
    // Parse CSV fields manually for speed
    // Format: INODE,ATIME,MTIME,UID,GID,PERM,SIZE,DISK,PATH
    let mut fields: [&[u8]; 9] = [&[]; 9];
    let mut field_idx = 0;
    let mut start = 0;
    let mut in_quotes = false;
    let mut i = 0;

    while i < csv_part.len() && field_idx < 9 {
        let b = csv_part[i];
        if b == b'"' {
            in_quotes = !in_quotes;
        } else if b == b',' && !in_quotes {
            fields[field_idx] = &csv_part[start..i];
            field_idx += 1;
            start = i + 1;
        }
        i += 1;
    }
    // Last field
    if field_idx < 9 {
        fields[field_idx] = &csv_part[start..];
        field_idx += 1;
    }

    if field_idx < 9 {
        anyhow::bail!("not enough fields: {}", field_idx);
    }

    let inode = fields[0];
    let accessed = unix_time(fields[1], time_cache)?;
    let modified = unix_time(fields[2], time_cache)?;
    let uid = fields[3];
    let gid = fields[4];
    let mode = perm_to_mode(fields[5], mode_cache)?;
    let size = fields[6];
    let disk_raw = parse_u64(fields[7]) * 512;

    // Write output directly to buffer
    out.extend_from_slice(inode);
    out.push(b',');
    push_i64(out, accessed);
    out.push(b',');
    push_i64(out, modified);
    out.push(b',');
    out.extend_from_slice(uid);
    out.push(b',');
    out.extend_from_slice(gid);
    out.push(b',');
    push_u32(out, mode);
    out.push(b',');
    out.extend_from_slice(size);
    out.push(b',');
    push_u64(out, disk_raw);
    out.push(b',');
    csv_quote_path(out, fields[8]);
    out.push(b'\n');

    Ok(())
}

#[inline]
fn unix_time(date_bytes: &[u8], cache: &mut HashMap<[u8; 10], i64>) -> Result<i64> {
    // Extract just the date part (first 10 bytes: YYYY-MM-DD)
    if date_bytes.len() < 10 {
        anyhow::bail!("date too short");
    }
    
    let mut key: [u8; 10] = [0; 10];
    key.copy_from_slice(&date_bytes[..10]);

    if let Some(&ts) = cache.get(&key) {
        return Ok(ts);
    }

    let date_str = std::str::from_utf8(&key)
        .context("invalid UTF-8 in date")?;
    
    let date = NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
        .with_context(|| format!("parsing date '{}'", date_str))?;

    let ts = Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
        .timestamp();

    cache.insert(key, ts);
    Ok(ts)
}

#[inline]
fn perm_to_mode(perm: &[u8], cache: &mut HashMap<[u8; 10], u32>) -> Result<u32> {
    if perm.len() != 10 {
        anyhow::bail!("permission string must be 10 bytes");
    }

    let mut key: [u8; 10] = [0; 10];
    key.copy_from_slice(perm);

    if let Some(&mode) = cache.get(&key) {
        return Ok(mode);
    }

    const S_IFDIR: u32  = 0o040000;
    const S_IFREG: u32  = 0o100000;
    const S_IFLNK: u32  = 0o120000;
    const S_IFCHR: u32  = 0o020000;
    const S_IFBLK: u32  = 0o060000;
    const S_IFIFO: u32  = 0o010000;
    const S_IFSOCK: u32 = 0o140000;

    let mut mode = match perm[0] {
        b'd' => S_IFDIR,
        b'-' => S_IFREG,
        b'l' => S_IFLNK,
        b'c' => S_IFCHR,
        b'b' => S_IFBLK,
        b'p' => S_IFIFO,
        b's' => S_IFSOCK,
        _ => anyhow::bail!("unknown file type: {}", perm[0] as char),
    };

    const PERM_BITS: [u32; 9] = [
        0o0400, 0o0200, 0o0100,
        0o0040, 0o0020, 0o0010,
        0o0004, 0o0002, 0o0001,
    ];

    for i in 0..9 {
        if perm[i + 1] != b'-' {
            mode |= PERM_BITS[i];
        }
    }

    cache.insert(key, mode);
    Ok(mode)
}

#[inline]
fn parse_u64(b: &[u8]) -> u64 {
    let mut n: u64 = 0;
    for &c in b {
        if c.is_ascii_digit() {
            n = n * 10 + (c - b'0') as u64;
        }
    }
    n
}

#[inline]
fn push_i64(buf: &mut Vec<u8>, n: i64) {
    let mut tmp = itoa::Buffer::new();
    buf.extend_from_slice(tmp.format(n).as_bytes());
}

#[inline]
fn push_u64(buf: &mut Vec<u8>, n: u64) {
    let mut tmp = itoa::Buffer::new();
    buf.extend_from_slice(tmp.format(n).as_bytes());
}

#[inline]
fn push_u32(buf: &mut Vec<u8>, n: u32) {
    let mut tmp = itoa::Buffer::new();
    buf.extend_from_slice(tmp.format(n).as_bytes());
}

/// Quote path for CSV output, handling quotes/commas/newlines
#[inline]
fn csv_quote_path(out: &mut Vec<u8>, path: &[u8]) {
    // If already quoted, pass through as-is (already properly CSV-escaped)
    if path.len() >= 2 && path[0] == b'"' && path[path.len()-1] == b'"' {
        out.extend_from_slice(path);
    } else {
        // Need to quote and escape
        out.push(b'"');
        for &b in path {
            if b == b'"' {
                out.push(b'"');
                out.push(b'"');
            } else {
                out.push(b);
            }
        }
        out.push(b'"');
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn caches() -> (HashMap<[u8; 10], i64>, HashMap<[u8; 10], u32>) {
        (HashMap::new(), HashMap::new())
    }

    // ========== find_csv_start tests ==========

    #[test]
    fn test_find_csv_start_basic() {
        let line = b"prefix;122!data,here";
        assert_eq!(find_csv_start(line), Some(11));
    }

    #[test]
    fn test_find_csv_start_negative() {
        let line = b"prefix;-101!data,here";
        assert_eq!(find_csv_start(line), Some(12));
    }

    #[test]
    fn test_find_csv_start_with_exclamation_in_name() {
        let line = b"test!name!with!bangs:6!system;122!data";
        assert_eq!(find_csv_start(line), Some(34));
    }

    // ========== perm_to_mode tests ==========

    #[test]
    fn test_perm_to_mode_directory_sr() {
        let (_, mut mc) = caches();
        assert_eq!(perm_to_mode(b"drwxr-sr-x", &mut mc).unwrap(), 16877);
    }

    #[test]
    fn test_perm_to_mode_directory_rws() {
        let (_, mut mc) = caches();
        assert_eq!(perm_to_mode(b"drwxrwsr-x", &mut mc).unwrap(), 16893);
    }

    #[test]
    fn test_perm_to_mode_sticky_t() {
        let (_, mut mc) = caches();
        assert_eq!(perm_to_mode(b"drwxr-s--T", &mut mc).unwrap(), 16873);
    }

    #[test]
    fn test_perm_to_mode_regular_file() {
        let (_, mut mc) = caches();
        assert_eq!(perm_to_mode(b"-rw-r--r--", &mut mc).unwrap(), 33188);
    }

    #[test]
    fn test_perm_to_mode_executable() {
        let (_, mut mc) = caches();
        assert_eq!(perm_to_mode(b"-rwxr-xr-x", &mut mc).unwrap(), 33261);
    }

    #[test]
    fn test_perm_to_mode_symlink() {
        let (_, mut mc) = caches();
        assert_eq!(perm_to_mode(b"lrwxrwxrwx", &mut mc).unwrap(), 41471);
    }

    #[test]
    fn test_perm_to_mode_cache() {
        let (_, mut mc) = caches();
        perm_to_mode(b"drwxr-xr-x", &mut mc).unwrap();
        assert_eq!(mc.len(), 1);
        perm_to_mode(b"drwxr-xr-x", &mut mc).unwrap();
        assert_eq!(mc.len(), 1);
    }

    // ========== unix_time tests ==========

    #[test]
    fn test_unix_time() {
        let (mut tc, _) = caches();
        assert_eq!(unix_time(b"2026-01-05 05:16:44.176298", &mut tc).unwrap(), 1767571200);
        assert_eq!(unix_time(b"2023-12-31 05:51:55.048507", &mut tc).unwrap(), 1703980800);
        assert_eq!(unix_time(b"2022-05-25 06:20:27.018751", &mut tc).unwrap(), 1653436800);
        assert_eq!(unix_time(b"2017-03-19 11:40:59.000000", &mut tc).unwrap(), 1489881600);
        assert_eq!(unix_time(b"2024-06-06 10:20:41.153388", &mut tc).unwrap(), 1717632000);
        assert_eq!(unix_time(b"2023-09-18 11:27:53.023686", &mut tc).unwrap(), 1694995200);
        assert_eq!(unix_time(b"2025-11-23 04:20:41.218196", &mut tc).unwrap(), 1763856000);
    }

    #[test]
    fn test_unix_time_cache() {
        let (mut tc, _) = caches();
        unix_time(b"2026-01-05 00:00:00", &mut tc).unwrap();
        assert_eq!(tc.len(), 1);
        unix_time(b"2026-01-05 12:00:00", &mut tc).unwrap();
        assert_eq!(tc.len(), 1);
    }

    // ========== csv_quote_path tests ==========

    #[test]
    fn test_csv_quote_path_simple() {
        let mut out = Vec::new();
        csv_quote_path(&mut out, b"\"/red/peddn10\"");
        assert_eq!(&out, b"\"/red/peddn10\"");
    }

    #[test]
    fn test_csv_quote_path_with_inner_quote() {
        let mut out = Vec::new();
        csv_quote_path(&mut out, b"\"path/with\"\"quote\"");
        assert_eq!(&out, b"\"path/with\"\"quote\"");
    }

    #[test]
    fn test_csv_quote_path_unquoted_input() {
        let mut out = Vec::new();
        csv_quote_path(&mut out, b"/simple/path");
        assert_eq!(&out, b"\"/simple/path\"");
    }

    // ========== convert_line tests ==========

    fn convert(line: &[u8]) -> String {
        let (mut tc, mut mc) = caches();
        let mut out = Vec::new();
        convert_line(line, &mut out, &mut tc, &mut mc).unwrap();
        String::from_utf8_lossy(&out).trim_end().to_string()
    }

    #[test]
    fn test_convert_line_1() {
        let line = br#"5001:000fffffffffffff:0000000000000003:1:100:c659b7:0:4000010:1!.:6!system;106!250609822-3,2026-01-05 05:16:44.176298,2025-11-23 04:20:41.218196,0,0,drwxr-sr-x,262144,512,"/red/peddn10""#;
        assert_eq!(convert(line), r#"250609822-3,1767571200,1763856000,0,0,16877,262144,262144,"/red/peddn10""#);
    }

    #[test]
    fn test_convert_line_2() {
        let line = br#"5001:000fffffffffffff:00000000000036b6:10002:0:c659b7:0:4000010:12!ABHD_restart:6!system;121!250609822-14006,2026-01-05 19:00:02.603416,2023-12-31 05:51:55.048507,0,976,drwxrwsr-x,4096,0,"/red/peddn10/ABHD_restart""#;
        assert_eq!(convert(line), r#"250609822-14006,1767571200,1703980800,0,976,16893,4096,0,"/red/peddn10/ABHD_restart""#);
    }

    #[test]
    fn test_convert_line_3() {
        let line = br#"5001:000fffffffffffff:00000000000036b7:10002:0:c659b7:0:4000010:12!ZMLH_simdata:6!system;126!250609822-14007,2026-01-05 19:00:02.602028,2022-05-25 06:20:27.018751,19121,2081,drwxrwsr-x,4096,0,"/red/peddn10/ZMLH_simdata""#;
        assert_eq!(convert(line), r#"250609822-14007,1767571200,1653436800,19121,2081,16893,4096,0,"/red/peddn10/ZMLH_simdata""#);
    }

    #[test]
    fn test_convert_line_4() {
        let line = br#"5001:000fffffffffffff:00000000000036b8:10002:0:c659b7:0:4000010:14!ABQQKF_restart:6!system;124!250609822-14008,2026-01-05 19:00:02.619190,2023-12-31 05:52:05.123526,0,1105,drwxrwsr-x,4096,0,"/red/peddn10/ABQQKF_restart""#;
        assert_eq!(convert(line), r#"250609822-14008,1767571200,1703980800,0,1105,16893,4096,0,"/red/peddn10/ABQQKF_restart""#);
    }

    #[test]
    fn test_convert_line_5() {
        let line = br#"5001:000fffffffffffff:00000000000036b9:10002:0:c659b7:0:4000010:16!BRRI_IRS_restart:6!system;126!250609822-14009,2026-01-05 03:10:03.792909,2023-12-31 05:52:35.913911,0,1102,drwxrwsr-x,4096,0,"/red/peddn10/BRRI_IRS_restart""#;
        assert_eq!(convert(line), r#"250609822-14009,1767571200,1703980800,0,1102,16893,4096,0,"/red/peddn10/BRRI_IRS_restart""#);
    }

    #[test]
    fn test_convert_line_6() {
        let line = br#"5001:000fffffffffffff:00000000000036ba:10002:0:c659b7:0:4000010:15!EVENT03_restart:6!system;125!250609822-14010,2026-01-05 19:00:02.626714,2023-12-31 05:53:36.589167,0,1231,drwxrwsr-x,4096,0,"/red/peddn10/EVENT03_restart""#;
        assert_eq!(convert(line), r#"250609822-14010,1767571200,1703980800,0,1231,16893,4096,0,"/red/peddn10/EVENT03_restart""#);
    }

    #[test]
    fn test_convert_line_7() {
        let line = br#"5001:000fffffffffffff:00000000000036bb:10002:0:c659b7:0:4000010:19!EVENTHRDHUN_restart:6!system;129!250609822-14011,2026-01-05 03:10:03.772983,2017-03-19 11:40:59.000000,0,1139,drwxrwsr-x,4096,0,"/red/peddn10/EVENTHRDHUN_restart""#;
        assert_eq!(convert(line), r#"250609822-14011,1767571200,1489881600,0,1139,16893,4096,0,"/red/peddn10/EVENTHRDHUN_restart""#);
    }

    #[test]
    fn test_convert_line_8_sticky() {
        let line = br#"5001:000fffffffffffff:00000000000036bc:10002:0:c659b7:0:4000010:24!GEOCONSISTENTGAS_restart:6!system;134!250609822-14012,2026-01-05 19:00:02.618861,2023-09-18 11:27:53.023686,0,1302,drwxr-s--T,4096,0,"/red/peddn10/GEOCONSISTENTGAS_restart""#;
        assert_eq!(convert(line), r#"250609822-14012,1767571200,1694995200,0,1302,16873,4096,0,"/red/peddn10/GEOCONSISTENTGAS_restart""#);
    }

    #[test]
    fn test_convert_line_9() {
        let line = br#"5001:000fffffffffffff:00000000000036bd:10002:0:c659b7:0:c000010:12!HRML_restart:6!system;119!250609822-14013,2026-01-05 19:00:02.604342,2024-06-06 10:20:41.153388,0,0,drwxr-sr-x,4096,0,"/red/peddn10/HRML_restart""#;
        assert_eq!(convert(line), r#"250609822-14013,1767571200,1717632000,0,0,16877,4096,0,"/red/peddn10/HRML_restart""#);
    }

    #[test]
    fn test_convert_line_10() {
        let line = br#"5001:000fffffffffffff:00000000000036be:10002:0:c659b7:0:c000010:15!HSBHGAS_restart:6!system;122!250609822-14014,2026-01-05 19:00:02.603189,2024-06-06 10:20:42.979394,0,0,drwxr-sr-x,4096,0,"/red/peddn10/HSBHGAS_restart""#;
        assert_eq!(convert(line), r#"250609822-14014,1767571200,1717632000,0,0,16877,4096,0,"/red/peddn10/HSBHGAS_restart""#);
    }

    // ========== Edge cases ==========

    #[test]
    fn test_convert_line_with_comma_in_filename() {
        let line = br#"5001:000fffffffffffff:00000000000036be:10002:0:c659b7:0:c000010:15!HSBHGAS_restart with comma, ok:6!system;122!250609822-14014,2026-01-05 19:00:02.603189,2024-06-06 10:20:42.979394,0,0,drwxr-sr-x,4096,0,"/red/peddn10/HSBHGAS_restart with comma, ok""#;
        assert_eq!(convert(line), r#"250609822-14014,1767571200,1717632000,0,0,16877,4096,0,"/red/peddn10/HSBHGAS_restart with comma, ok""#);
    }

    #[test]
    fn test_convert_line_with_exclamation_in_filename() {
        let line = br#"5001:000fffffffffffff:00000000000036be:10002:0:c659b7:0:c000010:15!HSBHGAS_restart with ! ok!!:6!system;122!250609822-14014,2026-01-05 19:00:02.603189,2024-06-06 10:20:42.979394,0,0,drwxr-sr-x,4096,0,"/red/peddn10/HSBHGAS_restart with ! ok!!""#;
        assert_eq!(convert(line), r#"250609822-14014,1767571200,1717632000,0,0,16877,4096,0,"/red/peddn10/HSBHGAS_restart with ! ok!!""#);
    }

    #[test]
    fn test_convert_line_with_negative_number_pattern() {
        let line = br#"5001:000fffffffffffff:00000000000036be:10002:0:c659b7:0:c000010:15!test:6!system;-101!250609822-14014,2026-01-05 19:00:02.603189,2024-06-06 10:20:42.979394,0,0,drwxr-sr-x,4096,0,"/red/peddn10/test""#;
        assert_eq!(convert(line), r#"250609822-14014,1767571200,1717632000,0,0,16877,4096,0,"/red/peddn10/test""#);
    }

    #[test]
    fn test_convert_line_disk_multiply() {
        let line = br#";1!inode,2026-01-05 00:00:00,2026-01-05 00:00:00,0,0,drwxr-xr-x,4096,8,"/test""#;
        let result = convert(line);
        assert!(result.ends_with(",4096,\"/test\""));
    }

    #[test]
    fn test_invalid_utf8_in_path() {
        // Invalid UTF-8 bytes in path are preserved as-is
        let mut line = br#";1!inode,2026-01-05 00:00:00,2026-01-05 00:00:00,0,0,drwxr-xr-x,4096,0,"/test"#.to_vec();
        line.extend_from_slice(&[0xFF, 0xFE]);
        line.push(b'"');
        
        let (mut tc, mut mc) = caches();
        let mut out = Vec::new();
        convert_line(&line, &mut out, &mut tc, &mut mc).unwrap();
        
        // Output should contain the invalid bytes
        assert!(out.windows(2).any(|w| w == [0xFF, 0xFE]));
    }
}
