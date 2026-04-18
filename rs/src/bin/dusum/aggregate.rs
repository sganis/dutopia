// rs/src/bin/dusum/aggregate.rs
use std::collections::HashMap;

#[cfg(unix)]
use std::ffi::CStr;

/// Pick the native separator byte for a raw path. A backslash anywhere in the
/// path (or a drive-letter prefix) means Windows-native; otherwise Unix.
fn separator(path: &[u8]) -> u8 {
    if path.iter().any(|&b| b == b'\\') {
        return b'\\';
    }
    if path.len() >= 2 && path[0].is_ascii_alphabetic() && path[1] == b':' {
        return b'\\';
    }
    b'/'
}

/// Trim trailing separator characters, preserving OS-native form.
///   `C:\Users\Default\` → `C:\Users\Default`
///   `/var/log/`         → `/var/log`
///   `C:\`               → `C:\`   (drive root keeps the trailing sep)
///   `/`                 → `/`     (root keeps the separator)
pub fn normalize_folder_bytes(path: &[u8]) -> Vec<u8> {
    let sep = separator(path);
    let mut out: Vec<u8> = path.to_vec();

    // Protect drive-letter roots: a lone `C:\` or `C:` stays as-is.
    let is_drive_root = out.len() <= 3
        && out.get(0).map_or(false, |b| b.is_ascii_alphabetic())
        && out.get(1) == Some(&b':');

    while out.len() > 1 && !is_drive_root && out.last() == Some(&sep) {
        out.pop();
    }
    out
}

/// Convert a file path into its list of ancestor folder paths, outer→inner,
/// preserving OS-native separators.
///   `/a/b/file.txt`         → [`/`, `/a`, `/a/b`]
///   `C:\Users\San\foo.txt`  → [`C:\`, `C:\Users`, `C:\Users\San`]
///   `\\srv\shr\dir\f.txt`   → [`\\srv`, `\\srv\shr`, `\\srv\shr\dir`]
pub fn get_folder_ancestors(path: &[u8]) -> Vec<Vec<u8>> {
    let sep = separator(path);

    let parent_end = path.iter().rposition(|&b| b == sep);

    let folder = match parent_end {
        None => return Vec::new(),
        Some(0) => {
            // File lives directly under root ("/" on Unix, or "\" which only
            // arises in malformed Windows input).
            return vec![vec![sep]];
        }
        Some(pos) => &path[..pos],
    };

    let mut folder = folder.to_vec();
    while folder.len() > 1 && folder.last() == Some(&sep) {
        folder.pop();
    }

    let mut ancestors: Vec<Vec<u8>> = Vec::new();

    // Unix absolute path: "/…" — root `/` is itself an ancestor.
    // UNC: "\\server\share\…" — server + share are the first two ancestors.
    // Drive-letter: "C:\…" — the drive root "C:\" is the first ancestor.
    // Relative: no anchor; first segment is the first ancestor.
    let (prefix, rest): (Vec<u8>, &[u8]) = if sep == b'/' && folder.starts_with(b"/") {
        ancestors.push(b"/".to_vec());
        (b"/".to_vec(), &folder[1..])
    } else if sep == b'\\' && folder.starts_with(b"\\\\") {
        (b"\\\\".to_vec(), &folder[2..])
    } else if sep == b'\\'
        && folder.len() >= 2
        && folder[0].is_ascii_alphabetic()
        && folder[1] == b':'
    {
        // Build the drive root `C:\` as the first ancestor.
        let drive_root = vec![folder[0], b':', b'\\'];
        ancestors.push(drive_root.clone());
        let rest = if folder.len() >= 3 && folder[2] == b'\\' {
            &folder[3..]
        } else {
            &folder[2..]
        };
        (drive_root, rest)
    } else {
        (Vec::new(), &folder[..])
    };

    let mut current: Vec<u8> = prefix;
    for segment in rest.split(|&b| b == sep).filter(|s| !s.is_empty()) {
        if !current.is_empty() && current.last() != Some(&sep) {
            current.push(sep);
        }
        current.extend_from_slice(segment);
        ancestors.push(current.clone());
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
pub fn get_username_from_uid(_uid: u32) -> String {
    // Windows duscan does not record per-file ownership yet (uid is always 0).
    // Use the interactive user as a stand-in so the duapi per-user filter has
    // something that matches the login identity. Falls back to "UNK" if the
    // environment is missing USERNAME (e.g. running as a service).
    std::env::var("USERNAME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "UNK".to_string())
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
    fn ancestors_windows_native_backslashes() {
        let res = get_folder_ancestors(b"C:\\a\\b\\file.txt");
        assert_eq!(
            res,
            vec![
                b"C:\\".to_vec(),
                b"C:\\a".to_vec(),
                b"C:\\a\\b".to_vec()
            ]
        );
    }

    #[test]
    fn ancestors_unc_native() {
        let res = get_folder_ancestors(b"\\\\server\\share\\dir\\file.txt");
        assert_eq!(
            res,
            vec![
                b"\\\\server".to_vec(),
                b"\\\\server\\share".to_vec(),
                b"\\\\server\\share\\dir".to_vec(),
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
        // No path separator at all: no ancestor folders.
        let res = get_folder_ancestors(b"file.txt");
        assert!(res.is_empty());
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

    #[test]
    fn ancestors_drive_root_file() {
        let res = get_folder_ancestors(b"C:\\foo.txt");
        assert_eq!(res, vec![b"C:\\".to_vec()]);
    }

    #[test]
    fn normalize_folder_bytes_basic() {
        assert_eq!(normalize_folder_bytes(b"/a/b"), b"/a/b".to_vec());
        assert_eq!(normalize_folder_bytes(b"/a/b/"), b"/a/b".to_vec());
        assert_eq!(normalize_folder_bytes(b"a/b"), b"a/b".to_vec());
        assert_eq!(normalize_folder_bytes(b"/"), b"/".to_vec());
        assert_eq!(normalize_folder_bytes(b""), Vec::<u8>::new());
    }

    #[test]
    fn normalize_folder_bytes_windows_native() {
        assert_eq!(
            normalize_folder_bytes(b"C:\\Users\\Default"),
            b"C:\\Users\\Default".to_vec()
        );
        assert_eq!(
            normalize_folder_bytes(b"C:\\Users\\Default\\"),
            b"C:\\Users\\Default".to_vec()
        );
        // Drive root keeps its trailing backslash.
        assert_eq!(normalize_folder_bytes(b"C:\\"), b"C:\\".to_vec());
    }
}
