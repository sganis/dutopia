// rs/src/bin/duapi/item.rs
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[cfg(unix)]
use std::ffi::CStr;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsItemOut {
    pub path: String,
    pub owner: String,
    pub size: u64,
    pub accessed: i64,
    pub modified: i64,
}

#[cfg(unix)]
fn username_from_uid(uid: u32) -> String {
    unsafe {
        let pw = libc::getpwuid(uid);
        if pw.is_null() {
            return "UNK".to_string();
        }
        let name_ptr = (*pw).pw_name;
        if name_ptr.is_null() {
            return "UNK".to_string();
        }
        match CStr::from_ptr(name_ptr).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => "UNK".to_string(),
        }
    }
}

#[cfg(unix)]
fn cached_username(uid: u32, cache: &mut HashMap<u32, String>) -> String {
    if let Some(name) = cache.get(&uid) {
        return name.clone();
    }
    let name = username_from_uid(uid);
    cache.insert(uid, name.clone());
    name
}

#[cfg(unix)]
pub fn get_items<P: AsRef<std::path::Path>>(
    folder: P,
    usernames: &[String],
    age_filter: Option<u8>,
) -> Result<Vec<FsItemOut>> {
    use chrono::{Duration, Utc};
    use std::collections::HashSet;
    use std::fs;
    use std::os::unix::fs::MetadataExt;

    let filter: Option<HashSet<String>> = if usernames.is_empty() {
        None
    } else {
        Some(usernames.iter().cloned().collect())
    };

    let now = Utc::now();
    let cutoff_recent = (now - Duration::days(60)).timestamp();
    let cutoff_old = (now - Duration::days(730)).timestamp();

    let mut out = Vec::new();
    let mut uid_cache: HashMap<u32, String> = HashMap::new();

    let dir = fs::read_dir(&folder)
        .with_context(|| format!("read_dir({}) failed", folder.as_ref().display()))?;

    for entry_res in dir {
        let entry = match entry_res {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();

        let md = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !md.file_type().is_file() {
            continue;
        }

        let owner = cached_username(md.uid(), &mut uid_cache);
        if let Some(ref allow) = filter {
            if !allow.contains(&owner) {
                continue;
            }
        }

        let atime = md.atime();
        let mtime = md.mtime();

        if let Some(af) = age_filter {
            let age = if mtime >= cutoff_recent {
                0
            } else if mtime < cutoff_old {
                2
            } else {
                1
            };
            if age != af {
                continue;
            }
        }

        out.push(FsItemOut {
            path: path.to_string_lossy().into_owned(),
            owner,
            size: md.size(),
            accessed: atime,
            modified: mtime,
        });
    }

    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

#[cfg(not(unix))]
pub fn get_items<P: AsRef<std::path::Path>>(
    _folder: P,
    _usernames: &[String],
    _age_filter: Option<u8>,
) -> Result<Vec<FsItemOut>> {
    anyhow::bail!("get_items is only implemented on Unix-like systems.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn test_get_items_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let items = get_items(tmp.path(), &[], None).unwrap();
        assert!(items.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn test_get_items_with_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), b"hello").unwrap();
        std::fs::write(tmp.path().join("b.txt"), b"world").unwrap();

        let items = get_items(tmp.path(), &[], None).unwrap();
        assert_eq!(items.len(), 2);
        assert!(items[0].path.ends_with("a.txt"));
        assert!(items[1].path.ends_with("b.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn test_get_items_skips_directories() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("file.txt"), b"content").unwrap();
        std::fs::create_dir(tmp.path().join("subdir")).unwrap();

        let items = get_items(tmp.path(), &[], None).unwrap();
        assert_eq!(items.len(), 1);
        assert!(items[0].path.ends_with("file.txt"));
    }

    #[cfg(not(unix))]
    #[test]
    fn test_get_items_not_unix() {
        let tmp = tempfile::tempdir().unwrap();
        let result = get_items(tmp.path(), &[], None);
        assert!(result.is_err());
    }
}
