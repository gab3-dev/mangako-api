use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    extract::State,
    http::{
        HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode, header::CACHE_CONTROL,
    },
    middleware::Next,
    response::{IntoResponse, Response},
};
use tower_http::{
    LatencyUnit,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::{DefaultOnFailure, DefaultOnResponse, TraceLayer},
};
use tracing::Level;

const X_CACHE: HeaderName = HeaderName::from_static("x-cache");

#[derive(Clone, Default)]
pub struct OperationalConfig {
    pub cache: Option<CacheConfig>,
}

#[derive(Clone)]
pub struct CacheConfig {
    pub ttl: Duration,
    pub max_entries: usize,
    pub max_bytes: usize,
}

#[derive(Clone, Copy)]
pub struct CacheTtl(pub Duration);

pub fn with_observability(router: Router) -> Router {
    router
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &Request<_>| {
                    let request_id = request
                        .headers()
                        .get("x-request-id")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("unknown");
                    tracing::info_span!(
                        "http_request",
                        request_id,
                        method = %request.method(),
                        uri = %request.uri(),
                    )
                })
                .on_response(
                    DefaultOnResponse::new()
                        .level(Level::INFO)
                        .latency_unit(LatencyUnit::Millis),
                )
                .on_failure(DefaultOnFailure::new().level(Level::ERROR)),
        )
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
}

#[derive(Clone)]
pub struct ResponseCache {
    config: CacheConfig,
    state: Arc<Mutex<CacheState>>,
}

struct CacheState {
    entries: HashMap<String, CachedResponse>,
    total_bytes: usize,
}

#[derive(Clone)]
struct CachedResponse {
    inserted_at: Instant,
    ttl: Duration,
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
}

impl ResponseCache {
    pub fn new(config: CacheConfig) -> Self {
        Self {
            config,
            state: Arc::new(Mutex::new(CacheState {
                entries: HashMap::new(),
                total_bytes: 0,
            })),
        }
    }

    fn get(&self, key: &str) -> Option<CachedResponse> {
        let mut state = self.state.lock().expect("response cache lock poisoned");
        let entry = state.entries.get(key)?;
        if entry.inserted_at.elapsed() >= entry.ttl {
            if let Some(expired) = state.entries.remove(key) {
                state.total_bytes = state.total_bytes.saturating_sub(expired.body.len());
            }
            return None;
        }
        Some(entry.clone())
    }

    fn insert(&self, key: String, response: CachedResponse) -> bool {
        let response_bytes = response.body.len();
        if response_bytes > self.config.max_bytes {
            return false;
        }

        let mut state = self.state.lock().expect("response cache lock poisoned");
        if let Some(replaced) = state.entries.remove(&key) {
            state.total_bytes = state.total_bytes.saturating_sub(replaced.body.len());
        }

        while state.entries.len() >= self.config.max_entries
            || state.total_bytes.saturating_add(response_bytes) > self.config.max_bytes
        {
            let Some(oldest) = state
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.inserted_at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some(evicted) = state.entries.remove(&oldest) {
                state.total_bytes = state.total_bytes.saturating_sub(evicted.body.len());
            }
        }

        state.total_bytes = state.total_bytes.saturating_add(response_bytes);
        state.entries.insert(key, response);
        true
    }

    fn invalidate_path(&self, path: &str) {
        let mut state = self.state.lock().expect("response cache lock poisoned");
        let keys = state
            .entries
            .keys()
            .filter(|key| key.split('?').next() == Some(path))
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            if let Some(removed) = state.entries.remove(&key) {
                state.total_bytes = state.total_bytes.saturating_sub(removed.body.len());
            }
        }
    }
}

