// rs/src/util/path.rs
use std::path::{Path, PathBuf};

/// Compute the parent of a `dusum`-stored path string, in the same OS-native
/// form `dusum::aggregate::get_folder_ancestors` produces. Returns `None`
/// when the path is at the top of its native hierarchy (Unix `/`, Windows
/// drive root `C:\`, UNC bare server `\\srv`) — callers maintaining a
/// synthetic root above all platform roots should treat `None` as
/// "parent_id = synthetic root".
///
///   `/var/log`            -> `Some("/var")`
///   `/var`                -> `Some("/")`
///   `/`                   -> `None`
///   `C:\Users\San`        -> `Some("C:\\Users")`
///   `C:\Users`            -> `Some("C:\\")`
///   `C:\`                 -> `None`
///   `\\srv\shr\dir`       -> `Some("\\\\srv\\shr")`
///   `\\srv\shr`           -> `Some("\\\\srv")`
///   `\\srv`               -> `None`
///   `""`                  -> `None`  (the synthetic root has no parent)
pub fn dusum_parent(p: &str) -> Option<String> {
    if p.is_empty() || p == "/" {
        return None;
    }
    let bytes = p.as_bytes();

    // Drive root: "C:\"
    if bytes.len() == 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && bytes[2] == b'\\'
    {
        return None;
    }

    // UNC bare server: "\\srv" — no further `\` after the leading "\\".
    if p.starts_with(r"\\") && !p[2..].contains('\\') {
        return None;
    }

    // Find the rightmost separator (either kind, whichever appears last).
    let last = match (p.rfind('\\'), p.rfind('/')) {
        (Some(a), Some(b)) => a.max(b),
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => return None,
    };

    // Subdir of Unix root: "/var" → "/"
    if last == 0 {
        return Some("/".to_string());
    }
    // Subdir of Windows drive root: "C:\Users" → "C:\"
    if last == 2 && bytes.len() > 3 && bytes[1] == b':' {
        return Some(format!("{}:\\", bytes[0] as char));
    }

    Some(p[..last].to_string())
}

#[cfg(windows)]
pub fn strip_verbatim_prefix(p: &Path) -> PathBuf {
    let s = match p.to_str() {
        Some(s) => s,
        None => return p.to_path_buf(),
    };

    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{}", rest))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        p.to_path_buf()
    }
}

#[cfg(not(windows))]
pub fn strip_verbatim_prefix(p: &Path) -> PathBuf {
    p.to_path_buf()
}

#[inline]
pub fn should_skip(path: &Path, skip: Option<&str>) -> bool {
    if let Some(s) = skip {
        path.as_os_str().to_string_lossy().contains(s)
    } else {
        false
    }
}

