// src/bin/dudb/raw_path.rs
//
// Map a raw duscan file path to (folder, name) with the folder in exactly the
// form `dusum::aggregate::get_folder_ancestors` stored it, so it can be looked
// up in the dusum path→id cache. Hot path of a billion-row ingest: already
// canonical Unix paths are borrowed, not copied.
use std::borrow::Cow;

/// Split a raw file path into (parent folder, filename), with the folder in
/// exactly the form `dusum::aggregate::get_folder_ancestors` stores it (its
/// deepest ancestor): native separators collapsed and trimmed, drive roots
/// kept as `C:\`, invalid UTF-8 replaced with U+FFFD like dusum output.
/// Returns None when dusum would emit no ancestor for the path (no separator,
/// trailing separator, or a folder part that normalizes to nothing).
pub fn split_folder_name(path: &[u8]) -> Option<(Cow<'_, str>, Cow<'_, str>)> {
    let sep = separator(path);
    let pos = path.iter().rposition(|&b| b == sep)?;
    let name = &path[pos + 1..];
    if name.is_empty() {
        return None;
    }
    let folder: Cow<[u8]> = if pos == 0 {
        Cow::Borrowed(&path[..1])
    } else if is_canonical_unix(&path[..pos], sep) {
        Cow::Borrowed(&path[..pos])
    } else {
        Cow::Owned(normalize_folder(&path[..pos], sep)?)
    };
    let folder = match folder {
        Cow::Borrowed(b) => String::from_utf8_lossy(b),
        Cow::Owned(v) => Cow::Owned(String::from_utf8_lossy(&v).into_owned()),
    };
    Some((folder, String::from_utf8_lossy(name)))
}

/// `/a/b` with no empty segments is already what dusum emits.
fn is_canonical_unix(f: &[u8], sep: u8) -> bool {
    sep == b'/'
        && f.first() == Some(&b'/')
        && f.last() != Some(&b'/')
        && memchr::memmem::find(f, b"//").is_none()
}

fn normalize_folder(f: &[u8], sep: u8) -> Option<Vec<u8>> {
    let mut f = f;
    while f.len() > 1 && f.last() == Some(&sep) {
        f = &f[..f.len() - 1];
    }
    // Anchor prefix, then rejoin non-empty segments so repeated
    // separators collapse exactly as get_folder_ancestors does.
    let (mut current, rest): (Vec<u8>, &[u8]) = if sep == b'/' && f.starts_with(b"/") {
        (b"/".to_vec(), &f[1..])
    } else if sep == b'\\' && f.starts_with(b"\\\\") {
        (b"\\\\".to_vec(), &f[2..])
    } else if sep == b'\\' && f.len() >= 2 && f[0].is_ascii_alphabetic() && f[1] == b':' {
        let rest = if f.len() >= 3 && f[2] == b'\\' {
            &f[3..]
        } else {
            &f[2..]
        };
        (vec![f[0], b':', b'\\'], rest)
    } else {
        (Vec::new(), f)
    };
    for segment in rest.split(|&b| b == sep).filter(|s| !s.is_empty()) {
        if !current.is_empty() && current.last() != Some(&sep) {
            current.push(sep);
        }
        current.extend_from_slice(segment);
    }
    // A bare UNC prefix ("\\") with no segments never appears in dusum
    // output; treat it as no-ancestor like get_folder_ancestors does.
    if current.is_empty() || current == b"\\\\" {
        return None;
    }
    Some(current)
}

/// Must match `dusum::aggregate::separator`: a backslash anywhere (or a drive
/// prefix) means Windows-native; otherwise Unix.
fn separator(path: &[u8]) -> u8 {
    if memchr::memchr(b'\\', path).is_some() {
        return b'\\';
    }
    if path.len() >= 2 && path[0].is_ascii_alphabetic() && path[1] == b':' {
        return b'\\';
    }
    b'/'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(p: &[u8]) -> (String, String) {
        let (f, n) = split_folder_name(p).unwrap();
        (f.into_owned(), n.into_owned())
    }

    fn pair(f: &str, n: &str) -> (String, String) {
        (f.to_string(), n.to_string())
    }

    #[test]
    fn split_folder_name_native_forms() {
        assert_eq!(ok(b"/f.txt"), pair("/", "f.txt"));
        assert_eq!(ok(b"/a/b/f.txt"), pair("/a/b", "f.txt"));
        // Repeated separators collapse, matching get_folder_ancestors.
        assert_eq!(ok(b"/a//b//f.txt"), pair("/a/b", "f.txt"));
        assert_eq!(ok(b"//a/f.txt"), pair("/a", "f.txt"));
        assert_eq!(ok(b"C:\\f.txt"), pair("C:\\", "f.txt"));
        assert_eq!(ok(b"C:\\Users\\San\\f.txt"), pair("C:\\Users\\San", "f.txt"));
        assert_eq!(ok(b"\\\\srv\\shr\\f.txt"), pair("\\\\srv\\shr", "f.txt"));
        assert_eq!(split_folder_name(b"relative.txt"), None);
        assert_eq!(split_folder_name(b"/a/trailing/"), None);
    }

    #[test]
    fn canonical_unix_folder_is_borrowed() {
        let (f, n) = split_folder_name(b"/a/b/f.txt").unwrap();
        assert!(matches!(f, Cow::Borrowed(_)));
        assert!(matches!(n, Cow::Borrowed(_)));
    }

    #[test]
    fn split_folder_name_lossy_utf8_matches_dusum() {
        // dusum writes folder paths through from_utf8_lossy; the raw pass must
        // produce the same lossy string or the folder lookup misses.
        let raw = [b'/', 0xFFu8, b'a', b'/', b'f'];
        let (folder, name) = split_folder_name(&raw).unwrap();
        assert_eq!(folder, String::from_utf8_lossy(&[b'/', 0xFFu8, b'a']));
        assert_eq!(name, "f");
    }
}
