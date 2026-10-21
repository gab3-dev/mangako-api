use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use axum::{
    Json, Router,
    extract::Query,
    http::{HeaderMap, StatusCode, header},
    routing::get,
};
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

use mangako_api::{auth::AuthConfig, cover_storage::CoverStorage, operations::OperationalConfig};

#[derive(Clone, Default)]
struct CapturedRequest(Arc<Mutex<Option<(String, String)>>>);

async fn upstream_search(
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
    captured: axum::extract::State<CapturedRequest>,
) -> Json<Value> {
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    *captured.0.0.lock().unwrap() =
        Some((authorization, query.get("q").cloned().unwrap_or_default()));
    Json(json!({
        "results": [
            {
                "id": "MLB-CATALOG-123",
                "name": "Dragon Ball Vol. 2.5",
                "pictures": [{ "url": "https://http2.mlstatic.com/cover.jpg" }]
            }
        ]
    }))
}

async fn upstream_product_items() -> Json<Value> {
    Json(json!({
        "results": [
            {
                "item_id": "MLB123",
                "price": 45.0,
                "original_price": 60.0,
                "currency_id": "BRL",
                "condition": "new"
            },
            {
                "item_id": "MLB456",
                "price": 30.0,
                "original_price": null,
                "currency_id": "BRL",
                "condition": "used"
            }
        ]
    }))
}

fn test_pool() -> sqlx::PgPool {
    PgPoolOptions::new()
        .connect_lazy("postgres://mangako:mangako@localhost:5432/mangako_api")
        .expect("valid test database URL")
}

fn cover_storage() -> CoverStorage {
    CoverStorage::new(
        PathBuf::from("/tmp/mangako-test-covers"),
        "http://localhost:3000/media".to_string(),
        1024,
    )
}

fn auth() -> AuthConfig {
    AuthConfig {
        read_token: "test-read-token".to_string(),
        write_token: "test-write-token".to_string(),
    }
}

async fn start_upstream(captured: CapturedRequest) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let upstream = Router::new()
        .route("/products/search", get(upstream_search))
        .route(
            "/products/MLB-CATALOG-123/items",
            get(upstream_product_items),
        )
        .with_state(captured);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });
    (address, task)
}

#[tokio::test]
async fn volume_search_authenticates_to_mercadolivre_and_returns_valid_offers() {
    let captured = CapturedRequest::default();
    let (upstream_address, upstream_task) = start_upstream(captured.clone()).await;
    let api = mangako_api::api::router_with_mercadolivre_base_url(
        test_pool(),
        auth(),
        OperationalConfig::default(),
        cover_storage(),
        Some("mercadolivre-test-access-token".to_string()),
        &format!("http://{upstream_address}/"),
    );

    let response = api
        .oneshot(
            axum::http::Request::builder()
                .uri("/shopping/mercado-livre/offers?title=Dragon%20Ball&volume=2.5&locale=pt-br")
                .header(header::AUTHORIZATION, "Bearer test-read-token")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let offers: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(offers["query"], "Dragon Ball mangá volume 2.5");
    assert_eq!(offers["volumeSpecific"], true);
    assert_eq!(offers["offers"].as_array().unwrap().len(), 2);
    assert_eq!(offers["offers"][0]["id"], "MLB456");
    assert_eq!(offers["offers"][0]["price"], 30.0);
    assert_eq!(offers["offers"][1]["id"], "MLB123");
    assert_eq!(offers["offers"][1]["title"], "Dragon Ball Vol. 2.5");
    assert_eq!(offers["offers"][1]["price"], 45.0);
    assert_eq!(offers["offers"][1]["originalPrice"], 60.0);
    assert_eq!(offers["offers"][1]["discountPercent"], 25);
    assert_eq!(
        offers["offers"][1]["permalink"],
        "https://www.mercadolivre.com.br/p/MLB-CATALOG-123"
    );
    assert!(offers["fetchedAt"].as_str().is_some());
    assert_eq!(
        captured.0.lock().unwrap().as_ref().unwrap(),
        &(
            "Bearer mercadolivre-test-access-token".to_string(),
            "Dragon Ball mangá volume 2.5".to_string(),
        )
    );

    upstream_task.abort();
}

#[tokio::test]
async fn volume_search_requires_read_auth_and_marketplace_credentials() {
    let api = mangako_api::api::router_with_mercadolivre(
        test_pool(),
        auth(),
        OperationalConfig::default(),
        cover_storage(),
        None,
    );

    let unauthorized = api
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/shopping/mercado-livre/offers?title=One%20Piece&volume=1")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let unavailable = api
        .oneshot(
            axum::http::Request::builder()
                .uri("/shopping/mercado-livre/offers?title=One%20Piece&volume=1")
                .header(header::AUTHORIZATION, "Bearer test-read-token")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn marketplace_without_credentials_keeps_existing_routes_and_auth_working() {
    let app = mangako_api::api::router_with_mercadolivre(
        test_pool(),
        auth(),
        OperationalConfig::default(),
        cover_storage(),
        None,
    );
    for (uri, token, expected) in [
        ("/health", None, StatusCode::OK),
        ("/api-docs/openapi.json", None, StatusCode::OK),
        ("/mangas?limit=0", None, StatusCode::UNAUTHORIZED),
        (
            "/mangas?limit=0",
            Some("test-read-token"),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/mangas/?limit=0",
            Some("test-read-token"),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/mangas?limit=0",
            Some("test-write-token"),
            StatusCode::UNAUTHORIZED,
        ),
        (
            "/mangas?limit=0&refresh=true",
            Some("test-write-token"),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let mut request = axum::http::Request::builder().uri(uri);
        if let Some(token) = token {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let response = app
            .clone()
            .oneshot(request.body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{uri}");
    }
}
