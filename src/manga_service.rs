use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    error::ApiError,
    manga::{MangaResponse, MangaVolumeResponse, load_manga_response, requested_language},
    mangadex::{MangaDexApi, MangaDexClient, MangaDexCover, MangaDexManga, cover_url},
    operations::FallbackRequest,
};

#[derive(Clone)]
pub struct MangaService<C = MangaDexClient> {
    pool: PgPool,
    mangadex: C,
}

impl<C> MangaService<C>
where
    C: MangaDexApi,
{
    pub fn new(pool: PgPool, mangadex: C) -> Self {
        Self { pool, mangadex }
    }

    pub async fn get_manga(
        &self,
        manga_ref: &str,
        refresh: bool,
        locale: Option<&str>,
    ) -> Result<MangaResponse, ApiError> {
        self.get_manga_inner(manga_ref, refresh, locale, None).await
    }

    pub async fn get_manga_for_request(
        &self,
        manga_ref: &str,
        refresh: bool,
        locale: Option<&str>,
        fallback: &FallbackRequest,
    ) -> Result<MangaResponse, ApiError> {
        self.get_manga_inner(manga_ref, refresh, locale, Some(fallback))
            .await
    }

    async fn get_manga_inner(
        &self,
        manga_ref: &str,
        refresh: bool,
        locale: Option<&str>,
        fallback: Option<&FallbackRequest>,
    ) -> Result<MangaResponse, ApiError> {
        if let Some(local) = load_manga_response(&self.pool, manga_ref, locale).await? {
            let stale = local
                .last_synced_at
                .is_none_or(|synced| synced < Utc::now() - Duration::days(1));
            if !refresh && !stale {
                return self.populate_latest_volume(local, locale).await;
            }

            let Some(mangadex_id) = local.mangadex_id else {
                return Ok(local);
            };
            match self.mangadex.get_manga(mangadex_id).await {
                Ok(Some(mut remote)) => {
                    if locale.is_none() {
                        self.populate_remote_latest_volume(&mut remote).await;
                    }
                    self.upsert_mangadex_manga(&remote).await?;
                    let response = load_manga_response(&self.pool, &remote.id.to_string(), locale)
                        .await?
                        .ok_or(ApiError::MangaNotFound)?;
                    return self.populate_latest_volume(response, locale).await;
                }
                Ok(None) => {
                    record_fallback(fallback);
                    return Ok(local);
                }
                Err(error) => {
                    tracing::warn!(%error, %mangadex_id, "serving stale local manga");
                    record_fallback(fallback);
                    return Ok(local);
                }
            }
        }

        let mangadex_id = Uuid::parse_str(manga_ref).map_err(|_| ApiError::MangaNotFound)?;
        let mut remote = self
            .mangadex
            .get_manga(mangadex_id)
            .await?
            .ok_or(ApiError::MangaNotFound)?;

        if locale.is_none() {
            self.populate_remote_latest_volume(&mut remote).await;
        }
        self.upsert_mangadex_manga(&remote).await?;

        let response = load_manga_response(&self.pool, &remote.id.to_string(), locale)
            .await?
            .ok_or(ApiError::MangaNotFound)?;
        self.populate_latest_volume(response, locale).await
    }

    pub async fn search_mangas(
        &self,
        title: Option<&str>,
        limit: u32,
        offset: u32,
        locale: Option<&str>,
    ) -> Result<Vec<MangaResponse>, ApiError> {
        self.search_mangas_inner(title, limit, offset, locale, None)
            .await
    }

    pub async fn search_mangas_for_request(
        &self,
        title: Option<&str>,
        limit: u32,
        offset: u32,
        locale: Option<&str>,
        fallback: &FallbackRequest,
    ) -> Result<Vec<MangaResponse>, ApiError> {
        self.search_mangas_inner(title, limit, offset, locale, Some(fallback))
            .await
    }

    async fn search_mangas_inner(
        &self,
        title: Option<&str>,
        limit: u32,
        offset: u32,
        locale: Option<&str>,
        fallback: Option<&FallbackRequest>,
    ) -> Result<Vec<MangaResponse>, ApiError> {
        let remote_results = match self.mangadex.search_mangas(title, offset, limit).await {
            Ok(results) => results,
            Err(error) => {
                tracing::warn!(%error, ?title, limit, offset, "MangaDex search failed; using local fallback");
                record_fallback(fallback);
                return search_local_mangas(&self.pool, title, limit, offset, locale)
                    .await
                    .map_err(ApiError::from);
            }
        };

        let mut results = Vec::with_capacity(remote_results.len());
        for mut remote in remote_results {
            if locale.is_none() {
                self.populate_remote_latest_volume(&mut remote).await;
            }
            self.upsert_mangadex_manga(&remote).await?;
            if let Some(manga) =
                load_manga_response(&self.pool, &remote.id.to_string(), locale).await?
            {
                results.push(self.populate_latest_volume(manga, locale).await?);
            }
        }

        Ok(results)
    }

    pub async fn get_manga_volumes(
        &self,
        manga_ref: &str,
        limit: u32,
        offset: u32,
        refresh: bool,
        locale: Option<&str>,
    ) -> Result<Vec<MangaVolumeResponse>, ApiError> {
        self.get_manga_volumes_inner(manga_ref, limit, offset, refresh, locale, None)
            .await
    }

    pub async fn get_manga_volumes_for_request(
        &self,
        manga_ref: &str,
        limit: u32,
        offset: u32,
        refresh: bool,
        locale: Option<&str>,
        fallback: &FallbackRequest,
    ) -> Result<Vec<MangaVolumeResponse>, ApiError> {
        self.get_manga_volumes_inner(manga_ref, limit, offset, refresh, locale, Some(fallback))
            .await
    }

    async fn get_manga_volumes_inner(
        &self,
        manga_ref: &str,
        limit: u32,
        offset: u32,
        refresh: bool,
        locale: Option<&str>,
        fallback: Option<&FallbackRequest>,
    ) -> Result<Vec<MangaVolumeResponse>, ApiError> {
        let manga = self
            .get_manga_inner(manga_ref, refresh, locale, fallback)
            .await?;
        let has_local_volumes = has_volumes(&self.pool, manga.id).await?;
        let volumes_stale = volumes_are_stale(&self.pool, manga.id).await?;

        let Some(mangadex_id) = manga.mangadex_id else {
            return list_volumes(
                &self.pool,
                manga.id,
                limit,
                offset,
                locale,
                manga.original_language.as_deref(),
            )
            .await
            .map_err(ApiError::from);
        };

        if (refresh || !has_local_volumes || volumes_stale)
            && let Err(error) = self.refresh_manga_volumes(manga.id, mangadex_id).await
        {
            if !has_local_volumes {
                return Err(error);
            }
            tracing::warn!(%error, %mangadex_id, "serving stale local manga volumes");
            record_fallback(fallback);
        }

        list_volumes(
            &self.pool,
            manga.id,
            limit,
            offset,
            locale,
            manga.original_language.as_deref(),
        )
        .await
        .map_err(ApiError::from)
    }

    async fn populate_latest_volume(
        &self,
        mut manga: MangaResponse,
        locale: Option<&str>,
    ) -> Result<MangaResponse, ApiError> {
        if manga.latest_volume_number.is_some() {
            return Ok(manga);
        }
        let Some(mangadex_id) = manga.mangadex_id else {
            return Ok(manga);
        };
        let preferred_language = requested_language(locale, manga.original_language.as_deref())
            .unwrap_or_else(|| "ja".to_string());
        let mut latest_volume = self
            .fetch_latest_volume_number(mangadex_id, &preferred_language)
            .await;
        if latest_volume.is_none()
            && let Some(original_language) = manga
                .original_language
                .as_deref()
                .and_then(|language| requested_language(Some(language), None))
                .filter(|language| language != &preferred_language)
        {
            latest_volume = self
                .fetch_latest_volume_number(mangadex_id, &original_language)
                .await;
        }
        let Some(latest_volume) = latest_volume else {
            return Ok(manga);
        };

        if locale.is_some() {
            manga.latest_volume_number = Some(latest_volume);
            return Ok(manga);
        }

        sqlx::query(
            r#"
            UPDATE mangas
            SET mangadex_last_volume = $2
            WHERE id = $1 AND mangadex_last_volume IS NULL
            "#,
        )
        .bind(manga.id)
        .bind(latest_volume)
        .execute(&self.pool)
        .await?;

        load_manga_response(&self.pool, &manga.id.to_string(), None)
            .await?
            .ok_or(ApiError::MangaNotFound)
    }

    async fn populate_remote_latest_volume(&self, manga: &mut MangaDexManga) {
        if manga
            .attributes
            .last_volume
            .as_deref()
            .is_some_and(|volume| !volume.trim().is_empty())
        {
            return;
        }

        manga.attributes.last_volume = self.fetch_latest_volume_number(manga.id, "ja").await;
    }

    async fn fetch_latest_volume_number(
        &self,
        mangadex_id: Uuid,
        language: &str,
    ) -> Option<String> {
        match self
            .mangadex
            .latest_volume_number(mangadex_id, language)
            .await
        {
            Ok(volume) => volume.as_deref().and_then(normalized_volume_key),
            Err(error) => {
                tracing::warn!(%error, %mangadex_id, "MangaDex latest volume lookup failed");
                None
            }
        }
    }

    async fn upsert_mangadex_manga(&self, manga: &MangaDexManga) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let primary_title = primary_title(manga);
        let slug = slug_for(&primary_title, manga.id);

        let manga_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO mangas (
                mangadex_id, slug, primary_title, original_language,
                publication_demographic, status, year, content_rating,
                mangadex_version, mangadex_last_volume, source_updated_at, last_synced_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, now())
            ON CONFLICT (mangadex_id) DO UPDATE SET
                primary_title = EXCLUDED.primary_title,
                original_language = EXCLUDED.original_language,
                publication_demographic = EXCLUDED.publication_demographic,
                status = EXCLUDED.status,
                year = EXCLUDED.year,
                content_rating = EXCLUDED.content_rating,
                mangadex_version = EXCLUDED.mangadex_version,
                mangadex_last_volume = COALESCE(
                    EXCLUDED.mangadex_last_volume,
                    mangas.mangadex_last_volume
                ),
                source_updated_at = EXCLUDED.source_updated_at,
                last_synced_at = now(),
                deleted_at = NULL
            RETURNING id
            "#,
        )
        .bind(manga.id)
        .bind(slug)
        .bind(primary_title)
        .bind(&manga.attributes.original_language)
        .bind(&manga.attributes.publication_demographic)
        .bind(&manga.attributes.status)
        .bind(manga.attributes.year)
        .bind(&manga.attributes.content_rating)
        .bind(manga.attributes.version)
        .bind(
            manga
                .attributes
                .last_volume
                .as_deref()
                .filter(|volume| !volume.trim().is_empty()),
        )
        .bind(manga.attributes.updated_at)
        .fetch_one(&mut *tx)
        .await?;

        upsert_localizations(&mut tx, manga_id, manga).await?;
        upsert_aliases(&mut tx, manga_id, manga).await?;
        upsert_cover(&mut tx, manga_id, manga).await?;
        upsert_creators(&mut tx, manga_id, manga).await?;
        upsert_source_sync(&mut tx, manga_id, manga).await?;

        tx.commit().await
    }

    async fn refresh_manga_volumes(
        &self,
        manga_id: Uuid,
        mangadex_id: Uuid,
    ) -> Result<(), ApiError> {
        let limit = 50;
        let mut offset = 0;
        let mut remote_cover_ids = Vec::new();

        loop {
            let page = self
                .mangadex
                .list_covers(mangadex_id, offset, limit)
                .await?;
            if page.is_empty() {
                break;
            }

            let mut tx = self.pool.begin().await?;
            for cover in &page {
                remote_cover_ids.push(cover.id);
                upsert_volume(&mut tx, manga_id, mangadex_id, cover).await?;
            }
            tx.commit().await?;

            if page.len() < limit as usize {
                break;
            }
            offset += limit;
        }

        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
            UPDATE manga_volumes
            SET deleted_at = now()
            WHERE manga_id = $1
              AND mangadex_cover_id IS NOT NULL
              AND deleted_at IS NULL
              AND NOT (mangadex_cover_id = ANY($2))
            "#,
        )
        .bind(manga_id)
        .bind(&remote_cover_ids)
        .execute(&mut *tx)
        .await?;
        mark_volumes_synced(&mut tx, manga_id, mangadex_id).await?;
        tx.commit().await?;

        Ok(())
    }
}

