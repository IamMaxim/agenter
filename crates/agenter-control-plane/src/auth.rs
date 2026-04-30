use agenter_core::UserId;
use argon2::{password_hash::SaltString, Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use axum::http::{header, HeaderMap};
use rand_core::OsRng;
use serde::Serialize;

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

pub fn session_cookie(token: &str) -> String {
    format!("{SESSION_COOKIE_NAME}={token}; HttpOnly; SameSite=Lax; Path=/")
}

pub fn expired_session_cookie() -> String {
    format!("{SESSION_COOKIE_NAME}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0")
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
}
