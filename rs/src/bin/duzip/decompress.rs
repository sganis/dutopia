// rs/src/bin/duzip/decompress.rs
use anyhow::Result;
use dutopia::util::{push_i64, push_u32, push_u64};
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, Write};
use std::path::PathBuf;

use crate::compress::{READ_BUF_SIZE, WRITE_BUF_SIZE};
use crate::record::BinaryRecord;

pub fn zst_to_csv(input: &PathBuf, output: Option<&PathBuf>) -> Result<()> {
    let start = std::time::Instant::now();
    let mut f = File::open(input)?;

    let mut magic_buf = [0u8; 4];
    f.read_exact(&mut magic_buf)?;
    f.rewind()?;
    let magic = u32::from_le_bytes(magic_buf);

    if magic != 0xFD2FB528 {
        eprintln!("Invalid format.");
        std::process::exit(1);
    }

    let reader: Box<dyn Read> = Box::new(zstd::stream::read::Decoder::new(f)?);
    let mut r = BufReader::with_capacity(READ_BUF_SIZE, reader);

    let out_path = output
        .cloned()
        .unwrap_or_else(|| input.with_extension("csv"));

    if out_path.exists() {
        anyhow::bail!(format!(
            "Output file already exists: {}",
            out_path.display()
        ));
    }

    let out_file = File::create(&out_path)?;
    let mut w = BufWriter::with_capacity(WRITE_BUF_SIZE, out_file);

    println!("Creating .csv file...");

    w.write_all(b"INODE,ATIME,MTIME,UID,GID,MODE,SIZE,DISK,PATH\n")?;

    let mut line = Vec::<u8>::with_capacity(256);
    let mut path_buf = Vec::<u8>::with_capacity(512);

    loop {
        let path_len = match read_u32_le_opt(&mut r)? {
            None => break,
            Some(v) => v as usize,
        };

        path_buf.resize(path_len, 0);
        read_exact_fully(&mut r, &mut path_buf)?;

        let dev = read_u64_le_exact(&mut r)?;
        let ino = read_u64_le_exact(&mut r)?;
        let atime = read_i64_le_exact(&mut r)?;
        let mtime = read_i64_le_exact(&mut r)?;
        let uid = read_u32_le_exact(&mut r)?;
        let gid = read_u32_le_exact(&mut r)?;
        let mode = read_u32_le_exact(&mut r)?;
        let size = read_u64_le_exact(&mut r)?;
        let disk = read_u64_le_exact(&mut r)?;

        line.clear();

        push_u64(&mut line, dev);
        line.push(b'-');
        push_u64(&mut line, ino);
        line.push(b',');
        push_i64(&mut line, atime);
        line.push(b',');
        push_i64(&mut line, mtime);
        line.push(b',');
        push_u32(&mut line, uid);
        line.push(b',');
        push_u32(&mut line, gid);
        line.push(b',');
        push_u32(&mut line, mode);
        line.push(b',');
        push_u64(&mut line, size);
        line.push(b',');
        push_u64(&mut line, disk);
        line.push(b',');
        csv_push_path(&mut line, &path_buf);
        line.push(b'\n');

        w.write_all(&line)?;
    }

    w.flush()?;
    println!("Output       : {}", out_path.display());
    println!("Elapsed time : {:.3} sec.", start.elapsed().as_secs_f64());

    Ok(())
}

#[cfg(unix)]
fn csv_push_path(out: &mut Vec<u8>, path_bytes: &[u8]) {
    let needs_quoting = path_bytes
        .iter()
        .any(|&b| b == b'"' || b == b',' || b == b'\n' || b == b'\r');

    if !needs_quoting {
        out.extend_from_slice(path_bytes);
    } else {
        out.push(b'"');
        for &b in path_bytes {
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

#[cfg(windows)]
fn csv_push_path(out: &mut Vec<u8>, path_bytes: &[u8]) {
    let s = String::from_utf8_lossy(path_bytes);
    let needs_quoting = s
        .chars()
        .any(|c| c == '"' || c == ',' || c == '\n' || c == '\r');
    if !needs_quoting {
        out.extend_from_slice(s.as_bytes());
    } else {
        out.push(b'"');
        for b in s.bytes() {
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

fn read_exact_fully<R: Read>(r: &mut R, buf: &mut [u8]) -> Result<()> {
    let mut read = 0;
    while read < buf.len() {
        let n = r.read(&mut buf[read..])?;
        if n == 0 {
            anyhow::bail!(
                "truncated input: expected {} bytes, got {}",
                buf.len(),
                read
            );
        }
        read += n;
    }
    Ok(())
}

fn read_u32_le_exact<R: Read>(r: &mut R) -> Result<u32> {
    let mut b = [0u8; 4];
    read_exact_fully(r, &mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn read_u64_le_exact<R: Read>(r: &mut R) -> Result<u64> {
    let mut b = [0u8; 8];
    read_exact_fully(r, &mut b)?;
    Ok(u64::from_le_bytes(b))
}

fn read_i64_le_exact<R: Read>(r: &mut R) -> Result<i64> {
    let mut b = [0u8; 8];
    read_exact_fully(r, &mut b)?;
    Ok(i64::from_le_bytes(b))
}

fn read_u32_le_opt<R: Read>(r: &mut R) -> Result<Option<u32>> {
    let mut b = [0u8; 4];
    let mut off = 0usize;
    loop {
        let n = r.read(&mut b[off..])?;
        if n == 0 {
            if off == 0 {
                return Ok(None);
            } else {
                anyhow::bail!("truncated record (path_len)");
            }
        }
        off += n;
        if off == 4 {
            return Ok(Some(u32::from_le_bytes(b)));
        }
    }
}

pub fn read_binary_record<R: Read>(r: &mut R) -> Result<Option<BinaryRecord>> {
    let path_len = match read_u32_le_opt(r)? {
        None => return Ok(None),
        Some(v) => v as usize,
    };

    let mut path = vec![0u8; path_len];
    read_exact_fully(r, &mut path)?;

    let dev = read_u64_le_exact(r)?;
    let ino = read_u64_le_exact(r)?;
    let atime = read_i64_le_exact(r)?;
    let mtime = read_i64_le_exact(r)?;
    let uid = read_u32_le_exact(r)?;
    let gid = read_u32_le_exact(r)?;
    let mode = read_u32_le_exact(r)?;
    let size = read_u64_le_exact(r)?;
    let disk = read_u64_le_exact(r)?;

    Ok(Some(BinaryRecord {
        path,
        dev,
        ino,
        atime,
        mtime,
        uid,
        gid,
        mode,
        size,
        disk,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_read_binary_record_empty() {
        let buffer = Vec::new();
        let mut cursor = Cursor::new(&buffer);
        let result = read_binary_record(&mut cursor).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_read_binary_record_truncated_path_len() {
        let buffer = vec![0x05, 0x00];
        let mut cursor = Cursor::new(&buffer);
        let err = read_binary_record(&mut cursor).unwrap_err();
        assert!(format!("{}", err).contains("path_len"));
    }
}