pub async fn cache_get_response(
    State(cache): State<ResponseCache>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if request.method() != Method::GET {
        return next.run(request).await;
    }
    if requests_refresh(request.uri().query()) {
        cache.invalidate_path(request.uri().path());
        return next.run(request).await;
    }

    let key = request.uri().to_string();
    if let Some(cached) = cache.get(&key) {
        let mut response = Response::new(Body::from(cached.body));
        *response.status_mut() = cached.status;
        *response.headers_mut() = cached.headers;
        response
            .headers_mut()
            .insert(X_CACHE, HeaderValue::from_static("HIT"));
        return response;
    }

    let response = next.run(request).await;
    if response.status() != StatusCode::OK {
        return response;
    }

    let (mut parts, body) = response.into_parts();
    let ttl = parts
        .extensions
        .remove::<CacheTtl>()
        .map(|cache_ttl| cache_ttl.0)
        .unwrap_or(cache.config.ttl);
    let bytes = match to_bytes(body, usize::MAX).await {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::error!(%error, "failed to buffer response for cache");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    parts.headers.insert(
        CACHE_CONTROL,
        HeaderValue::from_str(&format!("private, max-age={}", ttl.as_secs()))
            .expect("valid cache-control header"),
    );
    let cached = CachedResponse {
        inserted_at: Instant::now(),
        ttl,
        status: parts.status,
        headers: parts.headers.clone(),
        body: bytes.clone(),
    };
    let cached = cache.insert(key, cached);
    parts.headers.insert(
        X_CACHE,
        if cached {
            HeaderValue::from_static("MISS")
        } else {
            HeaderValue::from_static("BYPASS")
        },
    );
    Response::from_parts(parts, Body::from(bytes))
}

fn requests_refresh(query: Option<&str>) -> bool {
    query.is_some_and(|query| {
        query.split('&').any(|parameter| {
            let mut pair = parameter.splitn(2, '=');
            pair.next() == Some("refresh") && pair.next() == Some("true")
        })
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use axum::{Router, middleware, routing::get};
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn successful_get_responses_are_cached() {
        let calls = Arc::new(AtomicUsize::new(0));
        let handler_calls = calls.clone();
        let cache = ResponseCache::new(CacheConfig {
            ttl: Duration::from_secs(60),
            max_entries: 10,
            max_bytes: 1_024,
        });
        let app = Router::new()
            .route(
                "/value",
                get(move || {
                    let calls = handler_calls.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        "cached value"
                    }
                }),
            )
            .layer(middleware::from_fn_with_state(cache, cache_get_response));

        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/value")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.headers().get(X_CACHE).unwrap(), "MISS");

        let second = app
            .oneshot(
                Request::builder()
                    .uri("/value")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(second.headers().get(X_CACHE).unwrap(), "HIT");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn expired_cache_entries_are_refreshed() {
        let calls = Arc::new(AtomicUsize::new(0));
        let handler_calls = calls.clone();
        let cache = ResponseCache::new(CacheConfig {
            ttl: Duration::from_millis(10),
            max_entries: 10,
            max_bytes: 1_024,
        });
        let app = Router::new()
            .route(
                "/value",
                get(move || {
                    let calls = handler_calls.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        "cached value"
                    }
                }),
            )
            .layer(middleware::from_fn_with_state(cache, cache_get_response));

        let request = || {
            Request::builder()
                .uri("/value")
                .body(Body::empty())
                .unwrap()
        };
        app.clone().oneshot(request()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        let response = app.oneshot(request()).await.unwrap();

        assert_eq!(response.headers().get(X_CACHE).unwrap(), "MISS");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn response_specific_ttl_overrides_default_cache_ttl() {
        let calls = Arc::new(AtomicUsize::new(0));
        let handler_calls = calls.clone();
        let cache = ResponseCache::new(CacheConfig {
            ttl: Duration::from_secs(60),
            max_entries: 10,
            max_bytes: 1_024,
        });
        let app = Router::new()
            .route(
                "/popular",
                get(move || {
                    let calls = handler_calls.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        let mut response = "popular".into_response();
                        response
                            .extensions_mut()
                            .insert(CacheTtl(Duration::from_millis(10)));
                        response
                    }
                }),
            )
            .layer(middleware::from_fn_with_state(cache, cache_get_response));

        let request = || {
            Request::builder()
                .uri("/popular")
                .body(Body::empty())
                .unwrap()
        };
        app.clone().oneshot(request()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        app.oneshot(request()).await.unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn refresh_requests_bypass_response_cache() {
        let calls = Arc::new(AtomicUsize::new(0));
        let handler_calls = calls.clone();
        let cache = ResponseCache::new(CacheConfig {
            ttl: Duration::from_secs(60),
            max_entries: 10,
            max_bytes: 1_024,
        });
        let app = Router::new()
            .route(
                "/value",
                get(move || {
                    let calls = handler_calls.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        "value"
                    }
                }),
            )
            .layer(middleware::from_fn_with_state(cache, cache_get_response));

        let request = |uri| Request::builder().uri(uri).body(Body::empty()).unwrap();
        app.clone().oneshot(request("/value")).await.unwrap();
        app.clone()
            .oneshot(request("/value?refresh=true"))
            .await
            .unwrap();
        let after_refresh = app.oneshot(request("/value")).await.unwrap();

        assert_eq!(after_refresh.headers().get(X_CACHE).unwrap(), "MISS");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn responses_larger_than_cache_budget_are_bypassed() {
        let calls = Arc::new(AtomicUsize::new(0));
        let handler_calls = calls.clone();
        let cache = ResponseCache::new(CacheConfig {
            ttl: Duration::from_secs(60),
            max_entries: 10,
            max_bytes: 4,
        });
        let app = Router::new()
            .route(
                "/large",
                get(move || {
                    let calls = handler_calls.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        "larger"
                    }
                }),
            )
            .layer(middleware::from_fn_with_state(cache, cache_get_response));

        for _ in 0..2 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/large")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.headers().get(X_CACHE).unwrap(), "BYPASS");
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn oldest_response_is_evicted_when_byte_budget_is_exceeded() {
        let first_calls = Arc::new(AtomicUsize::new(0));
        let first_handler_calls = first_calls.clone();
        let cache = ResponseCache::new(CacheConfig {
            ttl: Duration::from_secs(60),
            max_entries: 10,
            max_bytes: 8,
        });
        let app = Router::new()
            .route(
                "/first",
                get(move || {
                    let calls = first_handler_calls.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        "first"
                    }
                }),
            )
            .route("/second", get(|| async { "second" }))
            .layer(middleware::from_fn_with_state(cache, cache_get_response));

        let request = |uri| Request::builder().uri(uri).body(Body::empty()).unwrap();
        app.clone().oneshot(request("/first")).await.unwrap();
        app.clone().oneshot(request("/second")).await.unwrap();
        let reloaded = app.oneshot(request("/first")).await.unwrap();

        assert_eq!(reloaded.headers().get(X_CACHE).unwrap(), "MISS");
        assert_eq!(first_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn observability_adds_and_propagates_request_id() {
        let app = with_observability(Router::new().route("/", get(|| async { "ok" })));

        let generated = app
            .clone()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(generated.headers().contains_key("x-request-id"));

        let propagated = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("x-request-id", "client-request-id")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            propagated.headers().get("x-request-id").unwrap(),
            "client-request-id"
        );
    }
}
