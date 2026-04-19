// rs/src/bin/duapi/oidc.rs
use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use dutopia::auth::Claims;

static CONFIG: OnceLock<Option<OidcConfig>> = OnceLock::new();
static JWKS_CACHE: OnceLock<Mutex<JwksCache>> = OnceLock::new();

const JWKS_TTL: Duration = Duration::from_secs(3600);

#[derive(Debug, Clone)]
pub struct OidcConfig {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
    pub scopes: String,
    pub username_claim: String,
    pub authorize_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
    pub post_login_redirect: String,
}

#[derive(Debug, Deserialize)]
struct Discovery {
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    id_token: String,
    #[allow(dead_code)]
    access_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Debug, Deserialize, Clone)]
struct Jwk {
    kid: String,
    kty: String,
    n: Option<String>,
    e: Option<String>,
    #[serde(rename = "use")]
    use_: Option<String>,
    alg: Option<String>,
}

struct JwksCache {
    keys: HashMap<String, Jwk>,
    fetched_at: Option<Instant>,
}

/// Initialize OIDC at boot. Returns Ok(Some(cfg)) if OIDC_ISSUER is set and discovery succeeds,
/// Ok(None) if OIDC is disabled, Err if misconfigured.
pub async fn init() -> Result<()> {
    let issuer = match std::env::var("OIDC_ISSUER") {
        Ok(s) if !s.trim().is_empty() => s.trim().trim_end_matches('/').to_string(),
        _ => {
            let _ = CONFIG.set(None);
            return Ok(());
        }
    };
    let client_id = std::env::var("OIDC_CLIENT_ID")
        .context("OIDC_CLIENT_ID required when OIDC_ISSUER is set")?;
    let client_secret = std::env::var("OIDC_CLIENT_SECRET")
        .context("OIDC_CLIENT_SECRET required when OIDC_ISSUER is set")?;
    let redirect_uri = std::env::var("OIDC_REDIRECT_URI")
        .context("OIDC_REDIRECT_URI required when OIDC_ISSUER is set")?;
    let scopes = std::env::var("OIDC_SCOPES").unwrap_or_else(|_| "openid profile email".into());
    let username_claim =
        std::env::var("OIDC_USERNAME_CLAIM").unwrap_or_else(|_| "preferred_username".into());
    let post_login_redirect =
        std::env::var("OIDC_POST_LOGIN_REDIRECT").unwrap_or_else(|_| "/".into());

    let disc_url = format!("{}/.well-known/openid-configuration", issuer);
    let disc: Discovery = reqwest::Client::new()
        .get(&disc_url)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .with_context(|| format!("fetching OIDC discovery from {disc_url}"))?
        .error_for_status()
        .with_context(|| format!("OIDC discovery {disc_url} returned error"))?
        .json()
        .await
        .context("parsing OIDC discovery")?;

    let cfg = OidcConfig {
        issuer,
        client_id,
        client_secret,
        redirect_uri,
        scopes,
        username_claim,
        authorize_endpoint: disc.authorization_endpoint,
        token_endpoint: disc.token_endpoint,
        jwks_uri: disc.jwks_uri,
        post_login_redirect,
    };
    tracing::info!(issuer = %cfg.issuer, "OIDC enabled");
    let _ = CONFIG.set(Some(cfg));
    let _ = JWKS_CACHE.set(Mutex::new(JwksCache {
        keys: HashMap::new(),
        fetched_at: None,
    }));
    Ok(())
}

pub fn config() -> Option<&'static OidcConfig> {
    CONFIG.get().and_then(|o| o.as_ref())
}

pub fn is_enabled() -> bool {
    config().is_some()
}

/// Generate PKCE verifier + challenge pair.
pub fn new_pkce() -> (String, String) {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

pub fn new_state() -> String {
    let mut bytes = [0u8; 24];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn authorize_url(state: &str, pkce_challenge: &str) -> Result<String> {
    let cfg = config().ok_or_else(|| anyhow!("OIDC not configured"))?;
    let mut u = url::Url::parse(&cfg.authorize_endpoint)?;
    u.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &cfg.client_id)
        .append_pair("redirect_uri", &cfg.redirect_uri)
        .append_pair("scope", &cfg.scopes)
        .append_pair("state", state)
        .append_pair("code_challenge", pkce_challenge)
        .append_pair("code_challenge_method", "S256");
    Ok(u.into())
}

/// Exchange an authorization code for an id_token, verify it, map to internal Claims.
pub async fn exchange_and_verify(code: &str, pkce_verifier: &str, ttl_secs: u64) -> Result<Claims> {
    let cfg = config().ok_or_else(|| anyhow!("OIDC not configured"))?;

    let params = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", &cfg.redirect_uri),
        ("client_id", &cfg.client_id),
        ("client_secret", &cfg.client_secret),
        ("code_verifier", pkce_verifier),
    ];
    let tok: TokenResponse = reqwest::Client::new()
        .post(&cfg.token_endpoint)
        .form(&params)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .context("token endpoint request")?
        .error_for_status()
        .context("token endpoint rejected request")?
        .json()
        .await
        .context("parsing token response")?;

    let id_claims = verify_id_token(&tok.id_token, cfg).await?;
    map_to_internal(id_claims, cfg, ttl_secs)
}

