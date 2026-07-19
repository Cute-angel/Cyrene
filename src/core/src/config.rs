use std::{
    fs,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, CoreResult};

pub const DEFAULT_BIND: &str = "127.0.0.1:46371";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CoreConfig {
    pub database: PathBuf,
    pub http_bind: SocketAddr,
    pub embedding: EmbeddingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbeddingConfig {
    pub enabled: bool,
    pub base_url: String,
    pub api_key_env: String,
    pub model: String,
    pub dimensions: usize,
    pub request_timeout_seconds: u64,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            base_url: "https://api.openai.com/v1".into(),
            api_key_env: "OPENAI_API_KEY".into(),
            model: "text-embedding-3-small".into(),
            dimensions: 512,
            request_timeout_seconds: 30,
        }
    }
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            database: PathBuf::from("cyrene.db"),
            http_bind: DEFAULT_BIND.parse().expect("valid default bind address"),
            embedding: EmbeddingConfig::default(),
        }
    }
}

impl CoreConfig {
    pub fn default_data_dir() -> CoreResult<PathBuf> {
        dirs::home_dir()
            .map(|path| path.join(".Cyrene").join("data"))
            .ok_or_else(|| CoreError::Config("could not determine the user home directory".into()))
    }

    pub fn initialize(data_dir: &Path) -> CoreResult<Self> {
        fs::create_dir_all(data_dir).map_err(|error| {
            CoreError::Config(format!("could not create {}: {error}", data_dir.display()))
        })?;
        let config = Self::default();
        config.validate()?;
        let path = data_dir.join("config.toml");
        if !path.exists() {
            let encoded = toml::to_string_pretty(&config).map_err(|error| {
                CoreError::Config(format!("could not encode configuration: {error}"))
            })?;
            fs::write(&path, encoded).map_err(|error| {
                CoreError::Config(format!("could not write {}: {error}", path.display()))
            })?;
        }
        Ok(config.resolve(data_dir))
    }

    pub fn load(data_dir: &Path) -> CoreResult<Self> {
        let path = data_dir.join("config.toml");
        let raw = fs::read_to_string(&path).map_err(|error| {
            CoreError::Config(format!("could not read {}: {error}", path.display()))
        })?;
        let config: Self = toml::from_str(&raw).map_err(|error| {
            CoreError::Config(format!("could not parse {}: {error}", path.display()))
        })?;
        config.validate()?;
        Ok(config.resolve(data_dir))
    }

    pub fn validate(&self) -> CoreResult<()> {
        if !self.http_bind.ip().is_loopback() {
            return Err(CoreError::Config(format!(
                "HTTP bind address must be loopback, got {}",
                self.http_bind.ip()
            )));
        }
        let base_url = reqwest::Url::parse(self.embedding.base_url.trim()).map_err(|error| {
            CoreError::Config(format!("embedding base URL is invalid: {error}"))
        })?;
        if !matches!(base_url.scheme(), "http" | "https") || base_url.host().is_none() {
            return Err(CoreError::Config(
                "embedding base URL must be an absolute HTTP or HTTPS URL".into(),
            ));
        }
        if base_url.query().is_some() || base_url.fragment().is_some() {
            return Err(CoreError::Config(
                "embedding base URL must not contain a query or fragment".into(),
            ));
        }
        if self.embedding.model.trim().is_empty() {
            return Err(CoreError::Config(
                "embedding model must not be empty".into(),
            ));
        }
        if self.embedding.dimensions == 0 {
            return Err(CoreError::Config(
                "embedding dimensions must be positive".into(),
            ));
        }
        if self.embedding.request_timeout_seconds == 0 {
            return Err(CoreError::Config(
                "embedding request timeout must be positive".into(),
            ));
        }
        Ok(())
    }

    fn resolve(mut self, data_dir: &Path) -> Self {
        if self.database.is_relative() {
            self.database = data_dir.join(&self.database);
        }
        self
    }

    pub fn with_bind(mut self, bind: Option<SocketAddr>) -> CoreResult<Self> {
        if let Some(bind) = bind {
            self.http_bind = bind;
        }
        self.validate()?;
        Ok(self)
    }

    #[must_use]
    pub const fn is_loopback(ip: IpAddr) -> bool {
        ip.is_loopback()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_loopback_bind() {
        let config = CoreConfig {
            http_bind: "0.0.0.0:46371".parse().unwrap(),
            ..CoreConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn accepts_openai_compatible_embedding_configuration() {
        let mut config = CoreConfig::default();
        config.embedding.base_url = "http://localhost:11434/v1".into();
        config.embedding.api_key_env.clear();
        config.embedding.model = "nomic-embed-text".into();
        config.embedding.dimensions = 768;

        assert!(config.validate().is_ok());
    }

    #[test]
    fn older_config_gets_openai_compatible_defaults() {
        let config: CoreConfig = toml::from_str(
            r#"
                database = "cyrene.db"
                http_bind = "127.0.0.1:46371"

                [embedding]
                enabled = true
                model = "text-embedding-3-small"
                dimensions = 512
                request_timeout_seconds = 30
            "#,
        )
        .unwrap();

        assert_eq!(config.embedding.base_url, "https://api.openai.com/v1");
        assert_eq!(config.embedding.api_key_env, "OPENAI_API_KEY");
    }

    #[test]
    fn rejects_invalid_embedding_configuration() {
        let mut config = CoreConfig::default();
        config.embedding.base_url = "file:///tmp/v1".into();
        assert!(config.validate().is_err());

        config.embedding.base_url = "http://localhost:8000/v1".into();
        config.embedding.model.clear();
        assert!(config.validate().is_err());

        config.embedding.model = "model".into();
        config.embedding.dimensions = 0;
        assert!(config.validate().is_err());
    }
}