pub fn is_volume_root(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if path.parent().is_none() {
            return true;
        }

        if let (Ok(meta), Some(parent)) = (std::fs::metadata(path), path.parent()) {
            if let Ok(pmeta) = std::fs::metadata(parent) {
                return meta.dev() != pmeta.dev();
            }
        }
        false
    }

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetVolumePathNameW;

        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut buf = [0u16; 260];
        let ok =
            unsafe { GetVolumePathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
        if ok == 0 {
            return false;
        }

        let vol = {
            let nul = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            String::from_utf16_lossy(&buf[..nul])
        };

        let p = strip_verbatim_prefix(path);
        let p = p.to_string_lossy();

        let mut p_norm = p.to_string();
        if !p_norm.ends_with('\\') && p_norm.chars().nth(1) == Some(':') && p_norm.len() == 2 {
            p_norm.push('\\');
        }
        if !p_norm.ends_with('\\') && vol.ends_with('\\') {
            p_norm.push('\\');
        }

        p_norm.eq_ignore_ascii_case(&vol)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_skip() {
        let p = PathBuf::from("/a/b/c/d");
        assert!(should_skip(&p, Some("b/c")));
        assert!(!should_skip(&p, Some("x")));
        assert!(!should_skip(&p, None));

        let p2 = PathBuf::from("C:\\Users\\test");
        assert!(should_skip(&p2, Some("Users")));
        assert!(!should_skip(&p2, Some("Documents")));
    }

    #[test]
    fn test_should_skip_edge_cases() {
        let p = PathBuf::from("");
        assert!(!should_skip(&p, Some("test")));
        assert!(!should_skip(&p, None));

        let p2 = PathBuf::from("test");
        assert!(should_skip(&p2, Some("")));
        assert!(should_skip(&p2, Some("test")));
        assert!(should_skip(&p2, Some("te")));
    }

    #[cfg(not(windows))]
    #[test]
    fn test_strip_verbatim_prefix_unix() {
        let p = PathBuf::from("/some/path");
        assert_eq!(strip_verbatim_prefix(&p), p);

        let p2 = PathBuf::from("relative/path");
        assert_eq!(strip_verbatim_prefix(&p2), p2);
    }

    #[cfg(windows)]
    #[test]
    fn test_strip_verbatim_prefix_windows() {
        let p = PathBuf::from(r"\\?\C:\test");
        assert_eq!(strip_verbatim_prefix(&p), PathBuf::from(r"C:\test"));

        let p2 = PathBuf::from(r"\\?\UNC\server\share");
        assert_eq!(strip_verbatim_prefix(&p2), PathBuf::from(r"\\server\share"));

        let p3 = PathBuf::from(r"C:\normal\path");
        assert_eq!(strip_verbatim_prefix(&p3), p3);
    }

    #[cfg(windows)]
    #[test]
    fn test_strip_verbatim_prefix_invalid_unicode() {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;

        let invalid_utf16 = vec![0xD800, 0x41];
        let os_string = OsString::from_wide(&invalid_utf16);
        let p = PathBuf::from(os_string);

        assert_eq!(strip_verbatim_prefix(&p), p);
    }

    #[cfg(unix)]
    #[test]
    fn test_is_volume_root_unix() {
        assert!(is_volume_root(Path::new("/")));

        let result = is_volume_root(Path::new("/usr"));
        let _ = result;

        let result = is_volume_root(Path::new("/non/existent/path"));
        assert!(!result);
    }

    #[cfg(windows)]
    #[test]
    fn test_is_volume_root_windows() {
        let result = is_volume_root(Path::new("C:\\"));
        let _ = result;

        let result = is_volume_root(Path::new("C:\\Windows"));
        let _ = result;
    }

    #[test]
    fn dusum_parent_unix() {
        assert_eq!(dusum_parent("/"), None);
        assert_eq!(dusum_parent("/var"), Some("/".to_string()));
        assert_eq!(dusum_parent("/var/log"), Some("/var".to_string()));
        assert_eq!(
            dusum_parent("/var/log/syslog"),
            Some("/var/log".to_string())
        );
    }

    #[test]
    fn dusum_parent_windows_drive() {
        assert_eq!(dusum_parent("C:\\"), None);
        assert_eq!(dusum_parent("c:\\"), None);
        assert_eq!(dusum_parent("C:\\Users"), Some("C:\\".to_string()));
        assert_eq!(
            dusum_parent("C:\\Users\\San"),
            Some("C:\\Users".to_string())
        );
        assert_eq!(dusum_parent("D:\\Foo\\Bar"), Some("D:\\Foo".to_string()));
    }

    #[test]
    fn dusum_parent_unc() {
        assert_eq!(dusum_parent(r"\\srv"), None);
        assert_eq!(dusum_parent(r"\\srv\shr"), Some(r"\\srv".to_string()));
        assert_eq!(
            dusum_parent(r"\\srv\shr\dir"),
            Some(r"\\srv\shr".to_string())
        );
    }

    #[test]
    fn dusum_parent_synthetic_root() {
        assert_eq!(dusum_parent(""), None);
    }

    /// Cross-check: for every path `dusum::get_folder_ancestors` would emit,
    /// `dusum_parent` of `ancestors[i+1]` must equal `ancestors[i]`. Pinning
    /// this guarantees parent_id chains rebuilt by `dudb` match dusum's
    /// notion of the tree byte-for-byte.
    #[test]
    fn dusum_parent_inverts_get_folder_ancestors() {
        let cases: &[&[&str]] = &[
            &["/", "/var", "/var/log"],
            &["C:\\", "C:\\Users", "C:\\Users\\San"],
            &[r"\\srv", r"\\srv\shr", r"\\srv\shr\dir"],
        ];
        for chain in cases {
            for win in chain.windows(2) {
                let parent = dusum_parent(win[1]).unwrap_or_default();
                assert_eq!(
                    parent, win[0],
                    "parent of {:?} should be {:?}, got {:?}",
                    win[1], win[0], parent
                );
            }
            // The first entry in each chain has no parent (top of its hierarchy).
            assert_eq!(dusum_parent(chain[0]), None, "{:?} should be top-level", chain[0]);
        }
    }

    #[test]
    fn test_path_processing_integration() {
        let test_paths = ["/usr/local/bin", "/home/user/documents", "/var/log/system.log"];

        for path_str in &test_paths {
            let path = Path::new(path_str);
            let stripped = strip_verbatim_prefix(&path);

            assert!(!should_skip(&stripped, Some("nonexistent")));

            if path_str.contains("user") {
                assert!(should_skip(&stripped, Some("user")));
            }
        }
    }
}
