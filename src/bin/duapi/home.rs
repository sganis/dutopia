// rs/src/bin/duapi/home.rs
//
// Home page when `FILESYSTEMS` is set: `GET /api/folders?path=` (the SPA's
// synthetic root "") lists one row per filesystem instead of `/`, so users
// land on `/red/nas1` and `/red/nas2` without clicking `/` → `/red` first.

use dutopia::auth::Claims;
use dutopia::authz::{path_in_roots, AccessMode};

/// The filesystem rows to show on the home page and the user filter to
/// apply, given the outcome of `authz::check_access` for path "".
///
/// - `Full` (global admin, or a caller asking for their own usage): every
///   filesystem, filtered to the requested users.
/// - `Navigable` (storage admin browsing with other users selected): only
///   the filesystems that hold one of their roots or lie inside one, with
///   the users filter dropped — the same view `/api/folders` gives a storage
///   admin on any ancestor of their roots.
pub fn rows_for(
    claims: &Claims,
    filesystems: &[String],
    access: &AccessMode,
    requested: Vec<String>,
) -> (Vec<String>, Vec<String>) {
    match access {
        AccessMode::Full => (filesystems.to_vec(), requested),
        AccessMode::Navigable => {
            let rows = filesystems
                .iter()
                .filter(|fs| {
                    path_in_roots(&claims.roots, fs)
                        || claims
                            .roots
                            .iter()
                            .any(|r| path_in_roots(std::slice::from_ref(*fs), r))
                })
                .cloned()
                .collect();
            (rows, Vec::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(is_admin: bool, roots: &[&str]) -> Claims {
        Claims {
            sub: "user1".into(),
            is_admin,
            roots: roots.iter().map(|s| s.to_string()).collect(),
            exp: 9_999_999_999,
        }
    }

    fn fss() -> Vec<String> {
        vec!["/red/nas1".into(), "/red/nas2".into(), "/red/ssd".into()]
    }

    #[test]
    fn full_access_lists_every_filesystem_with_requested_users() {
        let c = claims(false, &[]);
        let (rows, users) = rows_for(&c, &fss(), &AccessMode::Full, vec!["user1".into()]);
        assert_eq!(rows, fss());
        assert_eq!(users, vec!["user1".to_string()]);
    }

    #[test]
    fn navigable_lists_filesystems_holding_or_inside_roots() {
        // Root is a whole filesystem, another root is a folder inside one.
        let c = claims(false, &["/red/nas1", "/red/ssd/projA"]);
        let (rows, users) = rows_for(&c, &fss(), &AccessMode::Navigable, vec!["bob".into()]);
        assert_eq!(rows, vec!["/red/nas1".to_string(), "/red/ssd".to_string()]);
        assert!(users.is_empty());
    }

    #[test]
    fn navigable_root_above_filesystems_shows_them_all() {
        let c = claims(false, &["/red"]);
        let (rows, _) = rows_for(&c, &fss(), &AccessMode::Navigable, vec![]);
        assert_eq!(rows, fss());
    }

    #[test]
    fn navigable_prefix_trap() {
        let c = claims(false, &["/red/nas10"]);
        let (rows, _) = rows_for(&c, &fss(), &AccessMode::Navigable, vec![]);
        assert!(rows.is_empty());
    }
}
