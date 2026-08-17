use std::path::Path;

use crate::error::AppResult;

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub database: DatabaseConfig,
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub auth: AuthConfig,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub api_prefix: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 3000,
            api_prefix: "/api".to_string(),
        }
    }
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
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

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            cookie_name: "che_rest_session".to_string(),
            csrf_cookie_name: "csrf_token".to_string(),
            ttl_seconds: 604_800,
            secure: false,
            same_site: "Lax".to_string(),
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: "sqlite://db.sqlite?mode=rwc".to_string(),
            max_connections: 10,
        }
    }
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
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 3000);
        assert_eq!(config.server.api_prefix, "/api");
    }
}