fn record_fallback(fallback: Option<&FallbackRequest>) {
    if let Some(fallback) = fallback {
        fallback.record();
    }
}

async fn search_local_mangas(
    pool: &PgPool,
    title: Option<&str>,
    limit: u32,
    offset: u32,
    locale: Option<&str>,
) -> Result<Vec<MangaResponse>, sqlx::Error> {
    let manga_ids = if let Some(title) = title {
        let pattern = format!("%{title}%");
        sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT m.id
            FROM mangas m
            WHERE m.deleted_at IS NULL
              AND (
                  m.primary_title ILIKE $1
                  OR EXISTS (
                      SELECT 1 FROM manga_localizations ml
                      WHERE ml.manga_id = m.id AND ml.title ILIKE $1
                  )
                  OR EXISTS (
                      SELECT 1 FROM manga_aliases ma
                      WHERE ma.manga_id = m.id AND ma.title ILIKE $1
                  )
              )
            ORDER BY
                CASE WHEN lower(m.primary_title) = lower($2) THEN 0 ELSE 1 END,
                similarity(m.primary_title, $2) DESC,
                m.id
            LIMIT $3 OFFSET $4
            "#,
        )
        .bind(pattern)
        .bind(title)
        .bind(i64::from(limit))
        .bind(i64::from(offset))
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT id
            FROM mangas
            WHERE deleted_at IS NULL
            ORDER BY last_synced_at DESC NULLS LAST, updated_at DESC, id
            LIMIT $1 OFFSET $2
            "#,
        )
        .bind(i64::from(limit))
        .bind(i64::from(offset))
        .fetch_all(pool)
        .await?
    };

    let mut results = Vec::with_capacity(manga_ids.len());
    for manga_id in manga_ids {
        if let Some(manga) = load_manga_response(pool, &manga_id.to_string(), locale).await? {
            results.push(manga);
        }
    }

    Ok(results)
}