async fn get_jwk(kid: &str, cfg: &OidcConfig) -> Result<Jwk> {
    // Fast path: cache hit that's fresh
    {
        let guard = JWKS_CACHE.get().expect("jwks cache").lock().unwrap();
        if let (Some(k), Some(fetched)) = (guard.keys.get(kid), guard.fetched_at) {
            if fetched.elapsed() < JWKS_TTL {
                return Ok(k.clone());
            }
        }
    }
    // Refetch
    let jwks: Jwks = reqwest::Client::new()
        .get(&cfg.jwks_uri)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .context("JWKS fetch")?
        .error_for_status()
        .context("JWKS endpoint returned error")?
        .json()
        .await
        .context("parsing JWKS")?;

    let mut map = HashMap::with_capacity(jwks.keys.len());
    for k in jwks.keys {
        map.insert(k.kid.clone(), k);
    }
    let found = map
        .get(kid)
        .cloned()
        .ok_or_else(|| anyhow!("JWKS missing kid {kid}"))?;

    let mut guard = JWKS_CACHE.get().expect("jwks cache").lock().unwrap();
    guard.keys = map;
    guard.fetched_at = Some(Instant::now());
    Ok(found)
}

#[derive(Debug, Deserialize)]
struct IdTokenClaims {
    #[serde(flatten)]
    extra: HashMap<String, serde_json::Value>,
}

async fn verify_id_token(token: &str, cfg: &OidcConfig) -> Result<IdTokenClaims> {
    let header = decode_header(token).context("decode JWT header")?;
    let kid = header.kid.ok_or_else(|| anyhow!("id_token missing kid"))?;
    let alg = header.alg;
    if !matches!(alg, Algorithm::RS256 | Algorithm::RS384 | Algorithm::RS512) {
        return Err(anyhow!("unsupported id_token alg {:?}", alg));
    }

    let jwk = get_jwk(&kid, cfg).await?;
    if jwk.kty != "RSA" {
        return Err(anyhow!("JWK kty {} not supported", jwk.kty));
    }
    let n = jwk.n.ok_or_else(|| anyhow!("JWK missing n"))?;
    let e = jwk.e.ok_or_else(|| anyhow!("JWK missing e"))?;
    let key = DecodingKey::from_rsa_components(&n, &e).context("JWK -> DecodingKey")?;

    let mut validation = Validation::new(alg);
    validation.set_issuer(&[&cfg.issuer]);
    validation.set_audience(&[&cfg.client_id]);
    validation.validate_exp = true;

    let data = decode::<IdTokenClaims>(token, &key, &validation).context("id_token verify")?;
    let _ = (jwk.use_, jwk.alg); // suppress unused
    Ok(data.claims)
}

fn map_to_internal(id: IdTokenClaims, cfg: &OidcConfig, ttl_secs: u64) -> Result<Claims> {
    let username = id
        .extra
        .get(&cfg.username_claim)
        .and_then(|v| v.as_str())
        .or_else(|| id.extra.get("sub").and_then(|v| v.as_str()))
        .ok_or_else(|| anyhow!("id_token missing username claim"))?
        .to_string();

    let admins: HashSet<String> = std::env::var("ADMIN_GROUP")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    let is_admin = admins.contains(&username.to_ascii_lowercase());

    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + ttl_secs;

    Ok(Claims {
        sub: username,
        is_admin,
        exp: exp.try_into().unwrap(),
    })
}

// ---- HTTP handlers ----

use axum::{
    extract::Query,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    Json,
};
use jsonwebtoken::{encode, Header};
use serde::Serialize;

use dutopia::auth::keys;

const STATE_COOKIE: &str = "duapi_oidc_state";
const STATE_TTL_SECS: u64 = 10 * 60;

#[derive(Serialize, Deserialize)]
struct StateCookie {
    state: String,
    pkce_verifier: String,
    exp: usize,
}

#[derive(Serialize)]
pub struct AuthModeResp {
    pub mode: &'static str,
    pub login_url: Option<&'static str>,
}

/// GET /api/auth/mode
pub async fn mode_handler() -> impl IntoResponse {
    if is_enabled() {
        Json(AuthModeResp {
            mode: "oidc",
            login_url: Some("/api/auth/login"),
        })
    } else {
        Json(AuthModeResp {
            mode: "password",
            login_url: None,
        })
    }
}

