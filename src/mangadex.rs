use std::{collections::HashMap, future::Future, sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use serde::Deserialize;
use tokio::{
    sync::{Mutex, OwnedSemaphorePermit, Semaphore},
    time::{Instant, sleep_until},
};
use uuid::Uuid;

use crate::operations::RequestMetrics;

const MANGADEX_MAX_CONCURRENT_REQUESTS: usize = 1;
const MANGADEX_MIN_REQUEST_INTERVAL: Duration = Duration::from_millis(250);
const MANGADEX_FAILURE_THRESHOLD: u32 = 3;
const MANGADEX_CIRCUIT_COOLDOWN: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, thiserror::Error)]
pub enum MangaDexError {
    #[error("MangaDex circuit breaker is open")]
    CircuitOpen,
    #[error(transparent)]
    Request(#[from] reqwest::Error),
}

pub trait MangaDexApi: Clone + Send + Sync + 'static {
    fn get_manga(
        &self,
        id: Uuid,
    ) -> impl Future<Output = Result<Option<MangaDexManga>, MangaDexError>> + Send;

    fn search_mangas(
        &self,
        title: Option<&str>,
        offset: u32,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<MangaDexManga>, MangaDexError>> + Send;

    fn list_covers(
        &self,
        manga_id: Uuid,
        offset: u32,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<MangaDexCover>, MangaDexError>> + Send;

    fn latest_volume_number(
        &self,
        manga_id: Uuid,
        language: &str,
    ) -> impl Future<Output = Result<Option<String>, MangaDexError>> + Send;
}

#[derive(Clone)]
struct MangaDexCircuitBreaker {
    state: Arc<Mutex<MangaDexCircuitState>>,
    failure_threshold: u32,
    cooldown: Duration,
}

struct MangaDexCircuitState {
    consecutive_failures: u32,
    open_until: Option<Instant>,
}

impl MangaDexCircuitBreaker {
    fn new(failure_threshold: u32, cooldown: Duration) -> Self {
        Self {
            state: Arc::new(Mutex::new(MangaDexCircuitState {
                consecutive_failures: 0,
                open_until: None,
            })),
            failure_threshold,
            cooldown,
        }
    }

    async fn allows_request(&self) -> bool {
        let mut state = self.state.lock().await;
        if let Some(open_until) = state.open_until {
            if open_until > Instant::now() {
                return false;
            }
            state.open_until = None;
            state.consecutive_failures = 0;
        }
        true
    }

    async fn record_result(&self, success: bool) {
        let mut state = self.state.lock().await;
        if success {
            state.consecutive_failures = 0;
            state.open_until = None;
            return;
        }
        state.consecutive_failures += 1;
        if state.consecutive_failures >= self.failure_threshold {
            state.open_until = Some(Instant::now() + self.cooldown);
        }
    }
}

#[derive(Clone)]
struct MangaDexRequestLimiter {
    permits: Arc<Semaphore>,
    next_request_at: Arc<Mutex<Instant>>,
    min_interval: Duration,
}

impl MangaDexRequestLimiter {
    fn new(max_concurrent_requests: usize, min_interval: Duration) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(max_concurrent_requests)),
            next_request_at: Arc::new(Mutex::new(Instant::now())),
            min_interval,
        }
    }

    async fn acquire(&self) -> OwnedSemaphorePermit {
        let permit = self
            .permits
            .clone()
            .acquire_owned()
            .await
            .expect("MangaDex request limiter semaphore is never closed");
        let mut next_request_at = self.next_request_at.lock().await;
        let now = Instant::now();
        let scheduled_at = (*next_request_at).max(now);
        *next_request_at = scheduled_at + self.min_interval;
        drop(next_request_at);

        if scheduled_at > now {
            sleep_until(scheduled_at).await;
        }
        permit
    }
}

#[derive(Clone)]
pub struct MangaDexClient {
    http: reqwest::Client,
    base_url: String,
    limiter: MangaDexRequestLimiter,
    circuit_breaker: MangaDexCircuitBreaker,
    metrics: RequestMetrics,
}

impl MangaDexClient {
    pub fn new(http: reqwest::Client, metrics: RequestMetrics) -> Self {
        Self {
            http,
            base_url: "https://api.mangadex.org".to_string(),
            limiter: MangaDexRequestLimiter::new(
                MANGADEX_MAX_CONCURRENT_REQUESTS,
                MANGADEX_MIN_REQUEST_INTERVAL,
            ),
            circuit_breaker: MangaDexCircuitBreaker::new(
                MANGADEX_FAILURE_THRESHOLD,
                MANGADEX_CIRCUIT_COOLDOWN,
            ),
            metrics,
        }
    }

