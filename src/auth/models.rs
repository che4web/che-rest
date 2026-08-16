use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use time::OffsetDateTime;

#[derive(Debug, che_orm2::Model)]
#[orm(table = "auth_users")]
pub struct User {
    #[orm(primary_key)]
    pub id: i64,
    #[orm(unique)]
    pub username: String,
    pub password_hash: String,
    #[orm(default = "true")]
    pub is_active: bool,
    #[orm(default = "false")]
    pub is_staff: bool,
    #[orm(default = "false")]
    pub is_admin: bool,
    #[orm(default = "false")]
    pub is_superuser: bool,
}

#[derive(Debug, che_orm2::Model)]
#[orm(table = "auth_tokens", index("user_id"))]
pub struct AuthToken {
    #[orm(primary_key)]
    pub id: i64,
    #[orm(foreign_key = User, on_delete = "cascade")]
    pub user_id: i64,
    #[orm(unique)]
    pub key_hash: String,
}

#[derive(Debug, che_orm2::Model)]
#[orm(table = "auth_sessions", index("user_id"))]
pub struct AuthSession {
    #[orm(primary_key)]
    pub id: i64,
    #[orm(foreign_key = User, on_delete = "cascade")]
    pub user_id: i64,
    #[orm(unique)]
    pub key_hash: String,
    pub csrf_hash: String,
    pub data: String,
    pub revision: i64,
    pub expires_at: OffsetDateTime,
    #[orm(auto_now_add)]
    pub created_at: OffsetDateTime,
}

pub fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    Ok(Argon2::default()
        .hash_password(password.as_bytes(), &salt)?
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
