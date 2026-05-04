// rs/src/bin/duapi/cleanup.rs
//
// Cleanup-request endpoints: script download + email notify.
//
// Context: browser builds of Dutopia target read-only cluster deployments
// where duapi cannot delete files. Instead, the admin queues paths
// client-side and asks the server to either (a) generate a Python script
// the target user runs on a cluster node, or (b) email the target user
// with the list of paths to clean up.
//
// Stateless: nothing is persisted server-side — each request is independent.

use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use dutopia::auth::{AuthError, Claims};

use crate::email;

/// Hard cap on paths per request. Picked to bound script size + memory; the
/// browser-side queue practically never reaches this.
const MAX_PATHS: usize = 5000;
const MAX_PATH_LEN: usize = 4096;

#[derive(Debug, Deserialize)]
pub struct PathItem {
    pub path: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Deserialize)]
pub struct ScriptReq {
    pub username: String,
    pub paths: Vec<PathItem>,
}

#[derive(Debug, Deserialize)]
pub struct NotifyReq {
    pub username: String,
    pub paths: Vec<PathItem>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct NotifyResp {
    pub sent: bool,
    pub to: String,
}

/// POST /api/cleanup/script
///
/// Admin: any `username` accepted. Non-admin: `username` must equal
/// `claims.sub`. Returns a text/x-python attachment.
pub async fn script_handler(
    claims: Claims,
    Json(req): Json<ScriptReq>,
) -> Response {
    if !claims.is_admin && claims.sub != req.username {
        tracing::warn!(
            actor = %claims.sub, target = %req.username,
            "403 Forbidden /api/cleanup/script (non-admin targeting other user)"
        );
        return AuthError::Forbidden.into_response();
    }

    if let Err((code, msg)) = validate(&req.username, &req.paths) {
        tracing::warn!(actor = %claims.sub, %msg, "{code} /api/cleanup/script invalid");
        return (code, msg).into_response();
    }

    let script = render_script(&req.username, &req.paths);
    let filename = format!(
        "cleanup-{}-{}.py",
        sanitize_filename(&req.username),
        Utc::now().format("%Y%m%d"),
    );
    tracing::info!(
        actor = %claims.sub, target = %req.username, items = req.paths.len(),
        "200 OK /api/cleanup/script"
    );

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/x-python; charset=utf-8".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        script,
    )
        .into_response()
}

/// POST /api/cleanup/notify
///
/// Admin-only. Emails the target user the list of paths via SMTP. Returns
/// 404 if MAIL_DOMAIN is not set, 501 if SMTP is not configured, 502 on
/// SMTP send failure.
pub async fn notify_handler(
    claims: Claims,
    Json(req): Json<NotifyReq>,
) -> Response {
    if !claims.is_admin {
        tracing::warn!(
            actor = %claims.sub, "403 Forbidden /api/cleanup/notify (not admin)"
        );
        return AuthError::Forbidden.into_response();
    }

    if let Err((code, msg)) = validate(&req.username, &req.paths) {
        tracing::warn!(actor = %claims.sub, %msg, "{code} /api/cleanup/notify invalid");
        return (code, msg).into_response();
    }

    if !email::is_configured() {
        tracing::warn!("501 Not Implemented /api/cleanup/notify (SMTP not configured)");
        return (StatusCode::NOT_IMPLEMENTED, "SMTP not configured").into_response();
    }

    let to = match email::resolve_email(&req.username) {
        Some(addr) => addr,
        None => {
            tracing::warn!(target = %req.username, "404 /api/cleanup/notify (no email)");
            return (StatusCode::NOT_FOUND, "email unavailable for user").into_response();
        }
    };

    let subject = format!("Dutopia: cleanup requested for {} paths", req.paths.len());
    let body = render_email_body(&claims.sub, &req.username, &req.paths, req.message.as_deref());

    let send_fut = tokio::task::spawn_blocking({
        let to = to.clone();
        move || email::send(&to, &subject, &body)
    });

    match send_fut.await {
        Err(join_err) => {
            tracing::error!(err = %join_err, "500 Task Join Error /api/cleanup/notify");
            (StatusCode::INTERNAL_SERVER_ERROR, "task error").into_response()
        }
        Ok(Err(e)) => {
            tracing::warn!(err = %e, "502 Bad Gateway /api/cleanup/notify");
            (StatusCode::BAD_GATEWAY, "email send failed").into_response()
        }
        Ok(Ok(())) => {
            tracing::info!(
                actor = %claims.sub, target = %req.username, to = %to,
                items = req.paths.len(),
                "200 OK /api/cleanup/notify"
            );
            Json(NotifyResp { sent: true, to }).into_response()
        }
    }
}

