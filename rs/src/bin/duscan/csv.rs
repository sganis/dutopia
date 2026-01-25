// rs/src/bin/duscan/csv.rs
use std::path::Path;
use dutopia::util::{Row, push_u32, push_u64, push_i64};

#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

pub fn write_row_csv(buf: &mut Vec<u8>, path: &Path, r: &Row, no_atime: bool) {
    buf.reserve(256);
    // INODE as dev-ino
    push_u64(buf, r.dev);
    buf.push(b'-');
    push_u64(buf, r.ino);
    buf.push(b',');

    // ATIME (zeroed if requested)
    if no_atime {
        push_i64(buf, 0);
    } else {
        push_i64(buf, r.atime);
    }
    buf.push(b',');

    // MTIME
    push_i64(buf, r.mtime);
    buf.push(b',');

    // UID, GID, MODE
    push_u32(buf, r.uid);
    buf.push(b',');
    push_u32(buf, r.gid);
    buf.push(b',');
    push_u32(buf, r.mode);
    buf.push(b',');

    // SIZE, DISK
    push_u64(buf, r.size);
    buf.push(b',');
    let disk = r.blocks * 512;
    push_u64(buf, disk);
    buf.push(b',');

    csv_push_path_smart_quoted(buf, path);
    buf.push(b'\n');
}

pub fn write_row_bin(buf: &mut Vec<u8>, path: &Path, r: &Row, no_atime: bool) {
    #[cfg(unix)]
    let path_bytes: &[u8] = path.as_os_str().as_bytes();

    #[cfg(not(unix))]
    let path_lossy = path.to_string_lossy();
    #[cfg(not(unix))]
    let path_bytes: &[u8] = path_lossy.as_bytes();

    let path_len = path_bytes.len() as u32;
    let atime = if no_atime { 0i64 } else { r.atime };
    let disk = r.blocks * 512;

    buf.reserve(80 + path_bytes.len());
    buf.extend_from_slice(&path_len.to_le_bytes());
    buf.extend_from_slice(path_bytes);
    buf.extend_from_slice(&r.dev.to_le_bytes());
    buf.extend_from_slice(&r.ino.to_le_bytes());
    buf.extend_from_slice(&atime.to_le_bytes());
    buf.extend_from_slice(&r.mtime.to_le_bytes());
    buf.extend_from_slice(&r.uid.to_le_bytes());
    buf.extend_from_slice(&r.gid.to_le_bytes());
    buf.extend_from_slice(&r.mode.to_le_bytes());
    buf.extend_from_slice(&r.size.to_le_bytes());
    buf.extend_from_slice(&disk.to_le_bytes());
}

pub fn csv_push_path_smart_quoted(buf: &mut Vec<u8>, p: &Path) {
    #[cfg(unix)]
    {
        let bytes = p.as_os_str().as_bytes();
        csv_push_bytes_smart_quoted(buf, bytes);
    }
    #[cfg(not(unix))]
    {
        let s = p.to_string_lossy();
        csv_push_str_smart_quoted(buf, &s);
    }
}

#[cfg(unix)]
pub fn csv_push_bytes_smart_quoted(buf: &mut Vec<u8>, bytes: &[u8]) {
    let needs_quoting = bytes
        .iter()
        .any(|&b| b == b'"' || b == b',' || b == b'\n' || b == b'\r');
    if !needs_quoting {
        buf.extend_from_slice(bytes);
    } else {
        buf.push(b'"');
        if !bytes.contains(&b'"') {
            buf.extend_from_slice(bytes);
        } else {
            buf.reserve(bytes.len() + bytes.iter().filter(|&&b| b == b'"').count());
            for &b in bytes {
                if b == b'"' {
                    buf.push(b'"');
                    buf.push(b'"');
                } else {
                    buf.push(b);
                }
            }
        }
        buf.push(b'"');
    }
}

