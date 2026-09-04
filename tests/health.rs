use axum::{body::Body, http::Request};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

#[tokio::test]
async fn health_returns_ok() {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://mangako:mangako@localhost:5432/mangako_api")
        .expect("valid database URL");
    let app = mangako_api::api::router(pool);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::OK);
}

#[tokio::test]
async fn manga_search_routes_match_with_and_without_trailing_slash() {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://mangako:mangako@localhost:5432/mangako_api")
        .expect("valid database URL");
    let app = mangako_api::api::router(pool);

    for uri in ["/mangas?limit=0", "/mangas/?limit=0"] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn openapi_and_swagger_ui_are_served() {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://mangako:mangako@localhost:5432/mangako_api")
        .expect("valid database URL");
    let app = mangako_api::api::router(pool);

    let openapi_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api-docs/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(openapi_response.status(), axum::http::StatusCode::OK);
    let openapi_body = axum::body::to_bytes(openapi_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let openapi: serde_json::Value = serde_json::from_slice(&openapi_body).unwrap();
    let search_parameters = openapi["paths"]["/mangas"]["get"]["parameters"]
        .as_array()
        .unwrap();
    for parameter in ["title", "limit", "offset", "locale"] {
        assert!(
            search_parameters
                .iter()
                .any(|value| value["name"] == parameter)
        );
    }
    assert!(
        openapi["components"]["schemas"]["MangaResponse"]["properties"]
            .get("latestVolumeNumber")
            .is_some()
    );
    assert!(openapi["paths"].get("/stats/mangadex-fallback").is_some());

    let docs_response = app
        .oneshot(Request::builder().uri("/docs").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert!(docs_response.status().is_success() || docs_response.status().is_redirection());
}

#[tokio::test]
async fn manga_routes_require_api_token_when_configured() {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://mangako:mangako@localhost:5432/mangako_api")
        .expect("valid database URL");
    let app = mangako_api::api::router_with_api_token(pool, "secret-token".to_string());

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/mangas?title=frieren")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/mangas?title=frieren")
                .header(axum::http::header::AUTHORIZATION, "Bearer wrong-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/mangas?limit=0")
                .header(axum::http::header::AUTHORIZATION, "Bearer secret-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn health_and_docs_stay_public_when_api_token_is_configured() {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://mangako:mangako@localhost:5432/mangako_api")
        .expect("valid database URL");
    let app = mangako_api::api::router_with_api_token(pool, "secret-token".to_string());

    let health_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health_response.status(), axum::http::StatusCode::OK);

    let openapi_response = app
        .oneshot(
            Request::builder()
                .uri("/api-docs/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(openapi_response.status(), axum::http::StatusCode::OK);
}

#[tokio::test]
async fn mangadex_fallback_stats_require_an_api_token() {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://mangako:mangako@localhost:5432/mangako_api")
        .expect("valid database URL");
    let app = mangako_api::api::router_with_api_token(pool, "secret-token".to_string());

    let unauthorized = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/stats/mangadex-fallback")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), axum::http::StatusCode::UNAUTHORIZED);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/stats/mangadex-fallback")
                .header(axum::http::header::AUTHORIZATION, "Bearer secret-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let stats: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(stats["catalogRequests"], 0);
    assert_eq!(stats["fallbackRequests"], 0);
    assert_eq!(stats["fallbackRate"], 0.0);
}
