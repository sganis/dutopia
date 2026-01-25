// rs/src/bin/duzip/compress.rs
use anyhow::Result;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;

use crate::record::{parse_csv_record_bytes, BinaryRecord};

pub const READ_BUF_SIZE: usize = 2 * 1024 * 1024;
pub const WRITE_BUF_SIZE: usize = 8 * 1024 * 1024;

pub fn csv_to_zst(input: &PathBuf, output: Option<&PathBuf>) -> Result<()> {
    let start = std::time::Instant::now();
    let input_file = File::open(input)?;
    let mut reader = BufReader::with_capacity(READ_BUF_SIZE, input_file);

    let out_path = output
        .cloned()
        .unwrap_or_else(|| input.with_extension("zst"));

    if out_path.exists() {
        anyhow::bail!("Output file already exists: {}", out_path.display());
    }

    let out_file = File::create(&out_path)?;
    let encoder = zstd::stream::write::Encoder::new(out_file, 1)?;
    let mut writer = BufWriter::with_capacity(WRITE_BUF_SIZE, encoder);

    let mut header_line = String::new();
    reader.read_line(&mut header_line)?;
    let header = header_line.trim();

    if header != "INODE,ATIME,MTIME,UID,GID,MODE,SIZE,DISK,PATH" {
        anyhow::bail!(
            "Invalid CSV header. Expected: INODE,ATIME,MTIME,UID,GID,MODE,SIZE,DISK,PATH, Got: {}",
            header
        );
    }

    println!("Creating .zst file...");

    let mut line_buf = Vec::new();

    loop {
        line_buf.clear();
        let bytes_read = read_line_bytes(&mut reader, &mut line_buf)?;
        if bytes_read == 0 {
            break;
        }

        if line_buf.ends_with(b"\n") {
            line_buf.pop();
            if line_buf.ends_with(b"\r") {
                line_buf.pop();
            }
        }

        if line_buf.is_empty() {
            continue;
        }

        let record = parse_csv_record_bytes(&line_buf)?;
        write_binary_record(&mut writer, &record)?;
    }

    let encoder = writer
        .into_inner()
        .map_err(|_| anyhow::anyhow!("failed to flush buffered zstd encoder"))?;
    encoder.finish()?;

    println!("Output       : {}", out_path.display());
    println!("Elapsed time : {:.3} sec.", start.elapsed().as_secs_f64());

    Ok(())
}

pub fn read_line_bytes<R: BufRead>(reader: &mut R, buf: &mut Vec<u8>) -> Result<usize> {
    let mut bytes_read = 0;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            break;
        }

        if let Some(newline_pos) = available.iter().position(|&b| b == b'\n') {
            let to_read = newline_pos + 1;
            buf.extend_from_slice(&available[..to_read]);
            reader.consume(to_read);
            bytes_read += to_read;
            break;
        } else {
            buf.extend_from_slice(available);
            let len = available.len();
            reader.consume(len);
            bytes_read += len;
        }
    }
    Ok(bytes_read)
}

pub fn write_binary_record<W: Write>(writer: &mut W, record: &BinaryRecord) -> Result<()> {
    let path_len = record.path.len() as u32;
    writer.write_all(&path_len.to_le_bytes())?;
    writer.write_all(&record.path)?;
    writer.write_all(&record.dev.to_le_bytes())?;
    writer.write_all(&record.ino.to_le_bytes())?;
    writer.write_all(&record.atime.to_le_bytes())?;
    writer.write_all(&record.mtime.to_le_bytes())?;
    writer.write_all(&record.uid.to_le_bytes())?;
    writer.write_all(&record.gid.to_le_bytes())?;
    writer.write_all(&record.mode.to_le_bytes())?;
    writer.write_all(&record.size.to_le_bytes())?;
    writer.write_all(&record.disk.to_le_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::tests::{
        sample_record, sample_record_non_utf8, sample_record_with_newline,
        sample_record_with_quotes,
    };
    use crate::decompress::read_binary_record;
    use std::io::Cursor;

    #[test]
    fn test_write_and_read_binary_record() {
        let record = sample_record();
        let mut buffer = Vec::new();
        write_binary_record(&mut buffer, &record).unwrap();
        let mut cursor = Cursor::new(&buffer);
        let read_record = read_binary_record(&mut cursor).unwrap().unwrap();
        assert_eq!(record, read_record);
    }

    #[test]
    fn test_write_and_read_binary_record_with_quotes() {
        let record = sample_record_with_quotes();
        let mut buffer = Vec::new();
        write_binary_record(&mut buffer, &record).unwrap();
        let mut cursor = Cursor::new(&buffer);
        let read_record = read_binary_record(&mut cursor).unwrap().unwrap();
        assert_eq!(record, read_record);
    }

    #[test]
    fn test_write_and_read_binary_record_with_newline() {
        let record = sample_record_with_newline();
        let mut buffer = Vec::new();
        write_binary_record(&mut buffer, &record).unwrap();
        let mut cursor = Cursor::new(&buffer);
        let read_record = read_binary_record(&mut cursor).unwrap().unwrap();
        assert_eq!(record, read_record);
    }

    #[test]
    fn test_write_and_read_binary_record_non_utf8() {
        let record = sample_record_non_utf8();
        let mut buffer = Vec::new();
        write_binary_record(&mut buffer, &record).unwrap();
        let mut cursor = Cursor::new(&buffer);
        let read_record = read_binary_record(&mut cursor).unwrap().unwrap();
        assert_eq!(record, read_record);
    }

    #[test]
    fn test_read_line_bytes() {
        let data = b"line1\nline2\r\nline3\n";
        let mut cursor = Cursor::new(&data[..]);
        let mut buf = Vec::new();

        let bytes_read = read_line_bytes(&mut cursor, &mut buf).unwrap();
        assert_eq!(bytes_read, 6);
        assert_eq!(buf, b"line1\n");

        buf.clear();
        let bytes_read = read_line_bytes(&mut cursor, &mut buf).unwrap();
        assert_eq!(bytes_read, 7);
        assert_eq!(buf, b"line2\r\n");

        buf.clear();
        let bytes_read = read_line_bytes(&mut cursor, &mut buf).unwrap();
        assert_eq!(bytes_read, 6);
        assert_eq!(buf, b"line3\n");

        buf.clear();
        let bytes_read = read_line_bytes(&mut cursor, &mut buf).unwrap();
        assert_eq!(bytes_read, 0);
        assert!(buf.is_empty());
    }
}
