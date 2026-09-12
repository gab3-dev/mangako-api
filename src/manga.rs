use std::time::Duration;

use axum::{
    Json,
    extract::{Extension, State},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{
    api::AppState,
    error::{ApiError, ErrorResponse},
    operations::{CacheTtl, FallbackRequest, FallbackStatsResponse},
};

const MAX_PAGE_SIZE: u32 = 100;
const MAX_OFFSET: u32 = 10_000;
const POPULAR_CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MangaResponse {
    pub id: Uuid,
    pub mangadex_id: Option<Uuid>,
    pub slug: String,
    pub primary_title: String,
    pub original_language: Option<String>,
    pub publication_demographic: Option<String>,
    pub status: Option<String>,
    pub year: Option<i32>,
    pub content_rating: Option<String>,
    pub latest_volume_number: Option<String>,
    pub source_updated_at: Option<DateTime<Utc>>,
    pub last_synced_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub localizations: Vec<MangaLocalizationResponse>,
    pub aliases: Vec<MangaAliasResponse>,
    pub covers: Vec<MangaCoverResponse>,
    pub authors: Vec<MangaCreatorResponse>,
    pub artists: Vec<MangaCreatorResponse>,
}

#[derive(Serialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MangaCreatorResponse {
    pub id: Uuid,
    pub mangadex_id: Uuid,
    pub name: String,
    pub image_url: Option<String>,
}

#[derive(Serialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MangaLocalizationResponse {
    pub language: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub is_primary: bool,
}

#[derive(Serialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MangaAliasResponse {
    pub language: String,
    pub title: String,
}

#[derive(Serialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MangaCoverResponse {
    pub id: Uuid,
    pub mangadex_cover_id: Option<Uuid>,
    pub file_name: Option<String>,
    pub source_url: Option<String>,
    pub storage_key: Option<String>,
    pub locale: Option<String>,
    pub volume: Option<String>,
    pub is_primary: bool,
    pub source_updated_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Serialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MangaVolumeResponse {
    pub id: Uuid,
    pub mangadex_cover_id: Option<Uuid>,
    pub file_name: String,
    pub source_url: String,
    pub volume: Option<String>,
    pub volume_key: Option<String>,
    pub locale: String,
    pub is_special_edition: bool,
    pub source_created_at: Option<DateTime<Utc>>,
    pub source_updated_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct MangaRow {
    id: Uuid,
    mangadex_id: Option<Uuid>,
    slug: String,
    primary_title: String,
    original_language: Option<String>,
    publication_demographic: Option<String>,
    status: Option<String>,
    year: Option<i32>,
    content_rating: Option<String>,
    mangadex_last_volume: Option<String>,
    source_updated_at: Option<DateTime<Utc>>,
    last_synced_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Deserialize, IntoParams)]
pub struct SearchMangasQuery {
    /// Manga title. When omitted or blank, returns MangaDex popular titles.
    pub title: Option<String>,
    /// Page size. Defaults to 10 and is limited to 100.
    pub limit: Option<u32>,
    /// Result offset. Defaults to 0 and is limited to 10000.
    pub offset: Option<u32>,
    /// Preferred cover language used to calculate `latestVolumeNumber`. Use `original` for the manga's original language.
    pub locale: Option<String>,
}

#[derive(Deserialize, IntoParams)]
pub struct MangaDetailQuery {
    /// Force a MangaDex refresh and bypass response cache.
    pub refresh: Option<bool>,
    /// Preferred cover language used to calculate `latestVolumeNumber`. Use `original` for the manga's original language.
    pub locale: Option<String>,
}

#[derive(Deserialize, IntoParams)]
pub struct MangaVolumesQuery {
    /// Page size. Defaults to 50 and is limited to 100.
    pub limit: Option<u32>,
    /// Result offset. Defaults to 0 and is limited to 10000.
    pub offset: Option<u32>,
    /// Force a MangaDex refresh and bypass response cache.
    pub refresh: Option<bool>,
    /// Selects the language of regular volumes, without fallback when explicit. Use `original` for the manga's original language. Without it, uses Japanese and falls back to the original language only when no active regular Japanese volumes exist. Active special editions in all languages are always included before pagination.
    pub locale: Option<String>,
}

