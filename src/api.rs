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
    cover_storage::CoverStorage,
    manga,
    manga_service::MangaService,
    mangadex::{self, MangaDexClient},
    openapi::ApiDoc,
    operations::{FallbackMetrics, OperationalConfig, RateLimiter, RequestMetrics, ResponseCache},
};

#[derive(Clone)]
pub struct AppState {
    pub manga_service: MangaService,
    pub fallback_metrics: FallbackMetrics,
    pub request_metrics: RequestMetrics,
    pub cover_storage: Option<CoverStorage>,
}

pub fn router(pool: PgPool) -> Router {
    build_router(pool, None, OperationalConfig::default(), None, None)
}

pub fn router_with_api_tokens(pool: PgPool, read_token: String, write_token: String) -> Router {
    build_router(
        pool,
        Some(AuthConfig {
            read_token,
            write_token,
        }),
        OperationalConfig::default(),
        None,
        None,
    )
}

pub fn router_with_operations(
    pool: PgPool,
    auth: AuthConfig,
    operations: OperationalConfig,
) -> Router {
    build_router(pool, Some(auth), operations, None, None)
}

pub fn router_with_cover_storage(
    pool: PgPool,
    auth: AuthConfig,
    operations: OperationalConfig,
    cover_storage: CoverStorage,
) -> Router {
    router_with_mangadex_proxy(pool, auth, operations, cover_storage, None)
}

pub fn router_with_mangadex_proxy(
    pool: PgPool,
    auth: AuthConfig,
    operations: OperationalConfig,
    cover_storage: CoverStorage,
    mangadex_proxy_url: Option<reqwest::Url>,
) -> Router {
    build_router(
        pool,
        Some(auth),
        operations,
        Some(cover_storage),
        mangadex_proxy_url,
    )
}

fn build_router(
    pool: PgPool,
    auth: Option<AuthConfig>,
    operations: OperationalConfig,
    cover_storage: Option<CoverStorage>,
    mangadex_proxy_url: Option<reqwest::Url>,
) -> Router {
    let http = mangadex::http_client(mangadex_proxy_url, Duration::from_secs(15))
        .expect("MangaDex proxy URL is validated by Config");
    let request_metrics = RequestMetrics::new(pool.clone());
    let mangadex = MangaDexClient::new(http, request_metrics.clone());
    let manga_service = MangaService::new(pool, mangadex);
    let fallback_metrics = FallbackMetrics::new();

    let mut read_catalog = Router::new()
        .route("/mangas", get(manga::search_mangas))
        .route("/mangas/", get(manga::search_mangas))
        .route("/mangas/{manga_ref}", get(manga::get_manga))
        .route("/mangas/{manga_ref}/volumes", get(manga::get_manga_volumes));
    let mut stats = Router::new()
        .route(
            "/stats/mangadex-fallback",
            get(manga::mangadex_fallback_stats),
        )
        .route("/stats/requests", get(manga::request_metrics));
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
            "/mangas/{manga_ref}/covers/upload",
            post(manga::upload_manga_cover),
        )
        .route(
            "/mangas/{manga_ref}/volumes",
            post(manga::create_manga_volume),
        )
        .route(
            "/mangas/{manga_ref}/volumes/upload",
            post(manga::upload_manga_volume),
        )
        .route(
            "/mangas/{manga_ref}/volumes/{volume_id}",
            patch(manga::update_manga_volume).delete(manga::delete_manga_volume),
        )
        .layer(DefaultBodyLimit::max(
            cover_storage
                .as_ref()
                .map_or(operations.max_json_body_bytes, |storage| {
                    storage.max_bytes().max(operations.max_json_body_bytes)
                }),
        ));

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
    read_catalog = read_catalog.layer(middleware::from_fn_with_state(
        request_metrics.clone(),
        crate::operations::track_api_request,
    ));
    write_catalog = write_catalog.layer(middleware::from_fn_with_state(
        request_metrics.clone(),
        crate::operations::track_api_request,
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
            request_metrics,
            cover_storage,
        })
}

async fn health() -> &'static str {
    "ok"
}
