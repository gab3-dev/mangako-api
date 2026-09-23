use axum::{
    Router,
    extract::DefaultBodyLimit,
    middleware,
    routing::{get, patch, post},
};
use sqlx::PgPool;
use std::time::Duration;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::{
    auth::{self, AuthConfig},
    manga,
    manga_service::MangaService,
    mangadex::MangaDexClient,
    openapi::ApiDoc,
    operations::{FallbackMetrics, OperationalConfig, RateLimiter, ResponseCache},
};

#[derive(Clone)]
pub struct AppState {
    pub manga_service: MangaService,
    pub fallback_metrics: FallbackMetrics,
}

pub fn router(pool: PgPool) -> Router {
    build_router(pool, None, OperationalConfig::default())
}

pub fn router_with_api_tokens(pool: PgPool, read_token: String, write_token: String) -> Router {
    build_router(
        pool,
        Some(AuthConfig {
            read_token,
            write_token,
        }),
        OperationalConfig::default(),
    )
}

pub fn router_with_operations(
    pool: PgPool,
    auth: AuthConfig,
    operations: OperationalConfig,
) -> Router {
    build_router(pool, Some(auth), operations)
}

fn build_router(pool: PgPool, auth: Option<AuthConfig>, operations: OperationalConfig) -> Router {
    let http = reqwest::Client::builder()
        .user_agent(concat!("mangako-api/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .build()
        .expect("valid reqwest client");
    let mangadex = MangaDexClient::new(http);
    let manga_service = MangaService::new(pool, mangadex);
    let fallback_metrics = FallbackMetrics::new();

    let mut read_catalog = Router::new()
        .route("/mangas", get(manga::search_mangas))
        .route("/mangas/", get(manga::search_mangas))
        .route("/mangas/{manga_ref}", get(manga::get_manga))
        .route("/mangas/{manga_ref}/volumes", get(manga::get_manga_volumes));
    let mut stats = Router::new().route(
        "/stats/mangadex-fallback",
        get(manga::mangadex_fallback_stats),
    );
    let mut write_catalog = Router::new()
        .route("/mangas", post(manga::create_manga))
        .route(
            "/mangas/{manga_ref}",
            patch(manga::update_manga).delete(manga::delete_manga),
        )
        .route(
            "/mangas/{manga_ref}/covers",
            post(manga::create_manga_cover),
        )
        .route(
            "/mangas/{manga_ref}/volumes",
            post(manga::create_manga_volume),
        )
        .route(
            "/mangas/{manga_ref}/volumes/{volume_id}",
            patch(manga::update_manga_volume).delete(manga::delete_manga_volume),
        )
        .layer(DefaultBodyLimit::max(operations.max_json_body_bytes));

    if let Some(cache) = operations.cache {
        let cache = ResponseCache::new(cache);
        read_catalog = read_catalog.layer(middleware::from_fn_with_state(
            cache.clone(),
            crate::operations::cache_get_response,
        ));
        write_catalog = write_catalog.layer(middleware::from_fn_with_state(
            cache,
            crate::operations::cache_get_response,
        ));
    }

    read_catalog = read_catalog.layer(middleware::from_fn_with_state(
        fallback_metrics.clone(),
        crate::operations::track_catalog_request,
    ));
    let limiter = RateLimiter::new(operations.rate_limit);
    read_catalog = read_catalog.layer(middleware::from_fn_with_state(
        limiter.clone(),
        crate::operations::rate_limit_request,
    ));
    stats = stats.layer(middleware::from_fn_with_state(
        limiter.clone(),
        crate::operations::rate_limit_request,
    ));
    write_catalog = write_catalog.layer(middleware::from_fn_with_state(
        limiter,
        crate::operations::rate_limit_request,
    ));
    if let Some(auth) = auth {
        read_catalog = read_catalog.layer(middleware::from_fn_with_state(
            auth.clone(),
            auth::require_read_or_write_token_for_refresh,
        ));
        write_catalog = write_catalog.layer(middleware::from_fn_with_state(
            auth.clone(),
            auth::require_write_token,
        ));
        stats = stats.layer(middleware::from_fn_with_state(
            auth,
            auth::require_read_token,
        ));
    }

    Router::new()
        .route("/health", get(health))
        .merge(SwaggerUi::new("/docs").url("/api-docs/openapi.json", ApiDoc::openapi()))
        .merge(read_catalog)
        .merge(write_catalog)
        .merge(stats)
        .with_state(AppState {
            manga_service,
            fallback_metrics,
        })
}

async fn health() -> &'static str {
    "ok"
}
