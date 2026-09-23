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
        .clone()
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
    for path in [
        "/mangas",
        "/mangas/{manga_ref}/covers",
        "/mangas/{manga_ref}/volumes",
    ] {
        assert!(openapi["paths"][path]["post"].is_object(), "{path}");
    }
    assert!(openapi["paths"]["/mangas/{manga_ref}"]["patch"].is_object());
    assert!(openapi["paths"]["/mangas/{manga_ref}"]["delete"].is_object());
    assert!(openapi["paths"]["/mangas/{manga_ref}/volumes/{volume_id}"]["patch"].is_object());
    assert!(openapi["paths"]["/mangas/{manga_ref}/volumes/{volume_id}"]["delete"].is_object());
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
    let app = mangako_api::api::router_with_api_tokens(
        pool,
        "read-token-for-tests".to_string(),
        "write-token-for-tests".to_string(),
    );

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
        .clone()
        .oneshot(
            Request::builder()
                .uri("/mangas?limit=0")
                .header(
                    axum::http::header::AUTHORIZATION,
                    "Bearer read-token-for-tests",
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);

    let read_token_cannot_write = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mangas")
                .header(
                    axum::http::header::AUTHORIZATION,
                    "Bearer read-token-for-tests",
                )
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        read_token_cannot_write.status(),
        axum::http::StatusCode::UNAUTHORIZED
    );

    let write_token_cannot_read = app
        .oneshot(
            Request::builder()
                .uri("/mangas?limit=0")
                .header(
                    axum::http::header::AUTHORIZATION,
                    "Bearer write-token-for-tests",
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        write_token_cannot_read.status(),
        axum::http::StatusCode::UNAUTHORIZED
    );

    let read_token_cannot_refresh = mangako_api::api::router_with_api_tokens(
        PgPoolOptions::new()
            .connect_lazy("postgres://mangako:mangako@localhost:5432/mangako_api")
            .expect("valid database URL"),
        "read-token-for-tests".to_string(),
        "write-token-for-tests".to_string(),
    )
    .oneshot(
        Request::builder()
            .uri("/mangas/example?refresh=true")
            .header(
                axum::http::header::AUTHORIZATION,
                "Bearer read-token-for-tests",
            )
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        read_token_cannot_refresh.status(),
        axum::http::StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn write_payloads_are_size_limited_before_database_access() {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://mangako:mangako@localhost:5432/mangako_api")
        .expect("valid database URL");
    let app = mangako_api::api::router_with_api_tokens(
        pool,
        "read-token-for-tests".to_string(),
        "write-token-for-tests".to_string(),
    );
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mangas")
                .header(
                    axum::http::header::AUTHORIZATION,
                    "Bearer write-token-for-tests",
                )
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(vec![b'x'; 64 * 1024 + 1]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn health_and_docs_stay_public_when_api_token_is_configured() {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://mangako:mangako@localhost:5432/mangako_api")
        .expect("valid database URL");
    let app = mangako_api::api::router_with_api_tokens(
        pool,
        "read-token-for-tests".to_string(),
        "write-token-for-tests".to_string(),
    );

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
    let app = mangako_api::api::router_with_api_tokens(
        pool,
        "read-token-for-tests".to_string(),
        "write-token-for-tests".to_string(),
    );

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
                .header(
                    axum::http::header::AUTHORIZATION,
                    "Bearer read-token-for-tests",
                )
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
