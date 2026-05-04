// rs/src/query.rs
//
// Path normalization shared by duapi (HTTP handlers) and the Tauri desktop
// backend. Both take untrusted user-supplied paths and convert them to the
// native OS form the SQLite index was built with (trailing backslash on
// drive roots, no trailing separator elsewhere, reject `..` traversal).

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

    let is_unc = trimmed.starts_with(r"\\");
    let has_drive_prefix = bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':';
    let use_backslash = is_unc || has_drive_prefix || trimmed.contains('\\');
    let sep: char = if use_backslash { '\\' } else { '/' };
    let is_unix_absolute = !use_backslash && trimmed.starts_with('/');

    if trimmed == "/" {
        return Some("/".to_string());
    }
    if has_drive_prefix && bytes.len() <= 3 {
        return Some(format!("{}:\\", bytes[0] as char));
    }

    let body: &str = if is_unc {
        &trimmed[2..]
    } else if is_unix_absolute {
        &trimmed[1..]
    } else if has_drive_prefix {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_forms() {
        assert_eq!(normalize_path("/").as_deref(), Some("/"));
        assert_eq!(normalize_path("").as_deref(), Some(""));
        assert_eq!(normalize_path("/var/log").as_deref(), Some("/var/log"));
    }

    #[test]
    fn drive_letter_adds_backslash() {
        assert_eq!(normalize_path("C:").as_deref(), Some("C:\\"));
        assert_eq!(normalize_path("F:").as_deref(), Some("F:\\"));
        assert_eq!(normalize_path("C:\\").as_deref(), Some("C:\\"));
        assert_eq!(normalize_path("C:\\Dev\\foo").as_deref(), Some("C:\\Dev\\foo"));
        assert_eq!(normalize_path("C:\\Dev\\foo\\").as_deref(), Some("C:\\Dev\\foo"));
    }

    #[test]
    fn unc_preserved() {
        assert_eq!(
            normalize_path(r"\\server\share\dir").as_deref(),
            Some(r"\\server\share\dir")
        );
    }

    #[test]
    fn collapses_and_trims() {
        assert_eq!(normalize_path("/var//log/").as_deref(), Some("/var/log"));
        assert_eq!(normalize_path("/var/./log").as_deref(), Some("/var/log"));
        assert_eq!(normalize_path("  /var/log  ").as_deref(), Some("/var/log"));
    }

    #[test]
    fn rejects_traversal_and_nul() {
        assert!(normalize_path("/var/../etc/passwd").is_none());
        assert!(normalize_path("..").is_none());
        assert!(normalize_path("/var/log\0/etc").is_none());
    }
}
