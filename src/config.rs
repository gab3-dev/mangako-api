use std::{env, net::SocketAddr, str::FromStr, time::Duration};

use crate::{
    error::ApiError,
    operations::{CacheConfig, OperationalConfig},
};

pub struct Config {
    pub database_url: String,
    pub http_addr: SocketAddr,
    pub api_token: String,
    pub operations: OperationalConfig,
}

impl Config {
    pub fn from_env() -> Result<Self, ApiError> {
        let database_url = env::var("DATABASE_URL").map_err(|_| ApiError::MissingEnv {
            name: "DATABASE_URL",
        })?;

        let http_addr = env::var("HTTP_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:3000".to_string())
            .parse()?;
        let api_token =
            env::var("API_TOKEN").map_err(|_| ApiError::MissingEnv { name: "API_TOKEN" })?;
        let cache_ttl_seconds = env_or("CACHE_TTL_SECONDS", 60)?;
        let cache_max_entries = env_or("CACHE_MAX_ENTRIES", 1_000)?;
        let cache_max_bytes = env_or("CACHE_MAX_BYTES", 64 * 1024 * 1024)?;

        if cache_ttl_seconds > 0 {
            if cache_max_entries == 0 {
                return Err(ApiError::InvalidEnv {
                    name: "CACHE_MAX_ENTRIES",
                    message: "must be greater than zero when caching is enabled".to_string(),
                });
            }
            if cache_max_bytes == 0 {
                return Err(ApiError::InvalidEnv {
                    name: "CACHE_MAX_BYTES",
                    message: "must be greater than zero when caching is enabled".to_string(),
                });
            }
        }

        let operations = OperationalConfig {
            cache: (cache_ttl_seconds > 0).then(|| CacheConfig {
                ttl: Duration::from_secs(cache_ttl_seconds),
                max_entries: cache_max_entries,
                max_bytes: cache_max_bytes,
            }),
        };

        Ok(Self {
            database_url,
            http_addr,
            api_token,
            operations,
        })
    }
}

fn env_or<T>(name: &'static str, default: T) -> Result<T, ApiError>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    match env::var(name) {
        Ok(value) => value.parse().map_err(|error: T::Err| ApiError::InvalidEnv {
            name,
            message: error.to_string(),
        }),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(ApiError::InvalidEnv {
            name,
            message: error.to_string(),
        }),
    }
}
