// rs/src/bin/duscan/row.rs
use std::fs;
use std::path::Path;
use dutopia::util::Row;

pub fn row_from_metadata(md: &fs::Metadata) -> Row {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Row {
            dev: md.dev(),
            ino: md.ino(),
            mode: md.mode(),
            uid: md.uid(),
            gid: md.gid(),
            size: md.size(),
            blocks: md.blocks() as u64,
            atime: md.atime(),
            mtime: md.mtime(),
        }
    }
    #[cfg(windows)]
    {
        use std::time::SystemTime;

        let to_unix = |t: SystemTime| -> i64 {
            t.duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        };
        let atime = md.accessed().ok().map(to_unix).unwrap_or(0);
        let mtime = md.modified().ok().map(to_unix).unwrap_or(0);
        let blocks = (md.len() + 511) / 512;

        Row {
            dev: 0,
            ino: 0,
            mode: 0,
            uid: 0,
            gid: 0,
            size: md.len(),
            blocks,
            atime,
            mtime,
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        Row {
            dev: 0,
            ino: 0,
            mode: 0,
            uid: 0,
            gid: 0,
            size: md.len(),
            blocks: 0,
            atime: 0,
            mtime: 0,
        }
    }
}

pub fn stat_row(path: &Path) -> Option<Row> {
    let md = fs::symlink_metadata(path).ok()?;
    Some(row_from_metadata(&md))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_row_from_metadata() {
        let tmp = tempdir().unwrap();
        let test_file = tmp.path().join("test.txt");
        fs::write(&test_file, "test content").unwrap();

        let metadata = fs::metadata(&test_file).unwrap();
        let row = row_from_metadata(&metadata);

        assert_eq!(row.size, 12);
        assert!(row.mtime > 0);

        #[cfg(unix)]
        {
            assert!(row.dev > 0);
            assert!(row.ino > 0);
            assert!(row.mode > 0);
        }

        #[cfg(windows)]
        {
            assert_eq!(row.dev, 0);
            assert_eq!(row.ino, 0);
            assert_eq!(row.uid, 0);
            assert_eq!(row.gid, 0);
            assert_eq!(row.mode, 0);
        }
    }

    #[test]
    fn test_stat_row_success() {
        let tmp = tempdir().unwrap();
        let test_file = tmp.path().join("test.txt");
        fs::write(&test_file, "test content").unwrap();

        let row = stat_row(&test_file);
        assert!(row.is_some());
        assert_eq!(row.unwrap().size, 12);
    }

    #[test]
    fn test_stat_row_failure() {
        let nonexistent = Path::new("/nonexistent/path/that/does/not/exist");
        let row = stat_row(nonexistent);
        assert!(row.is_none());
    }

    #[test]
    fn test_stat_row_directory() {
        let tmp = tempdir().unwrap();
        let test_dir = tmp.path().join("testdir");
        fs::create_dir(&test_dir).unwrap();

        let row = stat_row(&test_dir);
        assert!(row.is_some());

        #[cfg(unix)]
        {
            let row = row.unwrap();
            assert!(row.mode > 0);
        }
    }

    #[test]
    fn test_row_creation() {
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

        assert_eq!(row.dev, 1);
        assert_eq!(row.ino, 2);
        assert_eq!(row.mode, 755);
        assert_eq!(row.uid, 1000);
        assert_eq!(row.gid, 1000);
        assert_eq!(row.size, 1024);
        assert_eq!(row.blocks, 2);
        assert_eq!(row.atime, 1234567890);
        assert_eq!(row.mtime, 1234567891);
    }
}
