use axum::{Router, middleware, routing::get};
use sqlx::PgPool;
use std::time::Duration;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::{
    auth, manga,
    manga_service::MangaService,
    mangadex::MangaDexClient,
    openapi::ApiDoc,
    operations::{OperationalConfig, ResponseCache},
};

#[derive(Clone)]
pub struct AppState {
    pub manga_service: MangaService,
}

pub fn router(pool: PgPool) -> Router {
    build_router(pool, None, OperationalConfig::default())
}

pub fn router_with_api_token(pool: PgPool, api_token: String) -> Router {
    build_router(pool, Some(api_token), OperationalConfig::default())
}

pub fn router_with_operations(
    pool: PgPool,
    api_token: String,
    operations: OperationalConfig,
) -> Router {
    build_router(pool, Some(api_token), operations)
}

fn build_router(pool: PgPool, api_token: Option<String>, operations: OperationalConfig) -> Router {
    let http = reqwest::Client::builder()
        .user_agent(concat!("mangako-api/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .build()
        .expect("valid reqwest client");
    let mangadex = MangaDexClient::new(http);
    let manga_service = MangaService::new(pool, mangadex);

    let mut catalog = Router::new()
        .route("/mangas", get(manga::search_mangas))
        .route("/mangas/", get(manga::search_mangas))
        .route("/mangas/{manga_ref}", get(manga::get_manga))
        .route("/mangas/{manga_ref}/volumes", get(manga::get_manga_volumes));

    if let Some(cache) = operations.cache {
        catalog = catalog.layer(middleware::from_fn_with_state(
            ResponseCache::new(cache),
            crate::operations::cache_get_response,
        ));
    }

    if let Some(api_token) = api_token {
        catalog = catalog.layer(middleware::from_fn_with_state(
            api_token,
            auth::require_api_token,
        ));
    }

    Router::new()
        .route("/health", get(health))
        .merge(SwaggerUi::new("/docs").url("/api-docs/openapi.json", ApiDoc::openapi()))
        .merge(catalog)
        .with_state(AppState { manga_service })
}

async fn health() -> &'static str {
    "ok"
}
