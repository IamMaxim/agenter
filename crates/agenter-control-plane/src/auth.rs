use agenter_core::UserId;
use argon2::{password_hash::SaltString, Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use axum::http::{header, HeaderMap};
use chrono::Duration;
use rand_core::OsRng;
use serde::Serialize;
use sha2::{Digest, Sha256};

pub const SESSION_COOKIE_NAME: &str = "agenter_session";

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AuthenticatedUser {
    pub user_id: UserId,
    pub email: String,
    pub display_name: Option<String>,
}

#[derive(Clone, Debug)]
pub struct BootstrapAdmin {
    pub user: AuthenticatedUser,
    pub password_hash: String,
}

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Ok(Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(anyhow::Error::msg)?
        .to_string())
}

pub fn verify_password(password: &str, password_hash: &str) -> bool {
    let Ok(parsed_hash) = PasswordHash::new(password_hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok()
}

pub fn session_token_from_headers(headers: &HeaderMap) -> Option<&str> {
    let cookie = headers.get(header::COOKIE)?.to_str().ok()?;
    cookie.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name == SESSION_COOKIE_NAME && !value.is_empty()).then_some(value)
    })
}

pub fn session_token_hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CookieSecurity {
    Secure,
    DevelopmentInsecure,
}

pub fn session_cookie_with_policy(
    token: &str,
    security: CookieSecurity,
    max_age: Option<Duration>,
) -> String {
    let secure = match security {
        CookieSecurity::Secure => "; Secure",
        CookieSecurity::DevelopmentInsecure => "",
    };
    let max_age = max_age
        .map(|duration| format!("; Max-Age={}", duration.num_seconds()))
        .unwrap_or_default();
    format!("{SESSION_COOKIE_NAME}={token}; HttpOnly; SameSite=Lax; Path=/{max_age}{secure}")
}

pub fn expired_session_cookie_with_policy(security: CookieSecurity) -> String {
    let secure = match security {
        CookieSecurity::Secure => "; Secure",
        CookieSecurity::DevelopmentInsecure => "",
    };
    format!("{SESSION_COOKIE_NAME}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0{secure}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argon2id_hash_verifies_original_password_only() {
        let hash = hash_password("correct horse battery staple").expect("hash password");

        assert_ne!(hash, "correct horse battery staple");
        assert!(hash.starts_with("$argon2id$"));
        assert!(verify_password("correct horse battery staple", &hash));
        assert!(!verify_password("wrong password", &hash));
    }

    #[test]
    fn session_cookie_defaults_to_secure_and_supports_explicit_dev_policy() {
        let secure = session_cookie_with_policy("token-1", CookieSecurity::Secure, None);
        assert!(secure.contains("Secure"));
        assert!(secure.contains("HttpOnly"));
        assert!(secure.contains("SameSite=Lax"));

        let dev = session_cookie_with_policy("token-1", CookieSecurity::DevelopmentInsecure, None);
        assert!(!dev.contains("Secure"));
    }

    #[test]
    fn session_token_hash_is_stable_and_does_not_expose_raw_token() {
        let hash = session_token_hash("token-1");

        assert_eq!(hash, session_token_hash("token-1"));
        assert_ne!(hash, session_token_hash("token-2"));
        assert_ne!(hash, "token-1");
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn session_cookie_can_set_30_day_max_age() {
        let cookie = session_cookie_with_policy(
            "token-1",
            CookieSecurity::DevelopmentInsecure,
            Some(chrono::Duration::days(30)),
        );

        assert!(cookie.contains("Max-Age=2592000"));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Lax"));
    }
}
