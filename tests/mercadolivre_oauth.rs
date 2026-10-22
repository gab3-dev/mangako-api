use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    Form, Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use mangako_api::{
    mercadolivre::MercadoLivreClient,
    mercadolivre_oauth::{MercadoLivreOAuth, MercadoLivreOAuthConfig},
};
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;
use uuid::Uuid;

fn app(pool: sqlx::PgPool, client: MercadoLivreClient) -> Router {
    mangako_api::api::router_with_marketplace_client(
        pool,
        mangako_api::auth::AuthConfig {
            read_token: "read-test".to_string(),
            write_token: "write-test".to_string(),
        },
        mangako_api::operations::OperationalConfig::default(),
        mangako_api::cover_storage::CoverStorage::new(
            std::path::PathBuf::from("/tmp/mangako-oauth-covers"),
            "http://localhost/media".to_string(),
            1024,
        ),
        None,
        client,
    )
}

#[derive(Clone, Default)]
struct Upstream {
    refreshes: Arc<AtomicUsize>,
    gets: Arc<AtomicUsize>,
    mode: Arc<AtomicU8>,
    used_refresh_tokens: Arc<Mutex<Vec<String>>>,
}

async fn exchange(
    State(state): State<Upstream>,
    Form(form): Form<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    assert_eq!(form.get("grant_type").unwrap(), "refresh_token");
    assert_eq!(form.get("client_secret").unwrap(), "fixture-client-secret");
    state
        .used_refresh_tokens
        .lock()
        .unwrap()
        .push(form["refresh_token"].clone());
    let number = state.refreshes.fetch_add(1, Ordering::SeqCst) + 1;
    tokio::time::sleep(Duration::from_millis(30)).await;
    match state.mode.load(Ordering::SeqCst) {
        1 => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"invalid_grant"})),
        ),
        2 => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"temporarily_unavailable"})),
        ),
        3 => (
            StatusCode::OK,
            Json(json!({"access_token":"incomplete-response","expires_in":21600})),
        ),
        _ => (
            StatusCode::OK,
            Json(json!({
                "access_token":format!("access-{number}"),
                "refresh_token":format!("refresh-{number}"),
                "expires_in":21600,
            })),
        ),
    }
}

async fn search(State(state): State<Upstream>, headers: HeaderMap) -> (StatusCode, Json<Value>) {
    state.gets.fetch_add(1, Ordering::SeqCst);
    let token = headers.get("authorization").unwrap().to_str().unwrap();
    let mode = state.mode.load(Ordering::SeqCst);
    if mode == 5 || (mode == 4 && token == "Bearer seed-access") {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"unauthorized"})),
        );
    }
    (
        StatusCode::OK,
        Json(json!({"results":[{
            "id":"MLB123", "name":"Frieren Volume 15", "domain_id":"MLB-BOOKS", "pictures":[]
        }]})),
    )
}

async fn items(State(state): State<Upstream>, headers: HeaderMap) -> Json<Value> {
    state.gets.fetch_add(1, Ordering::SeqCst);
    assert!(
        headers
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("Bearer access-")
    );
    Json(
        json!({"results":[{"item_id":"MLB456","price":42.99,"currency_id":"BRL","original_price":null,"condition":"new"}]}),
    )
}

struct Fixture {
    pool: sqlx::PgPool,
    upstream: Upstream,
    client_id: String,
    base_url: String,
    task: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn new() -> Self {
        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgres://mangako:mangako@localhost:5432/mangako_api".to_string()
        });
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(&database_url)
            .await
            .unwrap();
        mangako_api::db::migrate(&pool).await.unwrap();
        let upstream = Upstream::default();
        let app = Router::new()
            .route("/oauth/token", post(exchange))
            .route("/products/search", get(search))
            .route("/products/MLB123/items", get(items))
            .with_state(upstream.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            pool,
            upstream,
            client_id: format!("oauth-test-{}", Uuid::new_v4()),
            base_url: format!("http://{address}/"),
            task,
        }
    }

    fn manager(&self, expires_at: DateTime<Utc>) -> MercadoLivreOAuth {
        MercadoLivreOAuth::with_base_url(
            self.pool.clone(),
            reqwest::Client::new(),
            MercadoLivreOAuthConfig {
                client_id: self.client_id.clone(),
                client_secret: "fixture-client-secret".to_string(),
                initial_access_token: Some("seed-access".to_string()),
                initial_refresh_token: Some("seed-refresh".to_string()),
                initial_expires_at: Some(expires_at),
            },
            &self.base_url,
        )
    }

    async fn tokens(&self) -> (String, String, DateTime<Utc>, bool) {
        sqlx::query_as("SELECT access_token, refresh_token, expires_at, reauthorization_required FROM mercadolivre_oauth_tokens WHERE client_id = $1")
            .bind(&self.client_id).fetch_one(&self.pool).await.unwrap()
    }

    async fn finish(self) {
        sqlx::query("DELETE FROM mercadolivre_oauth_tokens WHERE client_id = $1")
            .bind(&self.client_id)
            .execute(&self.pool)
            .await
            .unwrap();
        self.task.abort();
    }
}

