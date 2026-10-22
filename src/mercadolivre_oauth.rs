use std::{sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use reqwest::{Client, StatusCode, Url};
use serde::Deserialize;
use sqlx::{FromRow, PgPool, Postgres, Transaction};

use crate::error::ApiError;

// Do not derive Debug/Serialize for types that contain OAuth credentials.
pub struct MercadoLivreOAuthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub initial_access_token: Option<String>,
    pub initial_refresh_token: Option<String>,
    pub initial_expires_at: Option<DateTime<Utc>>,
}

#[derive(Clone)]
pub struct MercadoLivreOAuth {
    pool: PgPool,
    http: Client,
    config: Arc<MercadoLivreOAuthConfig>,
    token_url: Url,
}

#[derive(FromRow)]
struct StoredTokens {
    access_token: String,
    refresh_token: String,
    expires_at: DateTime<Utc>,
    retry_after: Option<DateTime<Utc>>,
    reauthorization_required: bool,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
}

#[derive(Deserialize)]
struct TokenError {
    error: String,
}

impl MercadoLivreOAuth {
    pub fn new(pool: PgPool, http: Client, config: MercadoLivreOAuthConfig) -> Self {
        Self::with_base_url(pool, http, config, "https://api.mercadolibre.com/")
    }

    pub fn with_base_url(
        pool: PgPool,
        http: Client,
        config: MercadoLivreOAuthConfig,
        base_url: &str,
    ) -> Self {
        Self {
            pool,
            http,
            config: Arc::new(config),
            token_url: Url::parse(base_url)
                .expect("valid Mercado Livre API URL")
                .join("oauth/token")
                .expect("valid OAuth token URL"),
        }
    }

    /// Environment values seed a new client only; restarts must never replace rotated tokens.
    pub async fn initialize(&self) -> Result<(), ApiError> {
        let Some(refresh_token) = self.config.initial_refresh_token.as_deref() else {
            return Ok(());
        };
        let access_token = self.config.initial_access_token.as_deref().unwrap_or("");
        if !valid_token(refresh_token) || (!access_token.is_empty() && !valid_token(access_token)) {
            return Err(ApiError::MercadoLivreOAuthUnavailable);
        }
        sqlx::query(
            "INSERT INTO mercadolivre_oauth_tokens (client_id, access_token, refresh_token, expires_at)
             VALUES ($1, $2, $3, $4) ON CONFLICT (client_id) DO NOTHING",
        )
        .bind(&self.config.client_id)
        .bind(access_token)
        .bind(refresh_token)
        .bind(self.config.initial_expires_at.unwrap_or_else(Utc::now))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn access_token(&self) -> Result<String, ApiError> {
        self.token(None).await
    }

    pub async fn after_unauthorized(&self, rejected: &str) -> Result<String, ApiError> {
        self.token(Some(rejected)).await
    }

    /// A fresh token rejected after the one allowed retry must not cause a refresh storm.
    pub async fn mark_rejected(&self, rejected: &str) -> Result<(), ApiError> {
        sqlx::query(
            "UPDATE mercadolivre_oauth_tokens SET retry_after = now() + interval '60 seconds'
             WHERE client_id = $1 AND access_token = $2",
        )
        .bind(&self.config.client_id)
        .bind(rejected)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn token(&self, rejected: Option<&str>) -> Result<String, ApiError> {
        let mut transaction = self.pool.begin().await?;
        // The row lock serializes refreshes across requests AND across API instances.
        let stored: StoredTokens = sqlx::query_as(
            "SELECT access_token, refresh_token, expires_at, retry_after, reauthorization_required
             FROM mercadolivre_oauth_tokens WHERE client_id = $1 FOR UPDATE",
        )
        .bind(&self.config.client_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(ApiError::MercadoLivreOAuthUnavailable)?;
        let now = Utc::now();
        if stored.reauthorization_required || stored.retry_after.is_some_and(|retry| retry > now) {
            return Err(ApiError::MercadoLivreOAuthUnavailable);
        }
        if !stored.access_token.is_empty()
            && stored.expires_at > now + chrono::Duration::seconds(60)
            && rejected.is_none_or(|rejected| rejected != stored.access_token)
        {
            transaction.commit().await?;
            return Ok(stored.access_token);
        }

        let response = self
            .http
            .post(self.token_url.clone())
            .header(reqwest::header::ACCEPT, "application/json")
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", self.config.client_id.as_str()),
                ("client_secret", self.config.client_secret.as_str()),
                ("refresh_token", stored.refresh_token.as_str()),
            ])
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(_) => return self.failed(transaction, false).await,
        };
        if !response.status().is_success() {
            let status = response.status();
            let permanent =
                if status == StatusCode::BAD_REQUEST || status == StatusCode::UNAUTHORIZED {
                    response
                        .json::<TokenError>()
                        .await
                        .ok()
                        .is_some_and(|error| {
                            matches!(
                                error.error.as_str(),
                                "invalid_grant" | "invalid_client" | "unauthorized_client"
                            )
                        })
                } else {
                    false
                };
            // Do not log response bodies, form data, or reqwest Debug values containing secrets.
            tracing::warn!(%status, reauthorization_required = permanent, "Mercado Livre OAuth refresh rejected");
            return self.failed(transaction, permanent).await;
        }
        let tokens = match response.json::<TokenResponse>().await {
            Ok(tokens)
                if valid_token(&tokens.access_token)
                    && valid_token(&tokens.refresh_token)
                    && (1..=31_536_000).contains(&tokens.expires_in) =>
            {
                tokens
            }
            // A successful exchange may have consumed the old refresh token: do not blindly reuse it.
            _ => return self.failed(transaction, true).await,
        };
        let expires_at = Utc::now() + chrono::Duration::seconds(tokens.expires_in);
        sqlx::query(
            "UPDATE mercadolivre_oauth_tokens SET access_token = $2, refresh_token = $3,
             expires_at = $4, updated_at = now(), retry_after = NULL, reauthorization_required = false
             WHERE client_id = $1",
        )
        .bind(&self.config.client_id)
        .bind(&tokens.access_token)
        .bind(&tokens.refresh_token)
        .bind(expires_at)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        tracing::info!(%expires_at, "Mercado Livre OAuth tokens renewed and persisted");
        Ok(tokens.access_token)
    }

    async fn failed(
        &self,
        mut transaction: Transaction<'_, Postgres>,
        permanent: bool,
    ) -> Result<String, ApiError> {
        sqlx::query(
            "UPDATE mercadolivre_oauth_tokens SET retry_after = now() + interval '60 seconds',
             reauthorization_required = $2 WHERE client_id = $1",
        )
        .bind(&self.config.client_id)
        .bind(permanent)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Err(ApiError::MercadoLivreOAuthUnavailable)
    }

    pub async fn run_refresh_loop(self) {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut previously_failed = false;
        loop {
            interval.tick().await;
            let failed = self.access_token().await.is_err();
            if failed && !previously_failed {
                tracing::warn!(
                    "Mercado Livre OAuth renewal unavailable; check authorization and credentials"
                );
            } else if !failed && previously_failed {
                tracing::info!("Mercado Livre OAuth renewal recovered");
            }
            previously_failed = failed;
        }
    }
}

fn valid_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 16_384
        && !value.chars().any(char::is_whitespace)
        && !value.chars().any(char::is_control)
}
