use std::path::Path;

use crate::error::AppResult;

#[derive(Debug, Clone, serde::Deserialize)]
pub struct AppConfig {
    pub database: DatabaseConfig,
    #[serde(default)]
    pub auth: AuthConfig,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    pub session: SessionConfig,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    pub cookie_name: String,
    pub csrf_cookie_name: String,
    pub ttl_seconds: i64,
    pub secure: bool,
    pub same_site: String,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            session: SessionConfig::default(),
        }
    }
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            cookie_name: "che_rest_session".to_string(),
            csrf_cookie_name: "che_rest_csrf".to_string(),
            ttl_seconds: 604_800,
            secure: false,
            same_site: "Lax".to_string(),
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
}

impl AppConfig {
    pub fn from_file(path: impl AsRef<Path>) -> AppResult<Self> {
        let content = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&content)?)
    }
}

#[cfg(test)]
mod tests {
    use super::AppConfig;

    #[test]
    fn session_config_is_optional() {
        let config: AppConfig = toml::from_str("[database]\nurl = 'sqlite://db.sqlite'").unwrap();
        assert_eq!(config.auth.session.cookie_name, "che_rest_session");
        assert_eq!(config.auth.session.ttl_seconds, 604_800);
    }
}
