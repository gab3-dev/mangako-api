use std::{env, net::SocketAddr, path::PathBuf, str::FromStr, time::Duration};

use crate::{
    auth::AuthConfig,
    cover_storage::CoverStorage,
    error::ApiError,
    operations::{CacheConfig, OperationalConfig, RateLimitConfig},
};

pub struct Config {
    pub database_url: String,
    pub http_addr: SocketAddr,
    pub auth: AuthConfig,
    pub operations: OperationalConfig,
    pub cover_storage: CoverStorage,
}

impl Config {
    pub fn from_env() -> Result<Self, ApiError> {
        let database_url = env::var("DATABASE_URL").map_err(|_| ApiError::MissingEnv {
            name: "DATABASE_URL",
        })?;

        let http_addr = env::var("HTTP_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:3000".to_string())
            .parse()?;
        let read_token = required_token("API_READ_TOKEN")?;
        let write_token = required_token("API_WRITE_TOKEN")?;
        if read_token == write_token {
            return Err(ApiError::InvalidEnv {
                name: "API_WRITE_TOKEN",
                message: "must differ from API_READ_TOKEN".to_string(),
            });
        }
        let cache_ttl_seconds = env_or("CACHE_TTL_SECONDS", 60)?;
        let cache_max_entries = env_or("CACHE_MAX_ENTRIES", 1_000)?;
        let cache_max_bytes = env_or("CACHE_MAX_BYTES", 64 * 1024 * 1024)?;
        let max_json_body_bytes = env_or("MAX_JSON_BODY_BYTES", 64 * 1024)?;
        let requests_per_minute = env_or("RATE_LIMIT_REQUESTS_PER_MINUTE", 120)?;
        let rate_limit_max_clients = env_or("RATE_LIMIT_MAX_CLIENTS", 10_000)?;
        let trust_proxy_headers = env_or("TRUST_PROXY_HEADERS", false)?;
        let cover_storage_dir =
            env::var("COVER_STORAGE_DIR").unwrap_or_else(|_| "/var/lib/mangako/covers".to_string());
        let cover_public_base_url = env::var("COVER_PUBLIC_BASE_URL")
            .unwrap_or_else(|_| "http://localhost:3000/media".to_string());
        let cover_max_upload_bytes = env_or("MAX_COVER_UPLOAD_BYTES", 8 * 1024 * 1024)?;

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
        if max_json_body_bytes == 0 {
            return Err(ApiError::InvalidEnv {
                name: "MAX_JSON_BODY_BYTES",
                message: "must be greater than zero".to_string(),
            });
        }
        if cover_max_upload_bytes == 0 || cover_max_upload_bytes > 32 * 1024 * 1024 {
            return Err(ApiError::InvalidEnv {
                name: "MAX_COVER_UPLOAD_BYTES",
                message: "must be between 1 and 33554432".to_string(),
            });
        }
        if requests_per_minute == 0 || rate_limit_max_clients == 0 {
            return Err(ApiError::InvalidEnv {
                name: "RATE_LIMIT_REQUESTS_PER_MINUTE",
                message: "rate limit values must be greater than zero".to_string(),
            });
        }

        let operations = OperationalConfig {
            cache: (cache_ttl_seconds > 0).then(|| CacheConfig {
                ttl: Duration::from_secs(cache_ttl_seconds),
                max_entries: cache_max_entries,
                max_bytes: cache_max_bytes,
            }),
            rate_limit: RateLimitConfig {
                requests_per_minute,
                max_clients: rate_limit_max_clients,
                trust_proxy_headers,
            },
            max_json_body_bytes,
        };

        Ok(Self {
            database_url,
            http_addr,
            auth: AuthConfig {
                read_token,
                write_token,
            },
            operations,
            cover_storage: CoverStorage::new(
                PathBuf::from(cover_storage_dir),
                cover_public_base_url,
                cover_max_upload_bytes,
            ),
        })
    }
}

fn required_token(name: &'static str) -> Result<String, ApiError> {
    let token = env::var(name).map_err(|_| ApiError::MissingEnv { name })?;
    if token.len() < 32 || token.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return Err(ApiError::InvalidEnv {
            name,
            message: "must contain at least 32 non-whitespace characters".to_string(),
        });
    }
    Ok(token)
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