/// GET /api/auth/login — start OIDC code flow (redirect to IdP).
pub async fn login_handler() -> Response {
    if !is_enabled() {
        return (StatusCode::NOT_FOUND, "OIDC disabled").into_response();
    }
    let state = new_state();
    let (verifier, challenge) = new_pkce();
    let url = match authorize_url(&state, &challenge) {
        Ok(u) => u,
        Err(e) => {
            tracing::error!(err = %e, "authorize_url failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "oidc misconfigured").into_response();
        }
    };
    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as usize
        + STATE_TTL_SECS as usize;
    let cookie_val = match encode(
        &Header::default(),
        &StateCookie {
            state,
            pkce_verifier: verifier,
            exp,
        },
        &keys().encoding,
    ) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(err = %e, "state cookie sign failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "state cookie error").into_response();
        }
    };
    let cookie = format!(
        "{STATE_COOKIE}={cookie_val}; Path=/; Max-Age={STATE_TTL_SECS}; HttpOnly; SameSite=Lax; Secure"
    );
    let mut headers = HeaderMap::new();
    headers.insert(header::SET_COOKIE, cookie.parse().unwrap());
    (headers, Redirect::to(&url)).into_response()
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

/// GET /api/auth/callback — validate state, exchange code, mint internal JWT, redirect to SPA.
pub async fn callback_handler(headers: HeaderMap, Query(q): Query<CallbackQuery>) -> Response {
    if !is_enabled() {
        return (StatusCode::NOT_FOUND, "OIDC disabled").into_response();
    }
    if let Some(err) = q.error {
        tracing::warn!(err = %err, "OIDC IdP returned error");
        return (StatusCode::UNAUTHORIZED, format!("oidc error: {err}")).into_response();
    }
    let code = match q.code {
        Some(c) if !c.is_empty() => c,
        _ => return (StatusCode::BAD_REQUEST, "missing code").into_response(),
    };
    let state = match q.state {
        Some(s) if !s.is_empty() => s,
        _ => return (StatusCode::BAD_REQUEST, "missing state").into_response(),
    };

    let cookie_val = match extract_cookie(&headers, STATE_COOKIE) {
        Some(v) => v,
        None => return (StatusCode::BAD_REQUEST, "missing state cookie").into_response(),
    };
    let mut val = Validation::new(Algorithm::HS256);
    val.validate_exp = true;
    val.set_required_spec_claims(&["exp"]);
    let decoded = match jsonwebtoken::decode::<StateCookie>(&cookie_val, &keys().decoding, &val) {
        Ok(d) => d.claims,
        Err(e) => {
            tracing::warn!(err = %e, "state cookie invalid");
            return (StatusCode::BAD_REQUEST, "invalid state cookie").into_response();
        }
    };
    if decoded.state != state {
        tracing::warn!("state mismatch");
        return (StatusCode::BAD_REQUEST, "state mismatch").into_response();
    }

    const TTL_SECONDS: u64 = 24 * 60 * 60;
    let claims = match exchange_and_verify(&code, &decoded.pkce_verifier, TTL_SECONDS).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(err = %e, "OIDC exchange/verify failed");
            return (StatusCode::UNAUTHORIZED, "oidc verify failed").into_response();
        }
    };
    tracing::info!(user = %claims.sub, is_admin = claims.is_admin, "oidc login success");

    let token = match encode(&Header::default(), &claims, &keys().encoding) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(err = %e, "mint internal jwt failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "token creation error").into_response();
        }
    };

    let cfg = config().expect("enabled");
    let redirect = format!("{}#token={}", cfg.post_login_redirect, token);
    let clear = format!("{STATE_COOKIE}=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax; Secure");
    let mut out = HeaderMap::new();
    out.insert(header::SET_COOKIE, clear.parse().unwrap());
    (out, Redirect::to(&redirect)).into_response()
}

fn extract_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let kv = part.trim();
        if let Some(v) = kv.strip_prefix(&format!("{name}=")) {
            return Some(v.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_base64url_sha256_of_verifier() {
        let (v, c) = new_pkce();
        let expected = URL_SAFE_NO_PAD.encode(Sha256::digest(v.as_bytes()));
        assert_eq!(c, expected);
        assert!(v.len() >= 40);
    }

    #[test]
    fn state_is_unique() {
        let a = new_state();
        let b = new_state();
        assert_ne!(a, b);
        assert!(a.len() >= 24);
    }

    #[test]
    fn authorize_url_errors_when_disabled() {
        // CONFIG not initialized in this unit-test process.
        let r = authorize_url("s", "c");
        assert!(r.is_err());
    }

    #[test]
    fn map_to_internal_uses_username_claim_and_admin_group() {
        let mut extra = HashMap::new();
        extra.insert(
            "preferred_username".to_string(),
            serde_json::Value::String("alice".into()),
        );
        let id = IdTokenClaims { extra };
        let cfg = OidcConfig {
            issuer: "x".into(),
            client_id: "x".into(),
            client_secret: "x".into(),
            redirect_uri: "x".into(),
            scopes: "openid".into(),
            username_claim: "preferred_username".into(),
            authorize_endpoint: "x".into(),
            token_endpoint: "x".into(),
            jwks_uri: "x".into(),
            post_login_redirect: "/".into(),
        };
        unsafe { std::env::set_var("ADMIN_GROUP", "alice,bob") };
        let c = map_to_internal(id, &cfg, 60).unwrap();
        assert_eq!(c.sub, "alice");
        assert!(c.is_admin);
        unsafe { std::env::remove_var("ADMIN_GROUP") };
    }
}
