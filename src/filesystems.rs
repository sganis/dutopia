// rs/src/filesystems.rs
//! The filesystems (mount points) duscan scans, from the `FILESYSTEMS` env:
//! `FILESYSTEMS=/red/nas1,/red/nas2`. The DB itself is only a path tree from
//! `/`; this list is what turns `/red/nas1` into a filesystem, for the home
//! page and for the trash (`<fs>/.trash`).

/// Parse a comma-separated list. Entries are trimmed, trailing `/` dropped
/// (`/` itself stays `/`), duplicates removed; non-absolute entries are
/// skipped with a warning.
pub fn parse(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for entry in raw.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        if !entry.starts_with('/') {
            tracing::warn!(entry = %entry, "FILESYSTEMS: skipping non-absolute entry");
            continue;
        }
        let fs = match entry.trim_end_matches('/') {
            "" => "/".to_string(),
            s => s.to_string(),
        };
        if !out.contains(&fs) {
            out.push(fs);
        }
    }
    out
}

/// `parse` of the `FILESYSTEMS` env var (empty when unset).
pub fn from_env() -> Vec<String> {
    parse(&std::env::var("FILESYSTEMS").unwrap_or_default())
}

/// The filesystem holding `path`: the longest listed entry that is `path`
/// itself or a `/`-bounded prefix of it (same rule as
/// `authz::path_in_roots`, so `/red/nas1` does not hold `/red/nas10`).
pub fn fs_of<'a>(filesystems: &'a [String], path: &str) -> Option<&'a str> {
    filesystems
        .iter()
        .filter(|fs| crate::authz::path_in_roots(std::slice::from_ref(*fs), path))
        .max_by_key(|fs| fs.len())
        .map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_trims_and_drops_trailing_slash() {
        assert_eq!(parse(" /red/nas1/ , /red/nas2"), v(&["/red/nas1", "/red/nas2"]));
    }

    #[test]
    fn parse_skips_empty_relative_and_duplicates() {
        assert_eq!(parse(",, red/x ,/a,/a/,"), v(&["/a"]));
        assert!(parse("").is_empty());
    }

    #[test]
    fn parse_keeps_root() {
        assert_eq!(parse("/"), v(&["/"]));
    }

    #[test]
    fn fs_of_longest_prefix_wins() {
        let fss = v(&["/red", "/red/nas1"]);
        assert_eq!(fs_of(&fss, "/red/nas1/a/b"), Some("/red/nas1"));
        assert_eq!(fs_of(&fss, "/red/nas1"), Some("/red/nas1"));
        assert_eq!(fs_of(&fss, "/red/other"), Some("/red"));
    }

    #[test]
    fn fs_of_respects_slash_boundary() {
        let fss = v(&["/red/nas1"]);
        assert_eq!(fs_of(&fss, "/red/nas10/x"), None);
        assert_eq!(fs_of(&fss, "/red"), None);
        assert_eq!(fs_of(&fss, "/opt/x"), None);
    }

    #[test]
    fn fs_of_root_holds_everything() {
        let fss = v(&["/"]);
        assert_eq!(fs_of(&fss, "/opt/x"), Some("/"));
    }
}
