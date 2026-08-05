use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use che_orm::{Model, NaiveDateTime};

#[derive(Debug, Clone, Model)]
#[model(table = "auth_users")]
pub struct User {
    #[field(primary_key)]
    pub id: i64,

    #[field(unique, max_length = 150)]
    pub username: String,

    pub password_hash: String,

    #[field(default = true)]
    pub is_active: bool,

    #[field(default = false)]
    pub is_staff: bool,

    #[field(default = false)]
    pub is_admin: bool,

    #[field(default = false)]
    pub is_superuser: bool,
}

#[derive(Debug, Clone, Model)]
#[model(table = "auth_tokens")]
pub struct AuthToken {
    #[field(primary_key)]
    pub id: i64,

    #[field(foreign_key = User)]
    pub user_id: i64,

    #[field(unique, max_length = 64)]
    pub key_hash: String,
}

#[derive(Debug, Clone, Model)]
#[model(table = "auth_sessions")]
pub struct AuthSession {
    #[field(primary_key)]
    pub id: i64,

    #[field(foreign_key = User)]
    pub user_id: i64,

    #[field(unique, max_length = 64)]
    pub key_hash: String,

    pub csrf_hash: String,
    pub data: String,
    pub revision: i64,
    pub expires_at: NaiveDateTime,

    #[field(auto_now_add)]
    pub created_at: NaiveDateTime,
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