#[utoipa::path(
    get,
    path = "/mangas",
    security(("api_token" = [])),
    params(SearchMangasQuery),
    responses(
        (status = 200, description = "Manga search results", body = [MangaResponse]),
        (status = 400, description = "Invalid search query", body = ErrorResponse),
        (status = 500, description = "Internal error", body = ErrorResponse)
    )
)]
pub async fn search_mangas(
    State(state): State<AppState>,
    Extension(fallback): Extension<FallbackRequest>,
    axum::extract::Query(query): axum::extract::Query<SearchMangasQuery>,
) -> Result<Response, ApiError> {
    let (limit, offset) = pagination(query.limit, query.offset, 10)?;
    let title = query
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty());
    let locale = normalize_locale(query.locale.as_deref());
    let mangas = state
        .manga_service
        .search_mangas_for_request(title, limit, offset, locale.as_deref(), &fallback)
        .await?;
    let mut response = Json(mangas).into_response();
    if title.is_none() {
        response
            .extensions_mut()
            .insert(CacheTtl(POPULAR_CACHE_TTL));
    }
    Ok(response)
}

#[utoipa::path(
    get,
    path = "/mangas/{manga_ref}",
    security(("api_token" = [])),
    params(
        MangaDetailQuery,
        ("manga_ref" = String, Path, description = "Internal UUID, slug, or MangaDex UUID")
    ),
    responses(
        (status = 200, description = "Manga detail", body = MangaResponse),
        (status = 404, description = "Manga not found", body = ErrorResponse),
        (status = 500, description = "Internal error", body = ErrorResponse)
    )
)]
pub async fn get_manga(
    State(state): State<AppState>,
    Extension(fallback): Extension<FallbackRequest>,
    axum::extract::Path(manga_ref): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<MangaDetailQuery>,
) -> Result<Json<MangaResponse>, ApiError> {
    let locale = normalize_locale(query.locale.as_deref());
    state
        .manga_service
        .get_manga_for_request(
            &manga_ref,
            query.refresh.unwrap_or(false),
            locale.as_deref(),
            &fallback,
        )
        .await
        .map(Json)
}

#[utoipa::path(
    get,
    path = "/mangas/{manga_ref}/volumes",
    security(("api_token" = [])),
    params(
        MangaVolumesQuery,
        ("manga_ref" = String, Path, description = "Internal UUID, slug, or MangaDex UUID")
    ),
    responses(
        (status = 200, description = "Manga volume cover records", body = [MangaVolumeResponse]),
        (status = 404, description = "Manga not found", body = ErrorResponse),
        (status = 500, description = "Internal error", body = ErrorResponse)
    )
)]
pub async fn get_manga_volumes(
    State(state): State<AppState>,
    Extension(fallback): Extension<FallbackRequest>,
    axum::extract::Path(manga_ref): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<MangaVolumesQuery>,
) -> Result<Json<Vec<MangaVolumeResponse>>, ApiError> {
    let (limit, offset) = pagination(query.limit, query.offset, 50)?;
    let locale = normalize_locale(query.locale.as_deref());
    state
        .manga_service
        .get_manga_volumes_for_request(
            &manga_ref,
            limit,
            offset,
            query.refresh.unwrap_or(false),
            locale.as_deref(),
            &fallback,
        )
        .await
        .map(Json)
}

#[utoipa::path(
    get,
    path = "/stats/mangadex-fallback",
    security(("api_token" = [])),
    responses((status = 200, description = "MangaDex fallback statistics since process start", body = FallbackStatsResponse))
)]
pub async fn mangadex_fallback_stats(State(state): State<AppState>) -> Json<FallbackStatsResponse> {
    Json(state.fallback_metrics.snapshot())
}

