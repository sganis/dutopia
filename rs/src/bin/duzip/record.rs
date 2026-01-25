// rs/src/bin/duzip/record.rs
use anyhow::Result;

#[derive(Debug, Clone, PartialEq)]
pub struct BinaryRecord {
    pub path: Vec<u8>,
    pub dev: u64,
    pub ino: u64,
    pub atime: i64,
    pub mtime: i64,
    pub uid: u32,
    pub gid: u32,
    pub mode: u32,
    pub size: u64,
    pub disk: u64,
}

/// Parse CSV line as raw bytes
pub fn parse_csv_line_bytes(line: &[u8]) -> Vec<Vec<u8>> {
    let mut fields = Vec::new();
    let mut current_field = Vec::new();
    let mut in_quotes = false;
    let mut i = 0;

    while i < line.len() {
        let b = line[i];
        match b {
            b'"' if !in_quotes => {
                in_quotes = true;
            }
            b'"' if in_quotes => {
                if i + 1 < line.len() && line[i + 1] == b'"' {
                    current_field.push(b'"');
                    i += 1;
                } else {
                    in_quotes = false;
                }
            }
            b',' if !in_quotes => {
                fields.push(current_field);
                current_field = Vec::new();
            }
            _ => {
                current_field.push(b);
            }
        }
        i += 1;
    }

    fields.push(current_field);
    fields
}

/// Parse CSV record from raw bytes into BinaryRecord
pub fn parse_csv_record_bytes(line: &[u8]) -> Result<BinaryRecord> {
    let fields = parse_csv_line_bytes(line);

    if fields.len() != 9 {
        anyhow::bail!(
            "CSV record must have 9 fields, got {}: {}",
            fields.len(),
            String::from_utf8_lossy(line)
        );
    }

    let inode_str = String::from_utf8_lossy(&fields[0]);
    let inode_parts: Vec<&str> = inode_str.split('-').collect();
    if inode_parts.len() != 2 {
        anyhow::bail!("Invalid INODE format, expected dev-ino: {}", inode_str);
    }

    let dev = inode_parts[0]
        .parse::<u64>()
        .map_err(|e| anyhow::anyhow!("Invalid dev: {}", e))?;

    let ino = inode_parts[1]
        .parse::<u64>()
        .map_err(|e| anyhow::anyhow!("Invalid ino: {}", e))?;

    let atime = String::from_utf8_lossy(&fields[1])
        .parse::<i64>()
        .map_err(|e| anyhow::anyhow!("Invalid atime: {}", e))?;

    let mtime = String::from_utf8_lossy(&fields[2])
        .parse::<i64>()
        .map_err(|e| anyhow::anyhow!("Invalid mtime: {}", e))?;

    let uid = String::from_utf8_lossy(&fields[3])
        .parse::<u32>()
        .map_err(|e| anyhow::anyhow!("Invalid uid: {}", e))?;

    let gid = String::from_utf8_lossy(&fields[4])
        .parse::<u32>()
        .map_err(|e| anyhow::anyhow!("Invalid gid: {}", e))?;

    let mode = String::from_utf8_lossy(&fields[5])
        .parse::<u32>()
        .map_err(|e| anyhow::anyhow!("Invalid mode: {}", e))?;

    let size = String::from_utf8_lossy(&fields[6])
        .parse::<u64>()
        .map_err(|e| anyhow::anyhow!("Invalid size: {}", e))?;

    let disk = String::from_utf8_lossy(&fields[7])
        .parse::<u64>()
        .map_err(|e| anyhow::anyhow!("Invalid disk: {}", e))?;

    let path = fields[8].clone();

    Ok(BinaryRecord {
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
    })
}

#[cfg(test)]
pub fn parse_csv_record(line: &str) -> Result<BinaryRecord> {
    parse_csv_record_bytes(line.as_bytes())
}

#[cfg(test)]
pub fn parse_csv_line(line: &str) -> Vec<String> {
    let byte_fields = parse_csv_line_bytes(line.as_bytes());
    byte_fields
        .into_iter()
        .map(|field| String::from_utf8_lossy(&field).into_owned())
        .collect()
}

