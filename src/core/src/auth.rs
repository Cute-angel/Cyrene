use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use uuid::Uuid;

use crate::error::{CoreError, CoreResult};

#[must_use]
pub fn generate_token() -> String {
    format!("cyr_{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

pub fn hash_token(token: &str) -> CoreResult<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(token.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| CoreError::Internal(format!("could not hash access token: {error}")))
}

#[must_use]
pub fn verify_token(token: &str, encoded_hash: &str) -> bool {
    let Ok(hash) = PasswordHash::new(encoded_hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(token.as_bytes(), &hash)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_round_trip() {
        let token = generate_token();
        let hash = hash_token(&token).unwrap();
        assert!(verify_token(&token, &hash));
        assert!(!verify_token("wrong", &hash));
        assert!(!hash.contains(&token));
    }
}