    async fn send(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, MangaDexError> {
        let _permit = self.limiter.acquire().await;
        if !self.circuit_breaker.allows_request().await {
            return Err(MangaDexError::CircuitOpen);
        }
        let response = request.send().await;
        let success = response
            .as_ref()
            .is_ok_and(|response| response.status().is_success());
        if let Err(error) = self.metrics.record_mangadex_result(success).await {
            tracing::warn!(%error, "failed to record MangaDex request metric");
        }
        self.circuit_breaker.record_result(success).await;
        response.map_err(MangaDexError::from)
    }
}

impl MangaDexApi for MangaDexClient {
    async fn get_manga(&self, id: Uuid) -> Result<Option<MangaDexManga>, MangaDexError> {
        let response = self
            .send(
                self.http
                    .get(format!("{}/manga/{id}", self.base_url))
                    .query(&[
                        ("includes[]", "cover_art"),
                        ("includes[]", "author"),
                        ("includes[]", "artist"),
                    ]),
            )
            .await?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        Ok(Some(
            response
                .error_for_status()?
                .json::<MangaResponse>()
                .await?
                .data,
        ))
    }

    async fn search_mangas(
        &self,
        title: Option<&str>,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<MangaDexManga>, MangaDexError> {
        let query = manga_search_query(title, offset, limit);

        let response = self
            .send(
                self.http
                    .get(format!("{}/manga", self.base_url))
                    .query(&query),
            )
            .await?;

        let body = response
            .error_for_status()?
            .json::<MangaListResponse>()
            .await?;
        Ok(body.data)
    }

    async fn list_covers(
        &self,
        manga_id: Uuid,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<MangaDexCover>, MangaDexError> {
        let response = self
            .send(self.http.get(format!("{}/cover", self.base_url)).query(&[
                ("limit", limit.to_string()),
                ("offset", offset.to_string()),
                ("manga[]", manga_id.to_string()),
                ("order[volume]", "asc".to_string()),
            ]))
            .await?;

        Ok(response
            .error_for_status()?
            .json::<CoverListResponse>()
            .await?
            .data)
    }

    async fn latest_volume_number(
        &self,
        manga_id: Uuid,
        language: &str,
    ) -> Result<Option<String>, MangaDexError> {
        let response = self
            .send(
                self.http
                    .get(format!("{}/cover", self.base_url))
                    .query(&latest_volume_query(manga_id)),
            )
            .await?;

        Ok(response
            .error_for_status()?
            .json::<CoverListResponse>()
            .await?
            .data
            .into_iter()
            .find_map(|cover| {
                let volume = cover.attributes.volume?;
                (matches_language(cover.attributes.locale.as_deref(), language)
                    && is_regular_volume(&volume))
                .then_some(volume)
            }))
    }
}

#[derive(Debug, Deserialize)]
struct MangaResponse {
    data: MangaDexManga,
}

#[derive(Debug, Deserialize)]
struct MangaListResponse {
    data: Vec<MangaDexManga>,
}

#[derive(Debug, Deserialize)]
struct CoverListResponse {
    data: Vec<MangaDexCover>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MangaDexCover {
    pub id: Uuid,
    pub attributes: MangaDexCoverAttributes,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MangaDexCoverAttributes {
    pub file_name: String,
    pub volume: Option<String>,
    pub locale: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub version: Option<i32>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MangaDexManga {
    pub id: Uuid,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub attributes: MangaDexAttributes,
    #[serde(default)]
    pub relationships: Vec<MangaDexRelationship>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MangaDexAttributes {
    #[serde(default)]
    pub title: HashMap<String, Option<String>>,
    #[serde(default)]
    pub alt_titles: Vec<HashMap<String, Option<String>>>,
    #[serde(default)]
    pub description: HashMap<String, Option<String>>,
    pub original_language: Option<String>,
    pub publication_demographic: Option<String>,
    pub status: Option<String>,
    pub year: Option<i32>,
    pub content_rating: Option<String>,
    pub last_volume: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
    pub version: Option<i32>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MangaDexRelationship {
    pub id: Option<Uuid>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub attributes: Option<MangaDexRelationshipAttributes>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MangaDexRelationshipAttributes {
    pub name: Option<String>,
    pub image_url: Option<String>,
    pub file_name: Option<String>,
    pub locale: Option<String>,
    pub volume: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
    pub version: Option<i32>,
}

impl MangaDexManga {
    pub fn cover_art(&self) -> Option<&MangaDexRelationship> {
        self.relationships
            .iter()
            .find(|relationship| relationship.kind.as_deref() == Some("cover_art"))
    }
}

pub fn cover_url(manga_id: Uuid, file_name: &str) -> String {
    format!("https://uploads.mangadex.org/covers/{manga_id}/{file_name}.512.jpg")
}

fn manga_search_query(title: Option<&str>, offset: u32, limit: u32) -> Vec<(&str, String)> {
    let mut query = vec![
        ("limit", limit.to_string()),
        ("offset", offset.to_string()),
        ("includes[]", "cover_art".to_string()),
        ("includes[]", "author".to_string()),
        ("includes[]", "artist".to_string()),
    ];
    if let Some(title) = title.filter(|title| !title.is_empty()) {
        query.push(("title", title.to_string()));
    } else {
        query.push(("order[followedCount]", "desc".to_string()));
    }
    query
}

fn latest_volume_query(manga_id: Uuid) -> Vec<(&'static str, String)> {
    vec![
        ("limit", "100".to_string()),
        ("manga[]", manga_id.to_string()),
        ("order[volume]", "desc".to_string()),
    ]
}

fn matches_language(locale: Option<&str>, language: &str) -> bool {
    locale
        .map(|locale| locale.replace('_', "-").to_ascii_lowercase())
        .and_then(|locale| locale.split('-').next().map(str::to_string))
        .is_some_and(|locale| locale == language)
}

fn is_regular_volume(volume: &str) -> bool {
    volume
        .parse::<f64>()
        .is_ok_and(|number| number.is_finite() && number.fract() == 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_search_sends_title_and_pagination() {
        let query = manga_search_query(Some("Frieren"), 12, 6);

        assert!(query.contains(&("title", "Frieren".to_string())));
        assert!(query.contains(&("offset", "12".to_string())));
        assert!(query.contains(&("limit", "6".to_string())));
        assert!(!query.iter().any(|(key, _)| *key == "order[followedCount]"));
    }

    #[tokio::test]
    async fn request_limiter_spaces_requests_across_clones() {
        let limiter = MangaDexRequestLimiter::new(2, Duration::from_millis(40));
        let _first = limiter.acquire().await;

        let started_at = Instant::now();
        let _second = limiter.clone().acquire().await;

        assert!(started_at.elapsed() >= Duration::from_millis(35));
    }

    #[tokio::test]
    async fn request_limiter_caps_concurrent_requests() {
        let limiter = MangaDexRequestLimiter::new(1, Duration::ZERO);
        let first = limiter.acquire().await;
        let cloned = limiter.clone();
        let mut second = tokio::spawn(async move { cloned.acquire().await });

        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut second)
                .await
                .is_err()
        );
        drop(first);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), second)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn circuit_breaker_opens_after_consecutive_failures() {
        let breaker = MangaDexCircuitBreaker::new(3, Duration::from_millis(20));

        for _ in 0..3 {
            assert!(breaker.allows_request().await);
            breaker.record_result(false).await;
        }
        assert!(!breaker.allows_request().await);

        tokio::time::sleep(Duration::from_millis(25)).await;
        assert!(breaker.allows_request().await);
    }

    #[tokio::test]
    async fn circuit_breaker_resets_after_a_success() {
        let breaker = MangaDexCircuitBreaker::new(3, Duration::from_secs(1));

        breaker.record_result(false).await;
        breaker.record_result(false).await;
        breaker.record_result(true).await;
        breaker.record_result(false).await;
        breaker.record_result(false).await;

        assert!(breaker.allows_request().await);
    }

    #[test]
    fn empty_search_orders_by_followed_count() {
        let query = manga_search_query(None, 0, 10);

        assert!(query.contains(&("order[followedCount]", "desc".to_string())));
        assert!(!query.iter().any(|(key, _)| *key == "title"));
    }

    #[test]
    fn latest_volume_query_requests_highest_volume_without_locale_filtering() {
        let manga_id = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
        let query = latest_volume_query(manga_id);

        assert!(query.contains(&("limit", "100".to_string())));
        assert!(query.contains(&("manga[]", manga_id.to_string())));
        assert!(!query.iter().any(|(key, _)| *key == "locales[]"));
        assert!(query.contains(&("order[volume]", "desc".to_string())));
    }
}