async fn list_volumes(
    pool: &PgPool,
    manga_id: Uuid,
    limit: u32,
    offset: u32,
    locale: Option<&str>,
    original_language: Option<&str>,
) -> Result<Vec<MangaVolumeResponse>, sqlx::Error> {
    let preferred_language =
        requested_language(locale, original_language).unwrap_or_else(|| "ja".to_string());
    let fallback_language = if locale.is_none() {
        original_language
            .and_then(|language| requested_language(Some(language), None))
            .filter(|language| language != &preferred_language)
    } else {
        None
    };
    sqlx::query_as::<_, MangaVolumeResponse>(
        r#"
        SELECT id, mangadex_cover_id, file_name, source_url, volume, volume_key,
               locale, is_special_edition, source_created_at, source_updated_at, updated_at
        FROM manga_volumes
        WHERE manga_id = $1
          AND deleted_at IS NULL
          AND (
              split_part(replace(lower(locale), '_', '-'), '-', 1) = $2
              OR (
                  $3::text IS NOT NULL
                  AND NOT EXISTS (
                      SELECT 1
                      FROM manga_volumes preferred
                      WHERE preferred.manga_id = manga_volumes.manga_id
                        AND preferred.deleted_at IS NULL
                        AND split_part(replace(lower(preferred.locale), '_', '-'), '-', 1) = $2
                  )
                  AND split_part(replace(lower(locale), '_', '-'), '-', 1) = $3
              )
          )
        ORDER BY
            CASE WHEN volume_key ~ '^[0-9]+([.][0-9]+)?$' THEN volume_key::numeric END ASC NULLS LAST,
            locale ASC,
            source_updated_at DESC NULLS LAST,
            id
        LIMIT $4 OFFSET $5
        "#,
    )
    .bind(manga_id)
    .bind(preferred_language)
    .bind(fallback_language)
    .bind(i64::from(limit))
    .bind(i64::from(offset))
    .fetch_all(pool)
    .await
}

