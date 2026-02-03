use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordVerifier};

/// Verify a candidate password against an argon2 hash string.
///
/// Returns `true` if the password matches.
pub fn verify_password(hash: &str, candidate: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(candidate.as_bytes(), &parsed)
        .is_ok()
}