#[tokio::test]
async fn valid_token_is_reused_without_refresh() {
    let fixture = Fixture::new().await;
    let manager = fixture.manager(Utc::now() + chrono::Duration::hours(6));
    manager.initialize().await.unwrap();
    assert_eq!(manager.access_token().await.unwrap(), "seed-access");
    assert_eq!(fixture.upstream.refreshes.load(Ordering::SeqCst), 0);
    fixture.finish().await;
}

#[tokio::test]
async fn expiry_refreshes_once_across_concurrent_instances_and_persists_rotation() {
    let fixture = Fixture::new().await;
    let manager = fixture.manager(Utc::now() - chrono::Duration::seconds(1));
    manager.initialize().await.unwrap();
    let second = fixture.manager(Utc::now() - chrono::Duration::hours(1));
    second.initialize().await.unwrap();
    let mut calls = Vec::new();
    for index in 0..16 {
        let manager = if index % 2 == 0 {
            manager.clone()
        } else {
            second.clone()
        };
        calls.push(tokio::spawn(async move {
            manager.access_token().await.unwrap()
        }));
    }
    for call in calls {
        assert_eq!(call.await.unwrap(), "access-1");
    }
    assert_eq!(fixture.upstream.refreshes.load(Ordering::SeqCst), 1);
    let (access, refresh, expiry, _) = fixture.tokens().await;
    assert_eq!(access, "access-1");
    assert_eq!(refresh, "refresh-1");
    assert!(expiry > Utc::now() + chrono::Duration::hours(5));

    let restarted = fixture.manager(Utc::now() - chrono::Duration::hours(2));
    restarted.initialize().await.unwrap();
    assert_eq!(restarted.access_token().await.unwrap(), "access-1");
    assert_eq!(fixture.tokens().await.1, "refresh-1");
    sqlx::query("UPDATE mercadolivre_oauth_tokens SET expires_at = now() WHERE client_id = $1")
        .bind(&fixture.client_id)
        .execute(&fixture.pool)
        .await
        .unwrap();
    assert_eq!(restarted.access_token().await.unwrap(), "access-2");
    assert_eq!(
        *fixture.upstream.used_refresh_tokens.lock().unwrap(),
        vec!["seed-refresh", "refresh-1"]
    );
    fixture.finish().await;
}

#[tokio::test]
async fn refresh_loop_renews_before_expiry_without_a_shopping_request() {
    let fixture = Fixture::new().await;
    let manager = fixture.manager(Utc::now() + chrono::Duration::seconds(20));
    manager.initialize().await.unwrap();
    let task = tokio::spawn(manager.run_refresh_loop());
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if fixture.tokens().await.0 == "access-1" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(fixture.upstream.refreshes.load(Ordering::SeqCst), 1);
    task.abort();
    fixture.finish().await;
}

