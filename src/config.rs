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
    #[serde(default)]
    pub media: MediaConfig,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct MediaConfig {
    pub root: String,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            root: "uploads".into(),
        }
    }
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
    pub renewal_window_seconds: i64,
    pub absolute_ttl_seconds: i64,
    pub secure: bool,
    pub same_site: String,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            cookie_name: "che_rest_session".to_string(),
            csrf_cookie_name: "csrf_token".to_string(),
            ttl_seconds: 604_800,
            renewal_window_seconds: 86_400,
            absolute_ttl_seconds: 2_592_000,
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
            url: default_database_url().to_string(),
            max_connections: 10,
        }
    }
}

impl DatabaseConfig {
    pub(crate) async fn connect(&self) -> Result<che_orm::Database, che_orm::OrmError> {
        let url = self.connection_url()?;
        let pool_size = if cfg!(feature = "sqlite") && url == ":memory:" {
            1
        } else {
            self.max_connections as usize
        };
        che_orm::Database::connect_with_pool_size(url, pool_size).await
    }

    fn connection_url(&self) -> Result<String, che_orm::OrmError> {
        #[cfg(feature = "sqlite")]
        {
            if self.url.contains("://") && !self.url.starts_with("sqlite://") {
                return Err(che_orm::OrmError::Configuration(
                    "this build requires a SQLite URL or file path".into(),
                ));
            }
            Ok(sqlite_path(&self.url))
        }
        #[cfg(all(feature = "postgres", not(feature = "sqlite")))]
        {
            if !(self.url.starts_with("postgres://") || self.url.starts_with("postgresql://")) {
                return Err(che_orm::OrmError::Configuration(
                    "this build requires a postgres:// or postgresql:// URL".into(),
                ));
            }
            // Preserve credentials, TLS, options/search_path and all query parameters.
            Ok(self.url.clone())
        }
    }
}

fn default_database_url() -> &'static str {
    if cfg!(feature = "postgres") {
        "postgresql://localhost/che_rest"
    } else {
        "sqlite://db.sqlite?mode=rwc"
    }
}

pub(crate) fn backend_name() -> &'static str {
    if cfg!(feature = "postgres") {
        "PostgreSQL"
    } else {
        "SQLite"
    }
}

pub(crate) fn sqlite_path(url: &str) -> String {
    url.strip_prefix("sqlite://")
        .and_then(|value| value.split('?').next())
        .filter(|value| !value.is_empty())
        .unwrap_or(url)
        .to_string()
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
    fn backend_urls_are_validated_without_disclosing_credentials() {
        let mut config = super::DatabaseConfig::default();
        #[cfg(feature = "postgres")]
        {
            config.url = "postgresql://user:secret@localhost/app?sslmode=require&options=-csearch_path%3Dtenant".into();
            assert_eq!(config.connection_url().unwrap(), config.url);
            config.url = "sqlite://wrong.sqlite".into();
        }
        #[cfg(feature = "sqlite")]
        {
            config.url = "postgresql://user:secret@localhost/app".into();
        }
        let error = config.connection_url().unwrap_err().to_string();
        assert!(!error.contains("secret"));
        assert!(!error.contains(&config.url));
    }

    #[test]
    fn session_config_is_optional() {
        let config: AppConfig = toml::from_str("[database]\nurl = 'sqlite://db.sqlite'").unwrap();
        assert_eq!(config.auth.session.cookie_name, "che_rest_session");
        assert_eq!(config.auth.session.ttl_seconds, 604_800);
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 3000);
        assert_eq!(config.server.api_prefix, "/api");
        assert_eq!(config.media.root, "uploads");
    }
}
