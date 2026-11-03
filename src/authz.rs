// rs/src/authz.rs
//! Authorization tiers shared by duapi's REST handlers and MCP tools.
//!
//! Three tiers exist:
//! - **Global admin** (`is_admin = true`): unrestricted. Granted via the
//!   `GLOBAL_ADMINS` env var (comma-separated usernames, case-insensitive;
//!   the OIDC path additionally matches Keycloak `groups` entries).
//! - **Storage admin** (`is_admin = false`, non-empty `roots`): full admin
//!   inside each assigned root, regular self-only user elsewhere. Granted
//!   via `STORAGE_ADMINS`, e.g. `"user1:/red/peddn7,/red/peddn8"`.
//! - **Regular user**: may only query their own files (enforced at the
//!   handler layer).

use std::collections::HashMap;

use crate::auth::Claims;

/// Unit error for `check_access` denials.
#[derive(Debug, PartialEq, Eq)]
pub struct NotAllowed;

/// Outcome of `check_access`: `Full` proceeds with the requested users;
/// `Navigable` means the caller is a storage admin walking *towards* their
/// roots (e.g. the SPA's synthetic root `""` → `/red` → `/red/peddn8`) — the
/// request is allowed but folder children must be filtered to the route
/// into their roots and file listings come back empty.
#[derive(Debug, PartialEq, Eq)]
pub enum AccessMode {
    Full,
    Navigable,
}

/// Handler-layer access check for `/api/folders` and `/api/files`.
///
/// `Full` when:
/// - the caller is a global admin; or
/// - the caller is a storage admin and `path` is inside one of their roots
///   (any requested users are allowed there); or
/// - the caller requests exactly their own user (today's non-admin rule,
///   which storage admins fall back to outside their roots).
///
/// `Navigable` when the caller is a storage admin whose requested users
/// would otherwise be denied, but `path` is an ancestor of a root — the
/// SPA needs to walk from the top level into the assigned storage.
pub fn check_access(
    claims: &Claims,
    path: &str,
    requested: &[String],
) -> Result<AccessMode, NotAllowed> {
    if claims.is_admin {
        return Ok(AccessMode::Full);
    }
    if !claims.roots.is_empty() && path_in_roots(&claims.roots, path) {
        return Ok(AccessMode::Full);
    }
    if requested.len() == 1 && requested[0] == claims.sub {
        return Ok(AccessMode::Full);
    }
    if !claims.roots.is_empty() && path_on_route_to_roots(&claims.roots, path) {
        return Ok(AccessMode::Navigable);
    }
    Err(NotAllowed)
}

/// Read `GLOBAL_ADMINS`, falling back to the deprecated `ADMIN_GROUP` so
/// existing deployments keep working. Returns the raw value.
fn global_admins_env() -> String {
    match std::env::var("GLOBAL_ADMINS") {
        Ok(v) => v,
        Err(_) => {
            // Deprecated name kept for backwards compatibility.
            let legacy = std::env::var("ADMIN_GROUP").unwrap_or_default();
            if !legacy.trim().is_empty() {
                tracing::warn!(
                    "ADMIN_GROUP is deprecated, rename it to GLOBAL_ADMINS"
                );
            }
            legacy
        }
    }
}

/// Is this username a global admin? Comma-separated,
/// case-insensitive username match against `GLOBAL_ADMINS`.
pub fn is_global_admin(username: &str) -> bool {
    let name = username.trim().to_ascii_lowercase();
    if name.is_empty() {
        return false;
    }
    global_admins_env()
        .split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .any(|s| s == name)
}

/// Parse `STORAGE_ADMINS` into a username → roots map.
///
/// Format: `"user1:/red/peddn7,/red/peddn8;user2:/red/ssd"`. Entries without
/// a `:` or with an empty user are skipped. Usernames are lowercased
/// (consistent with `GLOBAL_ADMINS` matching); roots are trimmed as-is.
pub fn parse_storage_admins() -> HashMap<String, Vec<String>> {
    let mut map = HashMap::new();
    let raw = match std::env::var("STORAGE_ADMINS") {
        Ok(v) => v,
        Err(_) => return map,
    };
    for entry in raw.split(';') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let Some((user, roots)) = entry.split_once(':') else {
            tracing::warn!(entry = %entry, "STORAGE_ADMINS: skipping entry without ':'");
            continue;
        };
        let user = user.trim().to_ascii_lowercase();
        if user.is_empty() {
            continue;
        }
        let roots: Vec<String> = roots
            .split(',')
            .map(|r| r.trim().trim_end_matches('/').to_string())
            .filter(|r| !r.is_empty())
            .collect();
        if roots.is_empty() {
            continue;
        }
        map.insert(user, roots);
    }
    map
}

