// rs/src/bin/duapi/query.rs
use serde::Deserialize;

#[derive(Deserialize)]
pub struct FolderQuery {
    pub path: Option<String>,
    pub users: Option<String>,
    pub age: Option<u8>,
}

#[derive(Deserialize)]
pub struct FilesQuery {
    pub path: Option<String>,
    pub users: Option<String>,
    pub age: Option<u8>,
}

pub fn parse_users_csv(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(|p| p.to_string())
        .collect()
}

/// Normalize a user-supplied path to the native OS form used as the index key.
///
/// The path form is preserved: `/var/log` (Unix), `C:\Users\San` (Windows),
/// `\\server\share\dir` (UNC). The separator detected in the input is the
/// separator used in the output.
///
/// Returns `None` if the input contains a NUL byte or a literal `..` segment.
pub fn normalize_path(input: &str) -> Option<String> {
    if input.as_bytes().contains(&0) {
        return None;
    }
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Some(String::new());
    }
    let bytes = trimmed.as_bytes();

    // Detect native separator from the input.
    let is_unc = trimmed.starts_with(r"\\");
    let has_drive_prefix = bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':';
    let use_backslash = is_unc || has_drive_prefix || trimmed.contains('\\');
    let sep: char = if use_backslash { '\\' } else { '/' };
    let is_unix_absolute = !use_backslash && trimmed.starts_with('/');

    // Root markers — return canonical form directly.
    if trimmed == "/" {
        return Some("/".to_string());
    }
    if has_drive_prefix && bytes.len() <= 3 {
        return Some(format!("{}:\\", bytes[0] as char));
    }

    // Peel the anchor so we can split segments reliably.
    let body: &str = if is_unc {
        &trimmed[2..]
    } else if is_unix_absolute {
        &trimmed[1..]
    } else if has_drive_prefix {
        // `C:\Users\San` → split off `C:\` first.
        let start = if bytes.len() >= 3 && bytes[2] == b'\\' { 3 } else { 2 };
        &trimmed[start..]
    } else {
        trimmed
    };

    let mut out: Vec<&str> = Vec::new();
    for seg in body.split(sep) {
        match seg {
            "" | "." => continue,
            ".." => return None,
            s => out.push(s),
        }
    }

    Some(match (is_unc, has_drive_prefix, is_unix_absolute) {
        (true, _, _) => {
            if out.is_empty() {
                r"\\".to_string()
            } else {
                format!("\\\\{}", out.join("\\"))
            }
        }
        (_, true, _) => {
            let drive = format!("{}:\\", bytes[0] as char);
            if out.is_empty() {
                drive
            } else {
                format!("{}{}", drive, out.join("\\"))
            }
        }
        (_, _, true) => {
            if out.is_empty() {
                "/".to_string()
            } else {
                format!("/{}", out.join("/"))
            }
        }
        _ => out.join(&sep.to_string()),
    })
}

pub fn max_page_size() -> usize {
    std::env::var("MAX_PAGE_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_users_csv() {
        let v = parse_users_csv(" alice, bob ,, carol ");
        assert_eq!(v, vec!["alice", "bob", "carol"]);
        assert!(parse_users_csv(" ,, ,").is_empty());
    }

    #[test]
    fn test_parse_users_csv_empty() {
        assert!(parse_users_csv("").is_empty());
        assert!(parse_users_csv("   ").is_empty());
    }

    #[test]
    fn test_parse_users_csv_single() {
        let v = parse_users_csv("alice");
        assert_eq!(v, vec!["alice"]);
    }

    #[test]
    fn test_normalize_path_basic() {
        assert_eq!(normalize_path("/").as_deref(), Some("/"));
        assert_eq!(normalize_path("").as_deref(), Some(""));
        assert_eq!(normalize_path("/var/log").as_deref(), Some("/var/log"));
    }

    #[test]
    fn test_normalize_path_drive_letter_native() {
        assert_eq!(normalize_path("C:").as_deref(), Some("C:\\"));
        assert_eq!(normalize_path("C:\\").as_deref(), Some("C:\\"));
        assert_eq!(
            normalize_path("C:\\Dev\\foo").as_deref(),
            Some("C:\\Dev\\foo")
        );
        assert_eq!(
            normalize_path("C:\\Dev\\foo\\").as_deref(),
            Some("C:\\Dev\\foo")
        );
    }

    #[test]
    fn test_normalize_path_unc_native() {
        assert_eq!(
            normalize_path(r"\\server\share\dir").as_deref(),
            Some(r"\\server\share\dir")
        );
    }

    #[test]
    fn test_normalize_path_collapses_slashes_and_dots() {
        assert_eq!(normalize_path("/var//log/").as_deref(), Some("/var/log"));
        assert_eq!(normalize_path("/var/./log").as_deref(), Some("/var/log"));
        assert_eq!(normalize_path("/var/log/.").as_deref(), Some("/var/log"));
    }

    #[test]
    fn test_normalize_path_rejects_traversal() {
        assert!(normalize_path("/var/../etc/passwd").is_none());
        assert!(normalize_path("..").is_none());
        assert!(normalize_path("/a/b/../../c").is_none());
        // Percent-escaped `..` is axum's job to decode — we only block literal segments.
        assert!(normalize_path("/a/%2e%2e/b").is_some());
    }

    #[test]
    fn test_normalize_path_rejects_nul_byte() {
        assert!(normalize_path("/var/log\0/etc").is_none());
    }

    #[test]
    fn test_normalize_path_trims_whitespace() {
        assert_eq!(normalize_path("  /var/log  ").as_deref(), Some("/var/log"));
        assert_eq!(normalize_path("   ").as_deref(), Some(""));
    }

    #[test]
    fn test_max_page_size_default() {
        // SAFETY: serial_test would isolate, but for a default-only check we just unset.
        unsafe { std::env::remove_var("MAX_PAGE_SIZE") };
        assert_eq!(max_page_size(), 2000);
    }
}