async fn has_volumes(pool: &PgPool, manga_id: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM manga_volumes WHERE manga_id = $1 AND deleted_at IS NULL)",
    )
    .bind(manga_id)
    .fetch_one(pool)
    .await
}

async fn volumes_are_stale(pool: &PgPool, manga_id: Uuid) -> Result<bool, sqlx::Error> {
    let last_success: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT volumes_last_success_at FROM manga_source_syncs WHERE manga_id = $1",
    )
    .bind(manga_id)
    .fetch_optional(pool)
    .await?
    .flatten();

    Ok(last_success.is_none_or(|synced| synced < Utc::now() - Duration::days(1)))
}

async fn upsert_volume(
    tx: &mut Transaction<'_, Postgres>,
    manga_id: Uuid,
    mangadex_id: Uuid,
    cover: &MangaDexCover,
) -> Result<(), sqlx::Error> {
    let file_name = cover.attributes.file_name.trim();
    if file_name.is_empty() {
        return Ok(());
    }

    let locale = cover.attributes.locale.as_deref().unwrap_or("und");
    let volume_key = cover
        .attributes
        .volume
        .as_deref()
        .and_then(normalized_volume_key);
    let is_special_edition =
        volume_key.is_none() || is_fractional_volume_key(volume_key.as_deref());
    let source_url = cover_url(mangadex_id, file_name);

    if let Some(volume_key) = &volume_key {
        let existing_id: Option<Uuid> = sqlx::query_scalar(
            r#"
            SELECT id
            FROM manga_volumes
            WHERE manga_id = $1 AND volume_key = $2 AND locale = $3 AND deleted_at IS NULL
            "#,
        )
        .bind(manga_id)
        .bind(volume_key)
        .bind(locale)
        .fetch_optional(&mut **tx)
        .await?;

        if let Some(existing_id) = existing_id {
            sqlx::query(
                r#"
                UPDATE manga_volumes SET
                    mangadex_cover_id = $2,
                    file_name = $3,
                    source_url = $4,
                    volume = $5,
                    is_special_edition = $6,
                    mangadex_version = $7,
                    source_created_at = $8,
                    source_updated_at = $9,
                    deleted_at = NULL
                WHERE id = $1
                  AND (source_updated_at IS NULL OR $9 IS NULL OR source_updated_at <= $9)
                "#,
            )
            .bind(existing_id)
            .bind(cover.id)
            .bind(file_name)
            .bind(source_url)
            .bind(&cover.attributes.volume)
            .bind(is_special_edition)
            .bind(cover.attributes.version)
            .bind(cover.attributes.created_at)
            .bind(cover.attributes.updated_at)
            .execute(&mut **tx)
            .await?;
            return Ok(());
        }
    }

    sqlx::query(
        r#"
        INSERT INTO manga_volumes (
            manga_id, mangadex_cover_id, file_name, source_url, volume, volume_key,
            locale, is_special_edition, mangadex_version, source_created_at, source_updated_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        ON CONFLICT (mangadex_cover_id) DO UPDATE SET
            file_name = EXCLUDED.file_name,
            source_url = EXCLUDED.source_url,
            volume = EXCLUDED.volume,
            volume_key = EXCLUDED.volume_key,
            locale = EXCLUDED.locale,
            is_special_edition = EXCLUDED.is_special_edition,
            mangadex_version = EXCLUDED.mangadex_version,
            source_created_at = EXCLUDED.source_created_at,
            source_updated_at = EXCLUDED.source_updated_at,
            deleted_at = NULL
        "#,
    )
    .bind(manga_id)
    .bind(cover.id)
    .bind(file_name)
    .bind(source_url)
    .bind(&cover.attributes.volume)
    .bind(&volume_key)
    .bind(locale)
    .bind(is_special_edition)
    .bind(cover.attributes.version)
    .bind(cover.attributes.created_at)
    .bind(cover.attributes.updated_at)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

async fn upsert_localizations(
    tx: &mut Transaction<'_, Postgres>,
    manga_id: Uuid,
    manga: &MangaDexManga,
) -> Result<(), sqlx::Error> {
    let primary_language = primary_language(manga);
    let mut localizations = BTreeMap::<String, (Option<String>, Option<String>)>::new();

    let mut canonical_titles = manga.attributes.title.iter().collect::<Vec<_>>();
    canonical_titles.sort_by_key(|(language, _)| language.to_ascii_lowercase());
    for (language, title) in canonical_titles {
        let Some(title) = title
            .as_ref()
            .filter(|title| !title.trim().is_empty())
            .cloned()
        else {
            continue;
        };
        let entry = localizations
            .entry(language.to_ascii_lowercase())
            .or_default();
        if entry.0.is_none() {
            entry.0 = Some(title);
        }
    }

    for alternate_titles in &manga.attributes.alt_titles {
        let mut alternate_titles = alternate_titles.iter().collect::<Vec<_>>();
        alternate_titles.sort_by_key(|(language, _)| language.to_ascii_lowercase());
        for (language, title) in alternate_titles {
            let Some(title) = title
                .as_ref()
                .filter(|title| !title.trim().is_empty())
                .cloned()
            else {
                continue;
            };
            let entry = localizations
                .entry(language.to_ascii_lowercase())
                .or_default();
            if entry.0.is_none() {
                entry.0 = Some(title);
            }
        }
    }

    let mut descriptions = manga.attributes.description.iter().collect::<Vec<_>>();
    descriptions.sort_by_key(|(language, _)| language.to_ascii_lowercase());
    for (language, description) in descriptions {
        let Some(description) = description
            .as_ref()
            .filter(|description| !description.trim().is_empty())
            .cloned()
        else {
            continue;
        };
        let entry = localizations
            .entry(language.to_ascii_lowercase())
            .or_default();
        if entry.1.is_none() {
            entry.1 = Some(description);
        }
    }

    sqlx::query("DELETE FROM manga_localizations WHERE manga_id = $1")
        .bind(manga_id)
        .execute(&mut **tx)
        .await?;

    for (language, (title, description)) in localizations {
        if title.is_none() && description.is_none() {
            continue;
        }

        sqlx::query(
            r#"
            INSERT INTO manga_localizations (manga_id, language, title, description, is_primary)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (manga_id, language) DO UPDATE SET
                title = EXCLUDED.title,
                description = EXCLUDED.description,
                is_primary = EXCLUDED.is_primary
            "#,
        )
        .bind(manga_id)
        .bind(&language)
        .bind(title)
        .bind(description)
        .bind(language == primary_language)
        .execute(&mut **tx)
        .await?;
    }

    Ok(())
}

async fn upsert_aliases(
    tx: &mut Transaction<'_, Postgres>,
    manga_id: Uuid,
    manga: &MangaDexManga,
) -> Result<(), sqlx::Error> {
    let mut aliases = BTreeMap::<(String, String), ()>::new();

    sqlx::query("DELETE FROM manga_aliases WHERE manga_id = $1")
        .bind(manga_id)
        .execute(&mut **tx)
        .await?;

    for alt_title in &manga.attributes.alt_titles {
        for (language, title) in alt_title {
            let Some(title) = title.as_ref().filter(|title| !title.trim().is_empty()) else {
                continue;
            };
            aliases.insert((language.to_ascii_lowercase(), title.clone()), ());
        }
    }

    for ((language, title), _) in aliases {
        sqlx::query(
            r#"
            INSERT INTO manga_aliases (manga_id, language, title)
            VALUES ($1, $2, $3)
            ON CONFLICT (manga_id, language, normalized_title) DO NOTHING
            "#,
        )
        .bind(manga_id)
        .bind(language)
        .bind(title)
        .execute(&mut **tx)
        .await?;
    }

    Ok(())
}

async fn upsert_cover(
    tx: &mut Transaction<'_, Postgres>,
    manga_id: Uuid,
    manga: &MangaDexManga,
) -> Result<(), sqlx::Error> {
    let Some(cover) = manga.cover_art() else {
        return Ok(());
    };
    let Some(cover_id) = cover.id else {
        return Ok(());
    };
    let Some(attributes) = &cover.attributes else {
        return Ok(());
    };
    let Some(file_name) = attributes
        .file_name
        .as_ref()
        .filter(|file_name| !file_name.trim().is_empty())
    else {
        return Ok(());
    };

    sqlx::query("UPDATE manga_covers SET is_primary = false WHERE manga_id = $1")
        .bind(manga_id)
        .execute(&mut **tx)
        .await?;

    sqlx::query(
        r#"
        INSERT INTO manga_covers (
            manga_id, mangadex_cover_id, file_name, source_url, locale, volume,
            is_primary, mangadex_version, source_updated_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, true, $7, $8)
        ON CONFLICT (mangadex_cover_id) DO UPDATE SET
            file_name = EXCLUDED.file_name,
            source_url = EXCLUDED.source_url,
            locale = EXCLUDED.locale,
            volume = EXCLUDED.volume,
            is_primary = true,
            mangadex_version = EXCLUDED.mangadex_version,
            source_updated_at = EXCLUDED.source_updated_at,
            deleted_at = NULL
        "#,
    )
    .bind(manga_id)
    .bind(cover_id)
    .bind(file_name)
    .bind(cover_url(manga.id, file_name))
    .bind(&attributes.locale)
    .bind(&attributes.volume)
    .bind(attributes.version)
    .bind(attributes.updated_at)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

async fn upsert_creators(
    tx: &mut Transaction<'_, Postgres>,
    manga_id: Uuid,
    manga: &MangaDexManga,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM manga_creators WHERE manga_id = $1")
        .bind(manga_id)
        .execute(&mut **tx)
        .await?;

    for relationship in &manga.relationships {
        let Some(role) = relationship.kind.as_deref() else {
            continue;
        };
        if role != "author" && role != "artist" {
            continue;
        }

        let Some(mangadex_id) = relationship.id else {
            continue;
        };
        let Some(attributes) = &relationship.attributes else {
            continue;
        };
        let Some(name) = attributes
            .name
            .as_ref()
            .filter(|name| !name.trim().is_empty())
        else {
            continue;
        };

        let creator_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO creators (
                mangadex_id, name, image_url, mangadex_version, source_updated_at
            )
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (mangadex_id) DO UPDATE SET
                name = EXCLUDED.name,
                image_url = EXCLUDED.image_url,
                mangadex_version = EXCLUDED.mangadex_version,
                source_updated_at = EXCLUDED.source_updated_at,
                deleted_at = NULL
            RETURNING id
            "#,
        )
        .bind(mangadex_id)
        .bind(name)
        .bind(&attributes.image_url)
        .bind(attributes.version)
        .bind(attributes.updated_at)
        .fetch_one(&mut **tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO manga_creators (manga_id, creator_id, role)
            VALUES ($1, $2, $3)
            ON CONFLICT (manga_id, creator_id, role) DO NOTHING
            "#,
        )
        .bind(manga_id)
        .bind(creator_id)
        .bind(role)
        .execute(&mut **tx)
        .await?;
    }

    Ok(())
}

async fn upsert_source_sync(
    tx: &mut Transaction<'_, Postgres>,
    manga_id: Uuid,
    manga: &MangaDexManga,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO manga_source_syncs (
            manga_id, source, source_id, last_checked_at, last_success_at, next_check_at
        )
        VALUES ($1, 'mangadex', $2, now(), now(), now() + interval '1 day')
        ON CONFLICT (source, source_id) DO UPDATE SET
            last_checked_at = now(),
            last_success_at = now(),
            last_error_at = NULL,
            last_error = NULL,
            next_check_at = now() + interval '1 day'
        "#,
    )
    .bind(manga_id)
    .bind(manga.id)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

async fn mark_volumes_synced(
    tx: &mut Transaction<'_, Postgres>,
    manga_id: Uuid,
    mangadex_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO manga_source_syncs (
            manga_id, source, source_id, last_checked_at, last_success_at,
            next_check_at, volumes_last_checked_at, volumes_last_success_at
        )
        VALUES ($1, 'mangadex', $2, now(), now(), now() + interval '1 day', now(), now())
        ON CONFLICT (source, source_id) DO UPDATE SET
            volumes_last_checked_at = now(),
            volumes_last_success_at = now(),
            volumes_last_error = NULL
        "#,
    )
    .bind(manga_id)
    .bind(mangadex_id)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

fn primary_title(manga: &MangaDexManga) -> String {
    primary_title_and_language(manga).1
}

fn primary_language(manga: &MangaDexManga) -> String {
    primary_title_and_language(manga).0
}

fn primary_title_and_language(manga: &MangaDexManga) -> (String, String) {
    let mut titles = manga
        .attributes
        .title
        .iter()
        .filter_map(|(language, title)| {
            title
                .as_ref()
                .filter(|title| !title.trim().is_empty())
                .map(|title| (language.to_ascii_lowercase(), title.clone()))
        })
        .collect::<Vec<_>>();
    titles.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));

    for preferred in ["en", "ja-ro", "pt-br", "ja", "ko"] {
        if let Some((language, title)) = titles.iter().find(|(language, _)| language == preferred) {
            return (language.clone(), title.clone());
        }
    }

    titles
        .into_iter()
        .next()
        .unwrap_or_else(|| ("und".to_string(), "Titulo nao encontrado".to_string()))
}