/// Assigned roots for a username (empty = not a storage admin).
/// Read from the env on every call so tests can mutate it.
pub fn roots_for(username: &str) -> Vec<String> {
    parse_storage_admins()
        .get(&username.trim().to_ascii_lowercase())
        .cloned()
        .unwrap_or_default()
}

/// True if `path` is `root` itself or lies underneath it. A root of
/// `/red/peddn8` matches `/red/peddn8` and `/red/peddn8/x`, but **not**
/// `/red/peddn80`. Paths are expected to be already normalized
/// (`crate::query::normalize_path`).
pub fn path_in_roots(roots: &[String], path: &str) -> bool {
    let path = path.trim_end_matches('/');
    if path.is_empty() {
        // "/" is the whole filesystem — only a root of "/" covers it,
        // which the starts_with below would never match, so handle it.
        return roots.iter().any(|r| r.trim_end_matches('/').is_empty());
    }
    roots.iter().any(|r| {
        let r = r.trim_end_matches('/');
        path == r || path.starts_with(&format!("{r}/"))
    })
}

/// True when the caller holds storage-admin roots (a global admin ignores
/// roots entirely, so this returns false for them).
pub fn is_storage_admin(claims: &Claims) -> bool {
    !claims.is_admin && !claims.roots.is_empty()
}

/// True if `path` lies on the route from the filesystem root down to one of
/// `roots` — i.e. `path` is a strict ancestor of a root (or the root itself).
/// The SPA's synthetic root `""` and `/red` are on the route to
/// `/red/peddn8`; `/red/peddn9` is not. Storage admins may navigate these
/// ancestors so they can click their way into their own storage.
pub fn path_on_route_to_roots(roots: &[String], path: &str) -> bool {
    let path = path.trim_end_matches('/');
    roots.iter().any(|r| {
        let r = r.trim_end_matches('/');
        r == path
            || r.starts_with(path)
                && (path.is_empty() || r.as_bytes().get(path.len()) == Some(&b'/'))
    })
}