fn validate(username: &str, paths: &[PathItem]) -> Result<(), (StatusCode, String)> {
    if username.trim().is_empty() || username.len() > 256 {
        return Err((StatusCode::BAD_REQUEST, "invalid username".to_string()));
    }
    if paths.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "empty paths".to_string()));
    }
    if paths.len() > MAX_PATHS {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("too many paths (max {MAX_PATHS})"),
        ));
    }
    for p in paths {
        if p.path.is_empty() || p.path.len() > MAX_PATH_LEN {
            return Err((StatusCode::BAD_REQUEST, "invalid path length".to_string()));
        }
        if p.path.contains('\0') {
            return Err((StatusCode::BAD_REQUEST, "path contains NUL".to_string()));
        }
        if !is_absolute(&p.path) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("path must be absolute: {}", p.path),
            ));
        }
        if has_traversal(&p.path) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("path traversal rejected: {}", p.path),
            ));
        }
    }
    Ok(())
}

fn is_absolute(p: &str) -> bool {
    p.starts_with('/')
        || p.starts_with('\\')
        // Windows drive letter: "C:\..." or "C:/..."
        || (p.len() >= 3
            && p.as_bytes()[1] == b':'
            && (p.as_bytes()[2] == b'\\' || p.as_bytes()[2] == b'/')
            && p.as_bytes()[0].is_ascii_alphabetic())
}

fn has_traversal(p: &str) -> bool {
    // Reject ".." as a standalone path segment on either / or \ separators.
    p.split(|c| c == '/' || c == '\\').any(|seg| seg == "..")
}

fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

/// Build the user-facing Python 3 script. The path list is embedded as a
/// JSON literal inside a Python raw string so we don't have to worry about
/// escaping individual path characters — Python's json.loads does the right
/// thing for any valid JSON input we produce with serde_json.
pub fn render_script(username: &str, items: &[PathItem]) -> String {
    let json_items = serde_json::to_string(
        &items
            .iter()
            .map(|i| serde_json::json!({ "path": i.path, "size": i.size }))
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".to_string());

    // Make sure the JSON literal can't break out of Python's triple-quoted
    // raw string. Triple-quote or backslash sequences in a path would do it;
    // neither is legal in JSON-encoded strings, but we double-check anyway.
    let safe_json = json_items.replace("\"\"\"", "\\\"\\\"\\\"");

    let today = Utc::now().format("%Y-%m-%d");
    let safe_user = escape_python_single_quoted(username);

    format!(
        r#"#!/usr/bin/env python3
# Auto-generated by Dutopia on {today} for user '{safe_user}'.
# DRY RUN by default. Re-run with --execute to actually delete.
#
# Usage:
#   python3 cleanup.py                # dry-run; prints what would be removed
#   python3 cleanup.py --execute      # actually deletes
#   python3 cleanup.py --execute --force  # skip ownership check
import argparse, os, shutil, sys, stat, pwd, json

ITEMS = json.loads(r"""{safe_json}""")
TARGET_USER = '{safe_user}'


def main():
    p = argparse.ArgumentParser(description="Dutopia cleanup script")
    p.add_argument("--execute", action="store_true",
                   help="Actually delete (default is dry-run).")
    p.add_argument("--force", action="store_true",
                   help="Skip ownership check (not recommended).")
    args = p.parse_args()

    try:
        current = pwd.getpwuid(os.geteuid()).pw_name
    except KeyError:
        current = str(os.geteuid())
    if current != TARGET_USER and not args.force:
        sys.exit("Refusing: run as %s or pass --force (current=%s)." % (TARGET_USER, current))

    total = freed = 0
    errors = 0
    for it in ITEMS:
        path = it.get("path")
        size = int(it.get("size") or 0)
        total += size
        if not path:
            continue
        if not os.path.lexists(path):
            print("[skip] missing: %s" % path); continue
        try:
            st = os.lstat(path)
        except OSError as e:
            print("[err] stat %s: %s" % (path, e), file=sys.stderr); errors += 1; continue
        if st.st_uid != os.geteuid() and not args.force:
            print("[skip] not owner: %s" % path); continue
        if not args.execute:
            print("[dry-run] would remove %s (%d B)" % (path, size))
            freed += size; continue
        try:
            if stat.S_ISDIR(st.st_mode) and not stat.S_ISLNK(st.st_mode):
                shutil.rmtree(path)
            else:
                os.remove(path)
            print("[ok] removed %s" % path)
            freed += size
        except OSError as e:
            print("[err] %s: %s" % (path, e), file=sys.stderr); errors += 1
    verb = "removed" if args.execute else "to remove"
    print("Done. %d/%d bytes %s. %d errors." % (freed, total, verb, errors))
    if errors:
        sys.exit(1)


if __name__ == "__main__":
    main()
"#,
    )
}

fn render_email_body(
    actor: &str,
    target: &str,
    items: &[PathItem],
    message: Option<&str>,
) -> String {
    let mut body = String::with_capacity(256 + items.len() * 80);
    body.push_str("Hello ");
    body.push_str(target);
    body.push_str(",\n\n");
    body.push_str(actor);
    body.push_str(" has requested that you clean up the following paths");
    body.push_str(" — they appear to be taking up significant disk space.\n\n");
    if let Some(m) = message {
        let m = m.trim();
        if !m.is_empty() {
            body.push_str("Message:\n");
            body.push_str(m);
            body.push_str("\n\n");
        }
    }
    body.push_str("Paths:\n");
    let mut total: u64 = 0;
    for it in items {
        body.push_str("  ");
        body.push_str(&it.path);
        body.push_str("  (");
        body.push_str(&human_bytes(it.size));
        body.push_str(")\n");
        total += it.size;
    }
    body.push_str("\nTotal: ");
    body.push_str(&human_bytes(total));
    body.push_str("\n\n");
    body.push_str("You can ask your admin for a cleanup script that does this automatically.\n");
    body.push_str("— Dutopia\n");
    body
}

fn human_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB", "PB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} {}", n, UNITS[0])
    } else {
        format!("{:.2} {}", v, UNITS[i])
    }
}