fn slug_for(title: &str, mangadex_id: Uuid) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;

    for char in title.chars().flat_map(|char| char.to_lowercase()) {
        if char.is_ascii_alphanumeric() {
            slug.push(char);
            previous_dash = false;
        } else if !previous_dash && !slug.is_empty() {
            slug.push('-');
            previous_dash = true;
        }
    }

    while slug.ends_with('-') {
        slug.pop();
    }

    if slug.is_empty() {
        slug.push_str("manga");
    }

    let id = mangadex_id.to_string();
    slug.push('-');
    slug.push_str(&id[..8]);
    slug
}

fn normalized_volume_key(volume: &str) -> Option<String> {
    let number = volume.trim().parse::<f32>().ok()?;
    if !number.is_finite() {
        return None;
    }

    if number.fract() == 0.0 {
        Some(format!("{}", number as i32))
    } else {
        let mut value = format!("{number:.4}");
        while value.contains('.') && value.ends_with('0') {
            value.pop();
        }
        Some(value)
    }
}

fn is_fractional_volume_key(volume_key: Option<&str>) -> bool {
    volume_key
        .and_then(|volume| volume.parse::<f32>().ok())
        .is_none_or(|volume| volume.fract() != 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_volume_key_removes_trailing_zeroes() {
        assert_eq!(normalized_volume_key("1"), Some("1".to_string()));
        assert_eq!(normalized_volume_key("1.0"), Some("1".to_string()));
        assert_eq!(normalized_volume_key("01.500"), Some("1.5".to_string()));
        assert_eq!(normalized_volume_key("10.2500"), Some("10.25".to_string()));
    }

    #[test]
    fn normalized_volume_key_rejects_non_numeric_values() {
        assert_eq!(normalized_volume_key(""), None);
        assert_eq!(normalized_volume_key("special"), None);
        assert_eq!(normalized_volume_key("NaN"), None);
    }

    #[test]
    fn fractional_volume_detection_matches_special_edition_rule() {
        assert!(!is_fractional_volume_key(Some("1")));
        assert!(is_fractional_volume_key(Some("1.5")));
        assert!(is_fractional_volume_key(None));
        assert!(is_fractional_volume_key(Some("invalid")));
    }

    #[test]
    fn slug_contains_ascii_title_and_mangadex_prefix() {
        let id = Uuid::parse_str("391b0423-d847-456f-aff0-8b0cfc03066b").unwrap();
        assert_eq!(slug_for("One Piece!!", id), "one-piece-391b0423");
    }
}