#[cfg(test)]
pub fn format_csv_record(rec: &BinaryRecord) -> String {
    fn needs_quote(s: &str) -> bool {
        s.as_bytes()
            .iter()
            .any(|&b| matches!(b, b',' | b'"' | b'\n' | b'\r'))
    }
    fn quote_csv(s: &str) -> String {
        if !needs_quote(s) {
            return s.to_string();
        }
        let mut out = String::with_capacity(s.len() + 2);
        out.push('"');
        for ch in s.chars() {
            if ch == '"' {
                out.push('"');
                out.push('"');
            } else {
                out.push(ch);
            }
        }
        out.push('"');
        out
    }

    let inode = format!("{}-{}", rec.dev, rec.ino);
    let path_str = String::from_utf8_lossy(&rec.path);
    let path_csv = quote_csv(&path_str);

    format!(
        "{inode},{atime},{mtime},{uid},{gid},{mode},{size},{disk},{path}",
        inode = inode,
        atime = rec.atime,
        mtime = rec.mtime,
        uid = rec.uid,
        gid = rec.gid,
        mode = rec.mode,
        size = rec.size,
        disk = rec.disk,
        path = path_csv
    )
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn sample_record() -> BinaryRecord {
        BinaryRecord {
            path: b"/home/user/test.txt".to_vec(),
            dev: 2049,
            ino: 12345,
            atime: 1672531200,
            mtime: 1672617600,
            uid: 1000,
            gid: 1000,
            mode: 33188,
            size: 1024,
            disk: 42,
        }
    }

    pub fn sample_record_with_quotes() -> BinaryRecord {
        BinaryRecord {
            path: b"path with \"quotes\".txt".to_vec(),
            dev: 2050,
            ino: 67890,
            atime: -1,
            mtime: 0,
            uid: 0,
            gid: 0,
            mode: 16877,
            size: 4096,
            disk: 1,
        }
    }

    pub fn sample_record_with_newline() -> BinaryRecord {
        BinaryRecord {
            path: b"path\nwith\nnewlines.txt".to_vec(),
            dev: 2051,
            ino: 11111,
            atime: 1000000000,
            mtime: 2000000000,
            uid: 500,
            gid: 500,
            mode: 33188,
            size: 2048,
            disk: 3,
        }
    }

    pub fn sample_record_non_utf8() -> BinaryRecord {
        BinaryRecord {
            path: vec![0xFF, 0xFE, b'/', b'p', b'a', b't', b'h'],
            dev: 2052,
            ino: 22222,
            atime: 1500000000,
            mtime: 1600000000,
            uid: 750,
            gid: 750,
            mode: 16877,
            size: 4096,
            disk: 4,
        }
    }

    #[test]
    fn test_parse_csv_line_bytes_simple() {
        let line = b"a,b,c,d";
        let fields = parse_csv_line_bytes(line);
        assert_eq!(
            fields,
            vec![
                b"a".to_vec(),
                b"b".to_vec(),
                b"c".to_vec(),
                b"d".to_vec()
            ]
        );
    }

    #[test]
    fn test_parse_csv_line_bytes_quoted() {
        let line = br#"a,"b,c",d"#;
        let fields = parse_csv_line_bytes(line);
        assert_eq!(
            fields,
            vec![b"a".to_vec(), b"b,c".to_vec(), b"d".to_vec()]
        );
    }

    #[test]
    fn test_parse_csv_line_bytes_with_newline() {
        let line = b"a,\"b\nc\",d";
        let fields = parse_csv_line_bytes(line);
        assert_eq!(
            fields,
            vec![b"a".to_vec(), b"b\nc".to_vec(), b"d".to_vec()]
        );
    }

    #[test]
    fn test_parse_csv_line_bytes_non_utf8() {
        let line = vec![b'a', b',', b'"', 0xFF, 0xFE, b'"', b',', b'd'];
        let fields = parse_csv_line_bytes(&line);
        assert_eq!(
            fields,
            vec![b"a".to_vec(), vec![0xFF, 0xFE], b"d".to_vec()]
        );
    }

    #[test]
    fn test_parse_csv_line_simple() {
        let line = "a,b,c,d";
        let fields = parse_csv_line(line);
        assert_eq!(fields, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn test_parse_csv_line_quoted() {
        let line = r#"a,"b,c",d"#;
        let fields = parse_csv_line(line);
        assert_eq!(fields, vec!["a", "b,c", "d"]);
    }

    #[test]
    fn test_parse_csv_line_escaped_quotes() {
        let line = r#"a,"b""c",d"#;
        let fields = parse_csv_line(line);
        assert_eq!(fields, vec!["a", r#"b"c"#, "d"]);
    }

    #[test]
    fn test_parse_csv_line_empty_fields() {
        let line = "a,,c";
        let fields = parse_csv_line(line);
        assert_eq!(fields, vec!["a", "", "c"]);
    }

    #[test]
    fn test_parse_csv_line_trailing_comma() {
        let line = "a,b,c,";
        let fields = parse_csv_line(line);
        assert_eq!(fields, vec!["a", "b", "c", ""]);
    }

    #[test]
    fn test_parse_csv_record_valid() {
        let csv_line =
            "2049-12345,1672531200,1672617600,1000,1000,33188,1024,42,/home/user/test.txt";
        let record = parse_csv_record(csv_line).unwrap();
        let expected = sample_record();
        assert_eq!(record, expected);
    }

    #[test]
    fn test_parse_csv_record_with_quoted_path() {
        let csv_line = r#"2050-67890,-1,0,0,0,16877,4096,1,"path with ""quotes"".txt""#;
        let record = parse_csv_record(csv_line).unwrap();
        let expected = sample_record_with_quotes();
        assert_eq!(record, expected);
    }

    #[test]
    fn test_parse_csv_record_with_newline_in_path() {
        let csv_line =
            "2051-11111,1000000000,2000000000,500,500,33188,2048,3,\"path\nwith\nnewlines.txt\"";
        let record = parse_csv_record(csv_line).unwrap();
        let expected = sample_record_with_newline();
        assert_eq!(record, expected);
    }

    #[test]
    fn test_parse_csv_record_invalid_fields_count() {
        let csv_line = "2049-12345,1672531200,1672617600";
        let result = parse_csv_record(csv_line);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must have 9 fields"));
    }

    #[test]
    fn test_parse_csv_record_invalid_inode_format() {
        let csv_line = "invalid-inode,1672531200,1672617600,1000,1000,33188,1024,42,/path";
        let result = parse_csv_record(csv_line);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid dev"));
    }

    #[test]
    fn test_parse_csv_record_missing_dev_ino_separator() {
        let csv_line = "204912345,1672531200,1672617600,1000,1000,33188,1024,42,/path";
        let result = parse_csv_record(csv_line);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid INODE format"));
    }

    #[test]
    fn test_format_csv_record() {
        let record = sample_record();
        let csv_line = format_csv_record(&record);
        assert_eq!(
            csv_line,
            "2049-12345,1672531200,1672617600,1000,1000,33188,1024,42,/home/user/test.txt"
        );
    }

    #[test]
    fn test_format_csv_record_with_quotes() {
        let record = sample_record_with_quotes();
        let csv_line = format_csv_record(&record);
        assert_eq!(
            csv_line,
            r#"2050-67890,-1,0,0,0,16877,4096,1,"path with ""quotes"".txt""#
        );
    }

    #[test]
    fn test_format_csv_record_with_newline() {
        let record = sample_record_with_newline();
        let csv_line = format_csv_record(&record);
        assert_eq!(
            csv_line,
            "2051-11111,1000000000,2000000000,500,500,33188,2048,3,\"path\nwith\nnewlines.txt\""
        );
    }

    #[test]
    fn test_format_csv_record_non_utf8() {
        let record = sample_record_non_utf8();
        let csv_line = format_csv_record(&record);
        assert!(csv_line.contains("2052-22222"));
        assert!(csv_line.contains("/path"));
    }

    #[test]
    fn test_csv_roundtrip_format_then_parse() {
        let rec = sample_record_with_quotes();
        let csv = format_csv_record(&rec);
        let parsed = parse_csv_record(&csv).unwrap();
        assert_eq!(rec, parsed);
    }

    #[test]
    fn test_csv_roundtrip_with_newline() {
        let rec = sample_record_with_newline();
        let csv = format_csv_record(&rec);
        let parsed = parse_csv_record(&csv).unwrap();
        assert_eq!(rec, parsed);
    }

    #[test]
    fn test_csv_roundtrip_non_utf8() {
        let rec = sample_record_non_utf8();
        let csv = format_csv_record(&rec);
        let parsed = parse_csv_record(&csv).unwrap();
        assert_eq!(rec.dev, parsed.dev);
        assert_eq!(rec.ino, parsed.ino);
        assert_eq!(rec.atime, parsed.atime);
        assert_eq!(rec.mtime, parsed.mtime);
        assert_eq!(rec.uid, parsed.uid);
        assert_eq!(rec.gid, parsed.gid);
        assert_eq!(rec.mode, parsed.mode);
        assert_eq!(rec.size, parsed.size);
        assert_eq!(rec.disk, parsed.disk);
    }
}