fn escape_python_single_quoted(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(path: &str, size: u64) -> PathItem {
        PathItem { path: path.to_string(), size }
    }

    #[test]
    fn validate_accepts_absolute_unix_and_windows_paths() {
        assert!(validate("alice", &[p("/tmp/x", 1)]).is_ok());
        assert!(validate("alice", &[p("C:\\Users\\x", 1)]).is_ok());
        assert!(validate("alice", &[p("C:/Users/x", 1)]).is_ok());
    }

    #[test]
    fn validate_rejects_relative_paths() {
        assert!(validate("alice", &[p("tmp/x", 1)]).is_err());
        assert!(validate("alice", &[p("./x", 1)]).is_err());
    }

    #[test]
    fn validate_rejects_traversal() {
        assert!(validate("alice", &[p("/var/../etc/passwd", 1)]).is_err());
        assert!(validate("alice", &[p("C:\\Users\\..\\Windows", 1)]).is_err());
    }

    #[test]
    fn validate_rejects_nul_and_too_long() {
        assert!(validate("alice", &[p("/x\0y", 1)]).is_err());
        let huge = format!("/{}", "a".repeat(MAX_PATH_LEN + 1));
        assert!(validate("alice", &[p(&huge, 1)]).is_err());
    }

    #[test]
    fn validate_rejects_empty_paths() {
        assert!(validate("alice", &[]).is_err());
    }

    #[test]
    fn validate_rejects_empty_username() {
        assert!(validate("  ", &[p("/x", 1)]).is_err());
    }

    #[test]
    fn render_script_contains_target_user_and_items() {
        let s = render_script("alice", &[p("/tmp/x", 10), p("/tmp/y", 20)]);
        assert!(s.contains("TARGET_USER = 'alice'"));
        assert!(s.contains("/tmp/x"));
        assert!(s.contains("/tmp/y"));
        assert!(s.contains("--execute"));
        assert!(s.starts_with("#!/usr/bin/env python3"));
    }

    #[test]
    fn render_script_escapes_username() {
        let s = render_script("al'ice", &[p("/x", 1)]);
        assert!(s.contains("TARGET_USER = 'al\\'ice'"));
    }

    #[test]
    fn sanitize_filename_keeps_safe_chars_only() {
        assert_eq!(sanitize_filename("alice"), "alice");
        assert_eq!(sanitize_filename("a/b c"), "a_b_c");
        assert_eq!(sanitize_filename("../etc"), "___etc");
    }

    #[test]
    fn human_bytes_scales() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert!(human_bytes(1024).ends_with(" KB"));
        assert!(human_bytes(1024 * 1024 * 1024).ends_with(" GB"));
    }

    #[test]
    fn render_email_body_lists_all_paths() {
        let body = render_email_body(
            "admin",
            "alice",
            &[p("/tmp/a", 1000), p("/tmp/b", 2000)],
            Some("please"),
        );
        assert!(body.contains("/tmp/a"));
        assert!(body.contains("/tmp/b"));
        assert!(body.contains("please"));
        assert!(body.contains("alice"));
        assert!(body.contains("admin"));
    }
}
