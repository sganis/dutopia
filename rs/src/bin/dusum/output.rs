// rs/src/bin/dusum/output.rs
use anyhow::Result;
use csv::WriterBuilder;
use memchr::memchr_iter;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::aggregate::bytes_to_safe_string;
use crate::stats::UserStats;

pub fn count_lines(path: &Path) -> Result<usize> {
    let mut file = File::open(path)?;
    let mut buf = [0u8; 128 * 1024];
    let mut count = 0usize;
    let mut last: Option<u8> = None;

    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        count += memchr_iter(b'\n', &buf[..n]).count();
        last = Some(buf[n - 1]);
    }

    if let Some(b) = last {
        if b != b'\n' {
            count += 1;
        }
    }
    Ok(count)
}

pub fn write_results(
    output_path: &Path,
    aggregated_data: &HashMap<(Vec<u8>, String, u8), UserStats>,
) -> Result<()> {
    let mut sorted_entries: Vec<_> = aggregated_data.iter().collect();
    sorted_entries.sort_by(|a, b| {
        let (path_a, user_a, age_a) = &a.0;
        let (path_b, user_b, age_b) = &b.0;
        path_a
            .cmp(path_b)
            .then_with(|| user_a.cmp(user_b))
            .then_with(|| age_a.cmp(age_b))
    });

    let mut writer = WriterBuilder::new()
        .has_headers(true)
        .from_path(output_path)?;

    writer.write_record(&[
        "path",
        "user",
        "age",
        "files",
        "size",
        "disk",
        "linked",
        "accessed",
        "modified",
    ])?;

    for ((path_bytes, user, age), stats) in sorted_entries {
        let path_str = bytes_to_safe_string(path_bytes);
        writer.write_record(&[
            &path_str,
            user,
            &age.to_string(),
            &stats.file_count.to_string(),
            &stats.file_size.to_string(),
            &stats.disk_size.to_string(),
            &stats.linked_size.to_string(),
            &stats.latest_atime.to_string(),
            &stats.latest_mtime.to_string(),
        ])?;
    }

    writer.flush()?;
    Ok(())
}

pub fn write_unknown_uids(unk_path: &Path, unk_uids: &HashSet<u32>) -> Result<()> {
    let mut list: Vec<u32> = unk_uids.iter().copied().collect();
    list.sort_unstable();

    let mut wtr = WriterBuilder::new()
        .has_headers(false)
        .from_path(unk_path)?;

    for uid in list {
        wtr.write_record(&[uid.to_string()])?;
    }
    wtr.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write as IoWrite;
    use tempfile::NamedTempFile;

    #[test]
    fn count_lines_empty() {
        let f = NamedTempFile::new().unwrap();
        assert_eq!(count_lines(f.path()).unwrap(), 0);
    }

    #[test]
    fn count_lines_no_trailing_newline() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "a\nb\nc").unwrap();
        assert_eq!(count_lines(f.path()).unwrap(), 3);
    }

    #[test]
    fn count_lines_with_trailing_newline() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "a\nb\nc\n").unwrap();
        assert_eq!(count_lines(f.path()).unwrap(), 3);
    }

    #[test]
    fn write_results_emits_utf8_paths() {
        let mut map: HashMap<(Vec<u8>, String, u8), UserStats> = HashMap::new();
        let key = (vec![b'/', 0xFFu8, b'a'], "user".to_string(), 0u8);
        let mut s = UserStats::default();
        s.update(512, 512, 0, 1_700_000_000, 1_700_000_000);
        map.insert(key, s);

        let tmp = std::env::temp_dir().join(format!("sum_out_{}.csv", std::process::id()));
        let _ = fs::remove_file(&tmp);
        write_results(&tmp, &map).unwrap();

        let contents = fs::read_to_string(&tmp).unwrap();
        fs::remove_file(&tmp).ok();

        assert!(contents.contains('�'));
        assert!(contents.contains("/a") || contents.contains("/�a"));
        assert!(contents
            .lines()
            .next()
            .unwrap()
            .contains("path,user,age,files,size,disk,linked,accessed,modified"));
    }

    #[test]
    fn write_results_is_sorted_by_path_user_age() {
        let mut map: HashMap<(Vec<u8>, String, u8), UserStats> = HashMap::new();

        map.insert(
            (b"/a/b".to_vec(), "user2".to_string(), 1),
            UserStats {
                file_count: 2,
                file_size: 200,
                disk_size: 200,
                linked_size: 0,
                latest_atime: 20,
                latest_mtime: 20,
            },
        );
        map.insert(
            (b"/a".to_vec(), "user1".to_string(), 0),
            UserStats {
                file_count: 1,
                file_size: 100,
                disk_size: 100,
                linked_size: 0,
                latest_atime: 20,
                latest_mtime: 10,
            },
        );
        map.insert(
            (b"/a".to_vec(), "user0".to_string(), 2),
            UserStats {
                file_count: 3,
                file_size: 300,
                disk_size: 300,
                linked_size: 0,
                latest_atime: 20,
                latest_mtime: 30,
            },
        );

        let tmp = NamedTempFile::new().unwrap();
        write_results(tmp.path(), &map).unwrap();

        let contents = fs::read_to_string(tmp.path()).unwrap();
        let mut lines = contents.lines();
        assert_eq!(
            lines.next().unwrap(),
            "path,user,age,files,size,disk,linked,accessed,modified"
        );

        let row1 = lines.next().unwrap().to_string();
        let row2 = lines.next().unwrap().to_string();
        let row3 = lines.next().unwrap().to_string();

        assert!(row1.starts_with("/a,user0,2,"), "got: {row1}");
        assert!(row2.starts_with("/a,user1,0,"), "got: {row2}");
        assert!(row3.starts_with("/a/b,user2,1,"), "got: {row3}");

        assert!(lines.next().is_none());
    }

    #[test]
    fn write_results_includes_all_fields() {
        let mut map: HashMap<(Vec<u8>, String, u8), UserStats> = HashMap::new();
        map.insert(
            (b"/test".to_vec(), "testuser".to_string(), 1),
            UserStats {
                file_count: 5,
                file_size: 1000,
                disk_size: 800,
                linked_size: 200,
                latest_atime: 1234567890,
                latest_mtime: 1234567900,
            },
        );

        let tmp = NamedTempFile::new().unwrap();
        write_results(tmp.path(), &map).unwrap();

        let contents = fs::read_to_string(tmp.path()).unwrap();
        let mut lines = contents.lines();
        lines.next();

        let data_line = lines.next().unwrap();
        assert_eq!(
            data_line,
            "/test,testuser,1,5,1000,800,200,1234567890,1234567900"
        );
    }

    #[test]
    fn write_unknown_uids_is_sorted() {
        let tmp = NamedTempFile::new().unwrap();
        let mut set = std::collections::HashSet::new();
        set.insert(42);
        set.insert(7);
        set.insert(1000);

        write_unknown_uids(tmp.path(), &set).unwrap();
        let s = fs::read_to_string(tmp.path()).unwrap();
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines, vec!["7", "42", "1000"]);
    }

    #[test]
    fn write_unknown_uids_empty_set() {
        let tmp = NamedTempFile::new().unwrap();
        let set = std::collections::HashSet::new();

        write_unknown_uids(tmp.path(), &set).unwrap();
        let s = fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(s, "");
    }
}
