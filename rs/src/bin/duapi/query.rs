// rs/src/bin/duapi/query.rs
use serde::Deserialize;

pub use dutopia::query::normalize_path;

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
    fn test_max_page_size_default() {
        unsafe { std::env::remove_var("MAX_PAGE_SIZE") };
        assert_eq!(max_page_size(), 2000);
    }
}
