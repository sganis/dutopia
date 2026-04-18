// rs/src/auth.rs
use axum::{
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Response},
    Json, 
    RequestPartsExt, 
};
use axum_extra::{
    headers::{authorization::Bearer, Authorization},
    TypedHeader,
};
use jsonwebtoken::{decode, DecodingKey, EncodingKey, Validation};
use serde::{Deserialize, Serialize};
use std::fmt::Display;
use std::sync::OnceLock;

// ---- Keys (JWT) ----
pub struct Keys {
    pub encoding: EncodingKey,
    pub decoding: DecodingKey,
}

impl Keys {
    fn new(secret: &[u8]) -> Self {
        Self {
            encoding: EncodingKey::from_secret(secret),
            decoding: DecodingKey::from_secret(secret),
        }
    }
}

static KEYS: OnceLock<Keys> = OnceLock::new();

#[inline]
pub fn keys() -> &'static Keys {
    KEYS.get_or_init(|| {
        let secret = std::env::var("JWT_SECRET").expect("JWT_SECRET must be set");
        Keys::new(secret.as_bytes())
    })
}

#[derive(Debug, Deserialize)]
pub struct AuthPayload {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct AuthBody {
    pub access_token: String,
    pub token_type: String,
}

// implement a method to create a response type containing the JWT
impl AuthBody {
    pub fn new(access_token: String) -> Self {
        Self {
            access_token,
            token_type: "Bearer".to_string(),
        }
    }
}

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
}

#[derive(Debug)]
pub enum AuthError {
    Forbidden,
    WrongCredentials,
    MissingCredentials,
    TokenCreation,
    InvalidToken,
}

// implement IntoResponse for AuthError so we can use it as an Axum response type
impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, error_message) = match self {
            AuthError::Forbidden => (StatusCode::FORBIDDEN, "No access to this resource"),
            AuthError::WrongCredentials => (StatusCode::UNAUTHORIZED, "Wrong credentials"),
            AuthError::MissingCredentials => (StatusCode::BAD_REQUEST, "Missing credentials"),
            AuthError::TokenCreation => (StatusCode::INTERNAL_SERVER_ERROR, "Token creation error"),
            AuthError::InvalidToken => (StatusCode::BAD_REQUEST, "Invalid token"),
        };
        let body = Json(ErrorBody { error: error_message });
        (status, body).into_response()
    }
}


#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub sub: String,
    pub is_admin: bool,
    pub exp: usize,
}

// allow us to print the claim details for the private route
impl Display for Claims {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "username: {}\nis_admin: {}", self.sub, self.is_admin)
    }
}

// implement FromRequestParts for Claims (the JWT struct)
// FromRequestParts allows us to use Claims without consuming the request
impl<S> FromRequestParts<S> for Claims
where
    S: Send + Sync,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        // Extract the token from the authorization header
        let TypedHeader(Authorization(bearer)) = parts
            .extract::<TypedHeader<Authorization<Bearer>>>()
            .await
            .map_err(|_| AuthError::InvalidToken)?;
        // Decode the user data
        let token_data = decode::<Claims>(bearer.token(), &keys().decoding, &Validation::default())
            .map_err(|_| AuthError::InvalidToken)?;

        Ok(token_data.claims)
    }
}

/// Outcome of `verify_credentials`. `authenticated` is true when either the
/// platform verifier accepted the credentials or the `ADMIN_PASSWORD` test
/// bypass matched. `admin_override` is true only when the bypass granted
/// access, so the caller can escalate to `is_admin` without consulting
/// `ADMIN_GROUP`.
pub struct VerifyResult {
    pub authenticated: bool,
    pub admin_override: bool,
}

/// Verify credentials with an optional test-mode bypass.
///
/// If the `ADMIN_PASSWORD` env var is set to a non-empty value and the
/// supplied password matches it, authentication succeeds for any username
/// *and* the returned result carries `admin_override = true`. Intended
/// strictly for local development and CI — do NOT set `ADMIN_PASSWORD` in
/// production.
pub fn verify_credentials(username: &str, password: &str) -> VerifyResult {
    if let Ok(expected) = std::env::var("ADMIN_PASSWORD") {
        if !expected.is_empty() && password == expected {
            tracing::warn!(user = %username, "verify_credentials: ADMIN_PASSWORD override matched");
            return VerifyResult {
                authenticated: true,
                admin_override: true,
            };
        }
    }
    VerifyResult {
        authenticated: platform::verify_user(username, password),
        admin_override: false,
    }
}

// #[cfg(target_os = "macos")]
// pub mod platform {
//     use pam::Authenticator;