pub async fn load_manga_response(
    pool: &PgPool,
    manga_ref: &str,
    locale: Option<&str>,
) -> Result<Option<MangaResponse>, sqlx::Error> {
    let Some(manga) = find_manga(pool, manga_ref).await? else {
        return Ok(None);
    };

    let (localizations, aliases, covers, authors, artists, latest_volume_number) = tokio::try_join!(
        list_localizations(pool, manga.id),
        list_aliases(pool, manga.id),
        list_covers(pool, manga.id),
        list_creators(pool, manga.id, "author"),
        list_creators(pool, manga.id, "artist"),
        latest_volume_number(
            pool,
            manga.id,
            manga.mangadex_last_volume.as_deref(),
            locale,
            manga.original_language.as_deref(),
        ),
    )?;

    Ok(Some(MangaResponse {
        id: manga.id,
        mangadex_id: manga.mangadex_id,
        slug: manga.slug,
        primary_title: manga.primary_title,
        original_language: manga.original_language,
        publication_demographic: manga.publication_demographic,
        status: manga.status,
        year: manga.year,
        content_rating: manga.content_rating,
        latest_volume_number,
        source_updated_at: manga.source_updated_at,
        last_synced_at: manga.last_synced_at,
        created_at: manga.created_at,
        updated_at: manga.updated_at,
        localizations,
        aliases,
        covers,
        authors,
        artists,
    }))
}

async fn find_manga(pool: &PgPool, manga_ref: &str) -> Result<Option<MangaRow>, sqlx::Error> {
    if let Ok(id) = Uuid::parse_str(manga_ref) {
        sqlx::query_as::<_, MangaRow>(
            r#"
            SELECT id, mangadex_id, slug, primary_title, original_language,
                   publication_demographic, status, year, content_rating, mangadex_last_volume,
                   source_updated_at, last_synced_at, created_at, updated_at
            FROM mangas
            WHERE (id = $1 OR mangadex_id = $1) AND deleted_at IS NULL
            "#,
        )
        .bind(id)
        .fetch_optional(pool)
        .await
    } else {
        sqlx::query_as::<_, MangaRow>(
            r#"
            SELECT id, mangadex_id, slug, primary_title, original_language,
                   publication_demographic, status, year, content_rating, mangadex_last_volume,
                   source_updated_at, last_synced_at, created_at, updated_at
            FROM mangas
            WHERE slug = $1 AND deleted_at IS NULL
            "#,
        )
        .bind(manga_ref)
        .fetch_optional(pool)
        .await
    }
}

async fn list_localizations(
    pool: &PgPool,
    manga_id: Uuid,
) -> Result<Vec<MangaLocalizationResponse>, sqlx::Error> {
    sqlx::query_as::<_, MangaLocalizationResponse>(
        r#"
        SELECT language, title, description, is_primary
        FROM manga_localizations
        WHERE manga_id = $1
        ORDER BY is_primary DESC, language ASC
        "#,
    )
    .bind(manga_id)
    .fetch_all(pool)
    .await
}

async fn list_aliases(
    pool: &PgPool,
    manga_id: Uuid,
) -> Result<Vec<MangaAliasResponse>, sqlx::Error> {
    sqlx::query_as::<_, MangaAliasResponse>(
        r#"
        SELECT language, title
        FROM manga_aliases
        WHERE manga_id = $1
        ORDER BY language ASC, title ASC
        "#,
    )
    .bind(manga_id)
    .fetch_all(pool)
    .await
}

async fn list_covers(
    pool: &PgPool,
    manga_id: Uuid,
) -> Result<Vec<MangaCoverResponse>, sqlx::Error> {
    sqlx::query_as::<_, MangaCoverResponse>(
        r#"
        SELECT id, mangadex_cover_id, file_name, source_url, storage_key,
               locale, volume, is_primary, source_updated_at, updated_at
        FROM manga_covers
        WHERE manga_id = $1 AND deleted_at IS NULL
        ORDER BY is_primary DESC, volume ASC NULLS LAST, updated_at DESC
        "#,
    )
    .bind(manga_id)
    .fetch_all(pool)
    .await
}

