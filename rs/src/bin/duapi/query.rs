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

/// Normalize a user-supplied path. Returns `Some(clean)` where `clean` always starts with `/`,
/// with empty and `.` segments dropped and repeated/trailing `/` collapsed. Returns `None` if
/// the input contains a NUL byte or would escape via `..`.
pub fn normalize_path(input: &str) -> Option<String> {
    if input.as_bytes().contains(&0) {
        return None;
    }
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return Some("/".to_string());
    }
    let mut out: Vec<&str> = Vec::new();
    for seg in trimmed.split('/') {
        match seg {
            "" | "." => continue,
            ".." => return None,
            s => out.push(s),
        }
    }
    if out.is_empty() {
        Some("/".to_string())
    } else {
        Some(format!("/{}", out.join("/")))
    }
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
        assert_eq!(normalize_path("").as_deref(), Some("/"));
        assert_eq!(normalize_path("/var/log").as_deref(), Some("/var/log"));
        assert_eq!(normalize_path("var/log").as_deref(), Some("/var/log"));
    }

    #[test]
    fn test_normalize_path_collapses_slashes_and_dots() {
        assert_eq!(normalize_path("//var//log/").as_deref(), Some("/var/log"));
        assert_eq!(normalize_path("/var/./log").as_deref(), Some("/var/log"));
        assert_eq!(normalize_path("/var/log/.").as_deref(), Some("/var/log"));
    }

    #[test]
    fn test_normalize_path_rejects_traversal() {
        assert!(normalize_path("/var/../etc/passwd").is_none());
        assert!(normalize_path("..").is_none());
        assert!(normalize_path("/a/b/../../c").is_none());
        assert!(normalize_path("/a/%2e%2e/b").as_deref() == Some("/a/%2e%2e/b"));
        // ^ percent-decoding is axum's job; we only block literal ".." segments.
    }

    #[test]
    fn test_normalize_path_rejects_nul_byte() {
        assert!(normalize_path("/var/log\0/etc").is_none());
    }

    #[test]
    fn test_normalize_path_trims_whitespace() {
        assert_eq!(normalize_path("  /var/log  ").as_deref(), Some("/var/log"));
        assert_eq!(normalize_path("   ").as_deref(), Some("/"));
    }

    #[test]
    fn test_max_page_size_default() {
        // SAFETY: serial_test would isolate, but for a default-only check we just unset.
        unsafe { std::env::remove_var("MAX_PAGE_SIZE") };
        assert_eq!(max_page_size(), 2000);
    }
}