//     pub fn verify_user(username: &str, password: &str) -> bool {
//         let mut auth = match Authenticator::with_password("login") {
//             Ok(a) => a,
//             Err(_) => return false,
//         };
//         auth.get_handler().set_credentials(username, password);
//         auth.authenticate().is_ok()
//     }
// }

#[cfg(target_os = "macos")]
pub mod platform {
    use std::process::{Command, Stdio};
    pub fn verify_user(username: &str, password: &str) -> bool {
        // WARNING: password is passed as a process argument (visible to local admins via ps)
        // Prefer PAM for production security; this is the simplest working approach.
        match Command::new("dscl")
            .args([".", "-authonly", username, password])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            Ok(status) => {
                if status.success() {
                    true
                } else {
                    tracing::warn!(user = %username, code = ?status.code(), "verify_user: dscl exited non-zero");
                    false
                }
            },
            Err(e) => {
                tracing::warn!(user = %username, error = %e, "verify_user: failed to spawn dscl");
                false
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub mod platform {
    use std::process::{Command, Stdio};
    use std::io::Write;

    /// Verify user credentials using the `su` command.
    /// Returns true if authentication succeeds, false otherwise.
    /// NOTE: passwords are written to su's stdin; they are NEVER logged.
    pub fn verify_user(username: &str, password: &str) -> bool {
        tracing::debug!(user = %username, "verify_user: spawning su");
        let mut child = match Command::new("su")
            .arg(username)
            .arg("-c")
            .arg("true")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(e) => {
                tracing::warn!(user = %username, error = %e, "verify_user: failed to spawn su");
                return false;
            }
        };

        if let Some(mut stdin) = child.stdin.take() {
            if let Err(e) = writeln!(stdin, "{}", password) {
                tracing::warn!(user = %username, error = %e, "verify_user: failed to write password to su stdin");
                return false;
            }
        }

        match child.wait() {
            Ok(status) if status.success() => true,
            Ok(status) => {
                tracing::warn!(user = %username, code = ?status.code(), "verify_user: su exited non-zero");
                false
            }
            Err(e) => {
                tracing::warn!(user = %username, error = %e, "verify_user: failed waiting for su");
                false
            }
        }
    }
}

#[cfg(windows)]
pub mod platform {
    use std::env;

    /// Fake auth. The username must match the interactive Windows user (so it
    /// lines up with what dusum stored as the file owner). Override either the
    /// expected user or password via `FAKE_USER` / `FAKE_PASSWORD` for tests.
    pub fn verify_user(username: &str, password: &str) -> bool {
        let expected_user = env::var("FAKE_USER")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| env::var("USERNAME").ok().filter(|s| !s.trim().is_empty()))
            .unwrap_or_else(|| "admin".to_string());
        let expected_pass = env::var("FAKE_PASSWORD").unwrap_or_else(|_| "admin".to_string());
        username == expected_user && password == expected_pass
    }
}

#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests {
    use super::platform::verify_user;

    #[test]
    fn test_verify_user_rejects_garbage() {
        // su will fail for a non-existent user; we just want to confirm the function
        // returns `false` rather than panicking after the println! → tracing migration.
        assert!(!verify_user("definitely_not_a_real_user_xyz", "wrong"));
    }
}

#[cfg(test)]
mod admin_override_tests {
    use super::verify_credentials;
    use serial_test::serial;

    #[test]
    #[serial]
    fn admin_password_bypass_matches_and_grants_admin() {
        // SAFETY: serial test isolates env mutation.
        unsafe { std::env::set_var("ADMIN_PASSWORD", "s3cret") };
        let r = verify_credentials("anyuser", "s3cret");
        assert!(r.authenticated);
        assert!(r.admin_override);
        unsafe { std::env::remove_var("ADMIN_PASSWORD") };
    }

    #[test]
    #[serial]
    fn admin_password_bypass_ignored_when_empty() {
        unsafe { std::env::set_var("ADMIN_PASSWORD", "") };
        // Empty ADMIN_PASSWORD must not accept an empty password; falls through
        // to the platform verifier, which will fail for a garbage user.
        let r = verify_credentials("definitely_not_a_real_user_xyz", "");
        assert!(!r.authenticated);
        assert!(!r.admin_override);
        unsafe { std::env::remove_var("ADMIN_PASSWORD") };
    }

    #[test]
    #[serial]
    fn admin_password_wrong_value_does_not_bypass() {
        unsafe { std::env::set_var("ADMIN_PASSWORD", "expected") };
        let r = verify_credentials("definitely_not_a_real_user_xyz", "wrong");
        assert!(!r.authenticated);
        assert!(!r.admin_override);
        unsafe { std::env::remove_var("ADMIN_PASSWORD") };
    }
}