async fn list_creators(
    pool: &PgPool,
    manga_id: Uuid,
    role: &str,
) -> Result<Vec<MangaCreatorResponse>, sqlx::Error> {
    sqlx::query_as::<_, MangaCreatorResponse>(
        r#"
        SELECT c.id, c.mangadex_id, c.name, c.image_url
        FROM creators c
        JOIN manga_creators mc ON mc.creator_id = c.id
        WHERE mc.manga_id = $1 AND mc.role = $2 AND c.deleted_at IS NULL
        ORDER BY c.name ASC
        "#,
    )
    .bind(manga_id)
    .bind(role)
    .fetch_all(pool)
    .await
}

async fn latest_volume_number(
    pool: &PgPool,
    manga_id: Uuid,
    mangadex_last_volume: Option<&str>,
    locale: Option<&str>,
    original_language: Option<&str>,
) -> Result<Option<String>, sqlx::Error> {
    let preferred_language =
        requested_language(locale, original_language).unwrap_or_else(|| "ja".to_string());
    let synced = latest_synced_volume_number(pool, manga_id, &preferred_language).await?;
    let fallback = match original_language
        .and_then(normalize_language)
        .filter(|language| language != &preferred_language)
    {
        Some(language) => latest_synced_volume_number(pool, manga_id, &language).await?,
        None => None,
    };

    if locale.is_some() {
        Ok(synced.or(fallback))
    } else {
        Ok(synced
            .or(fallback)
            .or_else(|| mangadex_last_volume.map(ToOwned::to_owned)))
    }
}

async fn latest_synced_volume_number(
    pool: &PgPool,
    manga_id: Uuid,
    language: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        SELECT max(volume_key::numeric)::text
        FROM manga_volumes
        WHERE manga_id = $1
          AND deleted_at IS NULL
          AND is_special_edition = false
          AND split_part(replace(lower(locale), '_', '-'), '-', 1) = $2
          AND volume_key ~ '^[0-9]+([.][0-9]+)?$'
        "#,
    )
    .bind(manga_id)
    .bind(language)
    .fetch_one(pool)
    .await
}

fn normalize_locale(locale: Option<&str>) -> Option<String> {
    locale
        .map(str::trim)
        .filter(|locale| !locale.is_empty())
        .and_then(normalize_language)
}

pub(crate) fn requested_language(
    locale: Option<&str>,
    original_language: Option<&str>,
) -> Option<String> {
    match locale {
        Some("original") => original_language.and_then(normalize_language),
        Some(language) => normalize_language(language),
        None => None,
    }
}

fn normalize_language(locale: &str) -> Option<String> {
    let language = locale.trim().to_ascii_lowercase().replace('_', "-");
    if language.is_empty() {
        None
    } else if language == "original" {
        Some(language)
    } else {
        Some(language.split('-').next().unwrap_or_default().to_string())
    }
}

fn pagination(
    limit: Option<u32>,
    offset: Option<u32>,
    default_limit: u32,
) -> Result<(u32, u32), ApiError> {
    let limit = limit.unwrap_or(default_limit);
    let offset = offset.unwrap_or(0);
    if limit == 0 || limit > MAX_PAGE_SIZE {
        return Err(ApiError::BadRequest {
            message: format!("limit must be between 1 and {MAX_PAGE_SIZE}"),
        });
    }
    if offset > MAX_OFFSET {
        return Err(ApiError::BadRequest {
            message: format!("offset must be at most {MAX_OFFSET}"),
        });
    }
    Ok((limit, offset))
}

#[cfg(test)]
mod tests {
    use super::{normalize_locale, requested_language};

    #[test]
    fn locale_queries_match_a_base_language() {
        assert_eq!(normalize_locale(Some("PT-BR")).as_deref(), Some("pt"));
    }

    #[test]
    fn original_locale_uses_the_manga_original_language() {
        assert_eq!(
            requested_language(Some("original"), Some("ko")),
            Some("ko".to_string())
        );
    }
}
