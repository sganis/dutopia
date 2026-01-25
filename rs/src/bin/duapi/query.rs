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
}