#[tokio::test]
async fn upstream_401_refreshes_and_retries_the_get_once() {
    let fixture = Fixture::new().await;
    fixture.upstream.mode.store(4, Ordering::SeqCst);
    let manager = fixture.manager(Utc::now() + chrono::Duration::hours(6));
    manager.initialize().await.unwrap();
    let client = MercadoLivreClient::with_base_url(reqwest::Client::new(), None, &fixture.base_url)
        .using_oauth(manager);
    let response = app(fixture.pool.clone(), client)
        .oneshot(
            axum::http::Request::builder()
                .uri("/shopping/mercado-livre/offers?title=Frieren&volume=15&locale=ja")
                .header("Authorization", "Bearer read-test")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let response: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(response["offers"].as_array().unwrap().len(), 1);
    assert_eq!(fixture.upstream.gets.load(Ordering::SeqCst), 3);
    assert_eq!(fixture.upstream.refreshes.load(Ordering::SeqCst), 1);
    fixture.finish().await;
}

#[tokio::test]
async fn repeated_401_is_bounded_and_cools_down_instead_of_refreshing_forever() {
    let fixture = Fixture::new().await;
    fixture.upstream.mode.store(5, Ordering::SeqCst);
    let manager = fixture.manager(Utc::now() + chrono::Duration::hours(6));
    manager.initialize().await.unwrap();
    let client = MercadoLivreClient::with_base_url(reqwest::Client::new(), None, &fixture.base_url)
        .using_oauth(manager);
    assert!(
        client
            .search_offers("Frieren", Some(15.0), None)
            .await
            .is_err()
    );
    assert!(
        client
            .search_offers("Frieren", Some(15.0), None)
            .await
            .is_err()
    );
    assert_eq!(fixture.upstream.gets.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.upstream.refreshes.load(Ordering::SeqCst), 1);
    fixture.finish().await;
}

#[tokio::test]
async fn invalid_grant_keeps_tokens_and_requires_reauthorization_without_repeating_exchange() {
    let fixture = Fixture::new().await;
    fixture.upstream.mode.store(1, Ordering::SeqCst);
    let manager = fixture.manager(Utc::now());
    manager.initialize().await.unwrap();
    assert!(manager.access_token().await.is_err());
    assert!(manager.access_token().await.is_err());
    let (access, refresh, _, permanent) = fixture.tokens().await;
    assert_eq!(access, "seed-access");
    assert_eq!(refresh, "seed-refresh");
    assert!(permanent);
    assert_eq!(fixture.upstream.refreshes.load(Ordering::SeqCst), 1);
    fixture.finish().await;
}

#[tokio::test]
async fn transient_failure_cools_down_then_recovers_without_losing_refresh_token() {
    let fixture = Fixture::new().await;
    fixture.upstream.mode.store(2, Ordering::SeqCst);
    let manager = fixture.manager(Utc::now());
    manager.initialize().await.unwrap();
    assert!(manager.access_token().await.is_err());
    assert!(manager.access_token().await.is_err());
    assert!(!fixture.tokens().await.3);
    fixture.upstream.mode.store(0, Ordering::SeqCst);
    sqlx::query("UPDATE mercadolivre_oauth_tokens SET retry_after = now() - interval '1 second' WHERE client_id = $1")
        .bind(&fixture.client_id).execute(&fixture.pool).await.unwrap();
    assert_eq!(manager.access_token().await.unwrap(), "access-2");
    assert_eq!(fixture.upstream.refreshes.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.tokens().await.1, "refresh-2");
    fixture.finish().await;
}

#[tokio::test]
async fn missing_rotated_refresh_token_is_not_treated_as_a_success() {
    let fixture = Fixture::new().await;
    fixture.upstream.mode.store(3, Ordering::SeqCst);
    let manager = fixture.manager(Utc::now());
    manager.initialize().await.unwrap();
    assert!(manager.access_token().await.is_err());
    assert!(fixture.tokens().await.3);
    assert_eq!(fixture.tokens().await.0, "seed-access");
    fixture.finish().await;
}

#[tokio::test]
async fn authorization_failure_returns_503_without_leaking_any_tokens() {
    let fixture = Fixture::new().await;
    fixture.upstream.mode.store(1, Ordering::SeqCst);
    let manager = fixture.manager(Utc::now());
    manager.initialize().await.unwrap();
    let client = MercadoLivreClient::with_base_url(reqwest::Client::new(), None, &fixture.base_url)
        .using_oauth(manager);
    let response = app(fixture.pool.clone(), client)
        .oneshot(
            axum::http::Request::builder()
                .uri("/shopping/mercado-livre/offers?title=Frieren&volume=15")
                .header("Authorization", "Bearer read-test")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    for secret in ["seed-access", "seed-refresh", "fixture-client-secret"] {
        assert!(!body.contains(secret));
    }
    let result: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(result["code"], "mercadolivre_authorization_unavailable");
    fixture.finish().await;
}
