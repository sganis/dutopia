// rs/src/bin/dusum/aggregate.rs
use std::collections::HashMap;

#[cfg(unix)]
use std::ffi::CStr;

/// Convert path bytes into a list of ancestor folder paths:
///  "/a/b/file.txt" -> ["/", "/a", "/a/b"]
pub fn get_folder_ancestors(path: &[u8]) -> Vec<Vec<u8>> {
    let normalized: Vec<u8> = path
        .iter()
        .map(|&b| if b == b'\\' { b'/' } else { b })
        .collect();

    let parent_end = normalized.iter().rposition(|&b| b == b'/');

    let folder = match parent_end {
        Some(0) | None => return vec![b"/".to_vec()],
        Some(pos) => &normalized[..pos],
    };

    let mut folder = folder.to_vec();
    while folder.len() > 1 && folder.last() == Some(&b'/') {
        folder.pop();
    }

    let mut ancestors = vec![b"/".to_vec()];

    let trimmed = if folder.starts_with(&[b'/']) {
        &folder[1..]
    } else {
        &folder[..]
    };
    if trimmed.is_empty() {
        return ancestors;
    }

    let mut current_path = Vec::new();
    current_path.push(b'/');

    for segment in trimmed.split(|&b| b == b'/').filter(|s| !s.is_empty()) {
        if current_path.len() > 1 {
            current_path.push(b'/');
        }
        current_path.extend_from_slice(segment);
        ancestors.push(current_path.clone());
    }

    ancestors
}

/// Safely convert bytes to UTF-8 String (invalid sequences -> U+FFFD)
pub fn bytes_to_safe_string(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

pub fn resolve_user(uid: u32, cache: &mut HashMap<u32, String>) -> String {
    if let Some(u) = cache.get(&uid) {
        return u.clone();
    }
    let name = get_username_from_uid(uid);
    cache.insert(uid, name.clone());
    name
}

#[cfg(unix)]
pub fn get_username_from_uid(uid: u32) -> String {
    unsafe {
        let passwd = libc::getpwuid(uid);
        if passwd.is_null() {
            return "UNK".to_string();
        }
        let name_ptr = (*passwd).pw_name;
        if name_ptr.is_null() {
            return "UNK".to_string();
        }
        match CStr::from_ptr(name_ptr).to_str() {
            Ok(name) => name.to_string(),
            Err(_) => "UNK".to_string(),
        }
    }
}

#[cfg(not(unix))]
pub fn get_username_from_uid(uid: u32) -> String {
    uid.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_to_safe_string_handles_invalid_utf8() {
        let bad = [0xFFu8, b'a', 0xFE, b'b'];
        let s = bytes_to_safe_string(&bad);
        assert!(s.contains('�'));
        assert!(s.contains('a') && s.contains('b'));
    }

    #[test]
    fn bytes_to_safe_string_always_utf8() {
        let raw = [b'/', 0xFFu8, b'a', b'/', 0xFE, b'b'];
        let s = bytes_to_safe_string(&raw);
        assert!(s.is_char_boundary(s.len()));
        assert!(s.contains('�'));
        assert!(s.contains("/"));
    }

    #[test]
    fn ancestors_from_non_utf8_bytes() {
        let raw = [
            b'/', 0xFFu8, b'a', b'/', b'b', b'/', b'c', b'/', b'f', b'.', b't', b'x', b't',
        ];
        let ancestors = get_folder_ancestors(&raw);
        assert_eq!(ancestors[0], b"/".to_vec());
        assert!(ancestors.contains(&vec![b'/', 0xFFu8, b'a']));
        assert!(ancestors.contains(&vec![b'/', 0xFFu8, b'a', b'/', b'b']));
    }

    #[test]
    fn ancestors_trailing_slashes_and_multi_seps() {
        let res = get_folder_ancestors(b"/a//b///c//file.txt");
        assert_eq!(
            res,
            vec![
                b"/".to_vec(),
                b"/a".to_vec(),
                b"/a/b".to_vec(),
                b"/a/b/c".to_vec()
            ]
        );
    }

    #[test]
    fn ancestors_windows_backslashes_normalized() {
        let res = get_folder_ancestors(b"C:\\a\\b\\file.txt");
        assert_eq!(
            res,
            vec![
                b"/".to_vec(),
                b"/C:".to_vec(),
                b"/C:/a".to_vec(),
                b"/C:/a/b".to_vec()
            ]
        );
    }

    #[test]
    fn ancestors_handles_root_only() {
        assert_eq!(get_folder_ancestors(b"/file.txt"), vec![b"/".to_vec()]);
        assert_eq!(get_folder_ancestors(b"/"), vec![b"/".to_vec()]);
    }

    #[test]
    fn ancestors_handles_relative_paths() {
        let res = get_folder_ancestors(b"file.txt");
        assert_eq!(res, vec![b"/".to_vec()]);
    }

    #[test]
    fn ancestors_simple_nested_path() {
        let res = get_folder_ancestors(b"/a/b/c/file.txt");
        assert_eq!(
            res,
            vec![
                b"/".to_vec(),
                b"/a".to_vec(),
                b"/a/b".to_vec(),
                b"/a/b/c".to_vec()
            ]
        );
    }

    #[test]
    fn ancestors_single_level_path() {
        let res = get_folder_ancestors(b"/a/file.txt");
        assert_eq!(res, vec![b"/".to_vec(), b"/a".to_vec()]);
    }
}
