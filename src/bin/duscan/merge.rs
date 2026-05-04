// rs/src/bin/duscan/merge.rs
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;
use dutopia::util::get_hostname;

const READ_BUF_SIZE: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputFormat {
    Csv,
    Bin,
}

pub fn merge_shards(
    out_dir: &Path,
    final_path: &Path,
    threads: usize,
    out_fmt: OutputFormat,
    sort_csv: bool,
    pid: u32,
) -> io::Result<()> {
    let mut out = BufWriter::with_capacity(16 * 1024 * 1024, File::create(final_path)?);

    match out_fmt {
        OutputFormat::Csv => merge_shards_csv(out_dir, &mut out, threads, sort_csv, pid),
        OutputFormat::Bin => merge_shards_bin(out_dir, &mut out, threads, pid),
    }?;

    out.flush()?;
    Ok(())
}

fn merge_shards_csv(
    out_dir: &Path,
    out: &mut BufWriter<File>,
    threads: usize,
    sort_csv: bool,
    pid: u32,
) -> io::Result<()> {
    out.write_all(b"INODE,ATIME,MTIME,UID,GID,MODE,SIZE,DISK,PATH\n")?;
    let hostname = get_hostname();

    if !sort_csv {
        for tid in 0..threads {
            let shard = out_dir.join(format!("shard_{hostname}_{pid}_{tid}.tmp"));
            if !shard.exists() {
                continue;
            }
            let f = File::open(&shard)?;
            let mut reader = BufReader::with_capacity(READ_BUF_SIZE, f);
            io::copy(&mut reader, out)?;
            let _ = std::fs::remove_file(shard);
        }
        return Ok(());
    }

    // Sorted mode (only used when --skip-atime and CSV)
    let mut lines: Vec<String> = Vec::new();

    for tid in 0..threads {
        let shard = out_dir.join(format!("shard_{hostname}_{pid}_{tid}.tmp"));
        if !shard.exists() {
            continue;
        }

        let f = File::open(&shard)?;
        let mut reader = BufReader::with_capacity(READ_BUF_SIZE, f);

        let mut buf = String::new();
        reader.read_to_string(&mut buf)?;
        for line in buf.split_inclusive('\n') {
            if line.trim().is_empty() {
                continue;
            }
            let ln = line.strip_suffix('\n').unwrap_or(line).to_string();
            if !ln.is_empty() {
                lines.push(ln);
            }
        }

        let _ = std::fs::remove_file(shard);
    }

    lines.sort_unstable();

    for ln in lines {
        out.write_all(ln.as_bytes())?;
        out.write_all(b"\n")?;
    }

    Ok(())
}