/// Keep only the folder rows that a storage admin may see when navigating an
/// ancestor of their roots: anything inside (or equal to) a root, plus the
/// next hop towards each root. Siblings leading nowhere (e.g. `/red/peddn9`
/// when roots are `/red/peddn8`) are dropped.
pub fn filter_navigable_children(roots: &[String], children: &mut Vec<crate::db::FolderOut>) {
    children.retain(|c| {
        let p = c.path.trim_end_matches('/');
        if path_in_roots(roots, p) {
            return true; // inside (or equal to) a root
        }
        // Outside every root: keep only if some root lies strictly below it.
        roots.iter().any(|r| {
            let r = r.trim_end_matches('/');
            r.starts_with(p) && r.as_bytes().get(p.len()) == Some(&b'/')
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    fn claims(is_admin: bool, roots: Vec<&str>) -> Claims {
        Claims {
            sub: "user1".to_string(),
            is_admin,
            exp: 9_999_999_999,
            roots: roots.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    #[serial]
    fn parse_storage_admins_basic() {
        unsafe { std::env::set_var("STORAGE_ADMINS", "user1:/red/peddn7,/red/peddn8 ; USER2:/red/ssd") };
        let map = parse_storage_admins();
        assert_eq!(
            map.get("user1").unwrap(),
            &vec!["/red/peddn7".to_string(), "/red/peddn8".to_string()]
        );
        assert_eq!(map.get("user2").unwrap(), &vec!["/red/ssd".to_string()]);
        unsafe { std::env::remove_var("STORAGE_ADMINS") };
    }

    #[test]
    #[serial]
    fn parse_storage_admins_skips_bad_entries() {
        unsafe { std::env::set_var("STORAGE_ADMINS", "nocolon;  :/onlyroots;user3:;user4:/ok") };
        let map = parse_storage_admins();
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("user4"));
        unsafe { std::env::remove_var("STORAGE_ADMINS") };
    }

    #[test]
    #[serial]
    fn roots_for_is_case_insensitive_and_empty_by_default() {
        unsafe { std::env::remove_var("STORAGE_ADMINS") };
        assert!(roots_for("user1").is_empty());
        unsafe { std::env::set_var("STORAGE_ADMINS", "User1:/red/peddn8") };
        assert_eq!(roots_for("user1"), vec!["/red/peddn8".to_string()]);
        assert_eq!(roots_for("USER1"), vec!["/red/peddn8".to_string()]);
        unsafe { std::env::remove_var("STORAGE_ADMINS") };
    }

    #[test]
    #[serial]
    fn path_in_roots_prefix_rules() {
        let roots = vec!["/red/peddn8".to_string()];
        assert!(path_in_roots(&roots, "/red/peddn8"));
        assert!(path_in_roots(&roots, "/red/peddn8/"));
        assert!(path_in_roots(&roots, "/red/peddn8/sub/dir"));
        assert!(!path_in_roots(&roots, "/red/peddn80"));
        assert!(!path_in_roots(&roots, "/red"));
        assert!(!path_in_roots(&roots, ""));
    }

    #[test]
    #[serial]
    fn check_access_matrix() {
        let sa = claims(false, vec!["/red/peddn8"]);
        // Inside root: any users allowed, no users filter needed.
        assert_eq!(check_access(&sa, "/red/peddn8", &[]), Ok(AccessMode::Full));
        assert!(check_access(&sa, "/red/peddn8/x", &["other".into()]).is_ok());
        // Outside root: regular-user rules.
        assert!(check_access(&sa, "/red/peddn9", &["user1".into()]).is_ok());
        assert!(check_access(&sa, "/red/peddn9", &[]).is_err());
        assert!(check_access(&sa, "/red/peddn9", &["other".into()]).is_err());
        // Prefix trap: /red/peddn80 is outside /red/peddn8.
        assert!(check_access(&sa, "/red/peddn80", &["other".into()]).is_err());

        let global = claims(true, vec![]);
        assert!(check_access(&global, "/anything", &[]).is_ok());

        let user = claims(false, vec![]);
        assert!(check_access(&user, "/red/peddn8", &["user1".into()]).is_ok());
        assert!(check_access(&user, "/red/peddn8", &["other".into()]).is_err());
    }

    #[test]
    #[serial]
    fn check_access_navigation_mode() {
        let sa = claims(false, vec!["/red/peddn8"]);
        // Ancestors of a root are navigable with any users requested.
        assert_eq!(check_access(&sa, "", &[]), Ok(AccessMode::Navigable));
        assert_eq!(check_access(&sa, "/", &[]), Ok(AccessMode::Navigable));
        assert_eq!(check_access(&sa, "/red", &["other".into()]), Ok(AccessMode::Navigable));
        // Off-route siblings stay forbidden.
        assert!(check_access(&sa, "/red/peddn9", &[]).is_err());
        assert!(check_access(&sa, "/opt", &[]).is_err());
        // Empty roots never navigate.
        let user = claims(false, vec![]);
        assert!(check_access(&user, "", &[]).is_err());
    }

    #[test]
    #[serial]
    fn filter_navigable_children_keeps_route_only() {
        use crate::db::FolderOut;
        fn row(path: &str) -> FolderOut {
            FolderOut { path: path.into(), users: Default::default() }
        }
        let roots = vec!["/red/peddn8".to_string(), "/red/nas4".to_string()];
        let mut children = vec![
            row("/red/peddn8"),   // a root itself
            row("/red/peddn9"),   // sibling: dropped
            row("/red"),          // not a child level, kept check anyway
            row("/opt"),          // unrelated: dropped
        ];
        filter_navigable_children(&roots, &mut children);
        let paths: Vec<&str> = children.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(paths, vec!["/red/peddn8", "/red"]);
    }

    #[test]
    #[serial]
    fn is_storage_admin_flag() {
        assert!(is_storage_admin(&claims(false, vec!["/a"])));
        assert!(!is_storage_admin(&claims(true, vec!["/a"])));
        assert!(!is_storage_admin(&claims(false, vec![])));
    }
}