#[cfg(windows)]
pub fn csv_push_str_smart_quoted(buf: &mut Vec<u8>, s: &str) {
    let normalized = if s.starts_with(r"\\?\") {
        if s.starts_with(r"\\?\UNC\") {
            format!(r"\\{}", &s[8..])
        } else {
            s[4..].to_string()
        }
    } else {
        s.to_string()
    };
    let display_str = normalized.as_str();
    let needs_quoting = display_str
        .chars()
        .any(|c| c == '"' || c == ',' || c == '\n' || c == '\r');
    if !needs_quoting {
        buf.extend_from_slice(display_str.as_bytes());
    } else {
        buf.push(b'"');
        if !display_str.contains('"') {
            buf.extend_from_slice(display_str.as_bytes());
        } else {
            let quote_count = display_str.matches('"').count();
            buf.reserve(display_str.len() + quote_count);
            for b in display_str.bytes() {
                if b == b'"' {
                    buf.push(b'"');
                    buf.push(b'"');
                } else {
                    buf.push(b);
                }
            }
        }
        buf.push(b'"');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn test_csv_push_bytes_smart_quoted_fast_path() {
        let mut buf = Vec::new();
        csv_push_bytes_smart_quoted(&mut buf, b"abc_def");
        assert_eq!(&buf, b"abc_def");
    }

    #[cfg(unix)]
    #[test]
    fn test_csv_push_bytes_smart_quoted_with_comma() {
        let mut buf = Vec::new();
        csv_push_bytes_smart_quoted(&mut buf, b"a,b");
        assert_eq!(&buf, b"\"a,b\"");
    }

    #[cfg(unix)]
    #[test]
    fn test_csv_push_bytes_smart_quoted_with_quote() {
        let mut buf = Vec::new();
        csv_push_bytes_smart_quoted(&mut buf, b"a\"b");
        assert_eq!(&buf, b"\"a\"\"b\"");
    }

    #[cfg(unix)]
    #[test]
    fn test_csv_push_bytes_smart_quoted_with_newline() {
        let mut buf = Vec::new();
        csv_push_bytes_smart_quoted(&mut buf, b"a\nb");
        assert_eq!(&buf, b"\"a\nb\"");
    }

    #[cfg(unix)]
    #[test]
    fn test_csv_push_bytes_smart_quoted_with_carriage_return() {
        let mut buf = Vec::new();
        csv_push_bytes_smart_quoted(&mut buf, b"a\rb");
        assert_eq!(&buf, b"\"a\rb\"");
    }

    #[cfg(unix)]
    #[test]
    fn test_csv_push_bytes_smart_quoted_multiple_quotes() {
        let mut buf = Vec::new();
        csv_push_bytes_smart_quoted(&mut buf, b"a\"b\"c");
        assert_eq!(&buf, b"\"a\"\"b\"\"c\"");
    }

    #[cfg(unix)]
    #[test]
    fn test_csv_push_bytes_edge_cases() {
        let mut buf = Vec::new();
        csv_push_bytes_smart_quoted(&mut buf, b"\"");
        assert_eq!(&buf, b"\"\"\"\"");

        let mut buf = Vec::new();
        csv_push_bytes_smart_quoted(&mut buf, b"");
        assert_eq!(&buf, b"");

        let mut buf = Vec::new();
        csv_push_bytes_smart_quoted(&mut buf, b"\",\n\r");
        assert_eq!(&buf, b"\"\"\",\n\r\"");
    }

    #[cfg(windows)]
    #[test]
    fn test_csv_push_str_smart_quoted_normalize_verbatim() {
        let mut buf = Vec::new();
        csv_push_str_smart_quoted(&mut buf, r"\\?\C:\foo\bar");
        assert_eq!(std::str::from_utf8(&buf).unwrap(), r"C:\foo\bar");

        let mut buf2 = Vec::new();
        csv_push_str_smart_quoted(&mut buf2, r"\\?\UNC\server\share\foo");
        assert_eq!(std::str::from_utf8(&buf2).unwrap(), r"\\server\share\foo");
    }

    #[test]
    fn test_csv_push_path_smart_quoted() {
        let mut buf = Vec::new();
        let path = Path::new("simple/path");
        csv_push_path_smart_quoted(&mut buf, path);

        #[cfg(unix)]
        assert_eq!(&buf, b"simple/path");
        #[cfg(windows)]
        assert_eq!(std::str::from_utf8(&buf).unwrap(), "simple/path");
    }

    #[test]
    fn test_write_row_csv_with_atime() {
        let mut buf = Vec::new();
        let path = Path::new("test/path");
        let row = Row {
            dev: 1,
            ino: 2,
            mode: 755,
            uid: 1000,
            gid: 1000,
            size: 1024,
            blocks: 2,
            atime: 1234567890,
            mtime: 1234567891,
        };

        write_row_csv(&mut buf, path, &row, false);
        let result = String::from_utf8(buf).unwrap();

        assert!(result.starts_with("1-2,1234567890,1234567891,1000,1000,755,1024,1024,"));
        assert!(result.ends_with("\n"));
    }

    #[test]
    fn test_write_row_csv_no_atime() {
        let mut buf = Vec::new();
        let path = Path::new("test/path");
        let row = Row {
            dev: 1,
            ino: 2,
            mode: 755,
            uid: 1000,
            gid: 1000,
            size: 1024,
            blocks: 2,
            atime: 1234567890,
            mtime: 1234567891,
        };

        write_row_csv(&mut buf, path, &row, true);
        let result = String::from_utf8(buf).unwrap();

        assert!(result.starts_with("1-2,0,1234567891,1000,1000,755,1024,1024,"));
    }

    #[test]
    fn test_write_row_bin_with_atime() {
        let mut buf = Vec::new();
        let path = Path::new("test");
        let row = Row {
            dev: 1,
            ino: 2,
            mode: 755,
            uid: 1000,
            gid: 1000,
            size: 1024,
            blocks: 2,
            atime: 1234567890,
            mtime: 1234567891,
        };

        write_row_bin(&mut buf, path, &row, false);

        assert!(buf.len() >= 68);

        let path_len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        assert_eq!(path_len, 4);
    }

    #[test]
    fn test_write_row_bin_no_atime() {
        let mut buf = Vec::new();
        let path = Path::new("test");
        let row = Row {
            dev: 1,
            ino: 2,
            mode: 755,
            uid: 1000,
            gid: 1000,
            size: 1024,
            blocks: 2,
            atime: 1234567890,
            mtime: 1234567891,
        };

        write_row_bin(&mut buf, path, &row, true);

        let path_len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        let atime_offset = 4 + path_len + 8 + 8;
        let atime_bytes = &buf[atime_offset..atime_offset + 8];
        let atime = i64::from_le_bytes([
            atime_bytes[0],
            atime_bytes[1],
            atime_bytes[2],
            atime_bytes[3],
            atime_bytes[4],
            atime_bytes[5],
            atime_bytes[6],
            atime_bytes[7],
        ]);
        assert_eq!(atime, 0);
    }

    #[test]
    fn test_write_row_bin_empty_path() {
        let mut buf = Vec::new();
        let path = Path::new("");
        let row = Row {
            dev: 0,
            ino: 0,
            mode: 0,
            uid: 0,
            gid: 0,
            size: 0,
            blocks: 0,
            atime: 0,
            mtime: 0,
        };

        write_row_bin(&mut buf, path, &row, false);

        let path_len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        assert_eq!(path_len, 0);
    }

    #[test]
    fn test_csv_disk_calculation() {
        let mut buf = Vec::new();
        let path = Path::new("test");
        let row = Row {
            dev: 1,
            ino: 2,
            mode: 755,
            uid: 1000,
            gid: 1000,
            size: 1024,
            blocks: 3,
            atime: 1234567890,
            mtime: 1234567891,
        };

        write_row_csv(&mut buf, path, &row, false);
        let result = String::from_utf8(buf).unwrap();

        assert!(result.contains(",1536,"));
    }

    #[test]
    fn test_bin_disk_calculation() {
        let mut buf = Vec::new();
        let path = Path::new("test");
        let row = Row {
            dev: 1,
            ino: 2,
            mode: 755,
            uid: 1000,
            gid: 1000,
            size: 1024,
            blocks: 3,
            atime: 1234567890,
            mtime: 1234567891,
        };

        write_row_bin(&mut buf, path, &row, false);

        let disk_offset = buf.len() - 8;
        let disk_bytes = &buf[disk_offset..];
        let disk = u64::from_le_bytes([
            disk_bytes[0],
            disk_bytes[1],
            disk_bytes[2],
            disk_bytes[3],
            disk_bytes[4],
            disk_bytes[5],
            disk_bytes[6],
            disk_bytes[7],
        ]);

        assert_eq!(disk, 1536);
    }

    #[test]
    fn test_csv_push_path_with_special_chars() {
        let mut buf = Vec::new();
        let path = Path::new("path with spaces,commas\"quotes\nand\rnewlines");
        csv_push_path_smart_quoted(&mut buf, path);

        let result = String::from_utf8(buf).unwrap();
        assert!(result.starts_with('"'));
        assert!(result.ends_with('"'));
        assert!(result.contains(r#""""#));
    }

    #[test]
    fn test_write_row_csv_large_buffer() {
        let mut buf = Vec::new();

        for _ in 0..1000 {
            buf.extend_from_slice(&vec![b'x'; 1000]);
        }

        let initial_len = buf.len();
        let path = Path::new("test/path");
        let row = Row {
            dev: 1,
            ino: 2,
            mode: 755,
            uid: 1000,
            gid: 1000,
            size: 1024,
            blocks: 2,
            atime: 1234567890,
            mtime: 1234567891,
        };

        write_row_csv(&mut buf, path, &row, false);
        assert!(buf.len() > initial_len);
    }

    #[test]
    fn test_write_row_bin_large_buffer() {
        let mut buf = Vec::new();

        for _ in 0..1000 {
            buf.extend_from_slice(&vec![b'x'; 1000]);
        }

        let initial_len = buf.len();
        let path = Path::new("test/path/with/long/name");
        let row = Row {
            dev: 1,
            ino: 2,
            mode: 755,
            uid: 1000,
            gid: 1000,
            size: 1024,
            blocks: 2,
            atime: 1234567890,
            mtime: 1234567891,
        };

        write_row_bin(&mut buf, path, &row, false);
        assert!(buf.len() > initial_len);
    }

    #[test]
    fn test_buffer_reservation() {
        let mut buf = Vec::new();
        let initial_capacity = buf.capacity();

        let path = Path::new("some/test/path");
        let row = Row {
            dev: 1,
            ino: 2,
            mode: 755,
            uid: 1000,
            gid: 1000,
            size: 1024,
            blocks: 2,
            atime: 1234567890,
            mtime: 1234567891,
        };

        write_row_csv(&mut buf, path, &row, false);

        assert!(buf.capacity() >= initial_capacity + 256);
    }

    #[test]
    fn test_binary_buffer_reservation() {
        let mut buf = Vec::new();
        let initial_capacity = buf.capacity();

        let path = Path::new("some/test/path/that/is/longer");
        let row = Row {
            dev: 1,
            ino: 2,
            mode: 755,
            uid: 1000,
            gid: 1000,
            size: 1024,
            blocks: 2,
            atime: 1234567890,
            mtime: 1234567891,
        };

        write_row_bin(&mut buf, path, &row, false);

        assert!(buf.capacity() >= initial_capacity + 80 + path.as_os_str().len());
    }
}