fn merge_shards_bin(
    out_dir: &Path,
    out: &mut BufWriter<File>,
    threads: usize,
    pid: u32,
) -> io::Result<()> {
    let hostname = get_hostname();
    for tid in 0..threads {
        let shard = out_dir.join(format!("shard_{hostname}_{pid}_{tid}.tmp"));
        if !shard.exists() {
            continue;
        }
        let f = File::open(&shard)?;
        let mut reader = BufReader::with_capacity(READ_BUF_SIZE, f);
        io::copy(&mut reader, out)?;
        let _ = std::fs::remove_file(shard);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use tempfile::tempdir;

    #[test]
    fn test_merge_shards_csv_unsorted_only() -> io::Result<()> {
        let tmp = tempdir()?;
        let out_dir = tmp.path().to_path_buf();
        let final_path = out_dir.join("out_unsorted.csv");
        let pid = 123;

        let shard0 = out_dir.join(format!("shard_{}_{}_0.tmp", get_hostname(), pid));
        let shard1 = out_dir.join(format!("shard_{}_{}_1.tmp", get_hostname(), pid));

        {
            let mut w = File::create(&shard0)?;
            w.write_all(b"b\n")?;
        }
        {
            let mut w = File::create(&shard1)?;
            w.write_all(b"a\n")?;
        }

        merge_shards(&out_dir, &final_path, 2, OutputFormat::Csv, false, pid)?;

        let mut s = String::new();
        File::open(&final_path)?.read_to_string(&mut s)?;
        let mut lines: Vec<&str> = s.lines().collect();

        assert_eq!(lines.remove(0), "INODE,ATIME,MTIME,UID,GID,MODE,SIZE,DISK,PATH");
        assert_eq!(lines, vec!["b", "a"]);
        Ok(())
    }

    #[test]
    fn test_merge_shards_csv_sorted_with_no_atime() -> io::Result<()> {
        let tmp = tempdir()?;
        let out_dir = tmp.path().to_path_buf();
        let final_path = out_dir.join("out_sorted.csv");
        let pid = 123;

        let shard0 = out_dir.join(format!("shard_{}_{}_0.tmp", get_hostname(), pid));
        let shard1 = out_dir.join(format!("shard_{}_{}_1.tmp", get_hostname(), pid));
        {
            let mut w = File::create(&shard0)?;
            w.write_all(b"b\n")?;
        }
        {
            let mut w = File::create(&shard1)?;
            w.write_all(b"a\n")?;
        }

        merge_shards(&out_dir, &final_path, 2, OutputFormat::Csv, true, pid)?;

        let mut s = String::new();
        File::open(&final_path)?.read_to_string(&mut s)?;
        let mut lines: Vec<&str> = s.lines().collect();

        assert_eq!(lines.remove(0), "INODE,ATIME,MTIME,UID,GID,MODE,SIZE,DISK,PATH");
        assert_eq!(lines, vec!["a", "b"]);
        Ok(())
    }

    #[test]
    fn test_merge_shards_bin() -> io::Result<()> {
        let tmp = tempdir()?;
        let out_dir = tmp.path().to_path_buf();
        let final_path = out_dir.join("out.bin");
        let pid = 123;

        let shard0 = out_dir.join(format!("shard_{}_{}_0.tmp", get_hostname(), pid));
        let shard1 = out_dir.join(format!("shard_{}_{}_1.tmp", get_hostname(), pid));

        {
            let mut w = File::create(&shard0)?;
            w.write_all(b"binary_data_0")?;
        }
        {
            let mut w = File::create(&shard1)?;
            w.write_all(b"binary_data_1")?;
        }

        merge_shards(&out_dir, &final_path, 2, OutputFormat::Bin, false, pid)?;

        let mut s = Vec::new();
        File::open(&final_path)?.read_to_end(&mut s)?;
        assert_eq!(s, b"binary_data_0binary_data_1");
        Ok(())
    }

    #[test]
    fn test_merge_shards_with_missing_shards() -> io::Result<()> {
        let tmp = tempdir()?;
        let out_dir = tmp.path().to_path_buf();
        let final_path = out_dir.join("out.csv");
        let pid = 123;

        let shard1 = out_dir.join(format!("shard_{}_{}_1.tmp", get_hostname(), pid));
        {
            let mut w = File::create(&shard1)?;
            w.write_all(b"data\n")?;
        }

        merge_shards(&out_dir, &final_path, 2, OutputFormat::Csv, false, pid)?;

        let mut s = String::new();
        File::open(&final_path)?.read_to_string(&mut s)?;
        let lines: Vec<&str> = s.lines().collect();

        assert_eq!(lines[0], "INODE,ATIME,MTIME,UID,GID,MODE,SIZE,DISK,PATH");
        assert_eq!(lines[1], "data");
        Ok(())
    }

    #[test]
    fn test_merge_shards_csv_with_empty_lines() -> io::Result<()> {
        let tmp = tempdir()?;
        let out_dir = tmp.path().to_path_buf();
        let final_path = out_dir.join("out_sorted.csv");
        let pid = 123;

        let shard0 = out_dir.join(format!("shard_{}_{}_0.tmp", get_hostname(), pid));
        {
            let mut w = File::create(&shard0)?;
            w.write_all(b"valid_line\n\n   \n")?;
        }

        merge_shards(&out_dir, &final_path, 1, OutputFormat::Csv, true, pid)?;

        let mut s = String::new();
        File::open(&final_path)?.read_to_string(&mut s)?;
        let lines: Vec<&str> = s.lines().collect();

        assert_eq!(lines[0], "INODE,ATIME,MTIME,UID,GID,MODE,SIZE,DISK,PATH");
        assert_eq!(lines[1], "valid_line");
        assert_eq!(lines.len(), 2);
        Ok(())
    }

    #[test]
    fn test_merge_shards_many_threads() -> io::Result<()> {
        let tmp = tempdir()?;
        let out_dir = tmp.path().to_path_buf();
        let final_path = out_dir.join("out.csv");
        let pid = 123;
        let num_threads = 100;

        let shard0 = out_dir.join(format!("shard_{}_{}_0.tmp", get_hostname(), pid));
        let shard50 = out_dir.join(format!("shard_{}_{}_50.tmp", get_hostname(), pid));

        {
            let mut w = File::create(&shard0)?;
            w.write_all(b"data0\n")?;
        }
        {
            let mut w = File::create(&shard50)?;
            w.write_all(b"data50\n")?;
        }

        merge_shards(&out_dir, &final_path, num_threads, OutputFormat::Csv, false, pid)?;

        let mut s = String::new();
        File::open(&final_path)?.read_to_string(&mut s)?;
        let lines: Vec<&str> = s.lines().collect();

        assert_eq!(lines[0], "INODE,ATIME,MTIME,UID,GID,MODE,SIZE,DISK,PATH");
        assert!(lines.contains(&"data0"));
        assert!(lines.contains(&"data50"));
        Ok(())
    }

    #[test]
    fn test_output_format_equality() {
        assert_eq!(OutputFormat::Csv, OutputFormat::Csv);
        assert_eq!(OutputFormat::Bin, OutputFormat::Bin);
        assert_ne!(OutputFormat::Csv, OutputFormat::Bin);
    }
}
