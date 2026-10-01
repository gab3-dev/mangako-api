use std::{path::PathBuf, time::Duration};

use chrono::{DateTime, Utc};
use image::ImageFormat;
use reqwest::{StatusCode, Url, header::RETRY_AFTER};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
use tokio::{fs, io::AsyncWriteExt, time::sleep};
use uuid::Uuid;

use crate::error::ApiError;

const ALLOWED_MANGADEX_HOST: &str = "uploads.mangadex.org";
const COVER_DOWNLOAD_LOCK_KEY: i64 = 8_867_291;
const COVER_DOWNLOAD_INTERVAL: Duration = Duration::from_secs(1);
const COVER_RETRY_BASE_DELAY: Duration = Duration::from_secs(30);
const COVER_RETRY_MAX_DELAY: Duration = Duration::from_secs(60 * 60);
const COVER_THROTTLE_DELAY: Duration = Duration::from_secs(30 * 60);

#[derive(Clone)]
pub struct CoverStorage {
    root: PathBuf,
    public_base_url: String,
    max_bytes: usize,
}

impl CoverStorage {
    pub fn new(root: PathBuf, public_base_url: String, max_bytes: usize) -> Self {
        Self {
            root,
            public_base_url: public_base_url.trim_end_matches('/').to_string(),
            max_bytes,
        }
    }

    pub fn public_url(&self, key: &str) -> String {
        format!("{}/{key}", self.public_base_url)
    }

    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    pub async fn ensure_root(&self) -> Result<(), std::io::Error> {
        fs::create_dir_all(&self.root).await
    }

    pub async fn store_upload(&self, bytes: &[u8]) -> Result<StoredImage, ApiError> {
        if bytes.is_empty() || bytes.len() > self.max_bytes {
            return Err(ApiError::BadRequest {
                message: format!("cover file must be between 1 and {} bytes", self.max_bytes),
            });
        }
        let format = image::guess_format(bytes).map_err(|_| ApiError::BadRequest {
            message: "cover file must be JPEG, PNG, or WebP".to_string(),
        })?;
        let (extension, content_type) = match format {
            ImageFormat::Jpeg => ("jpg", "image/jpeg"),
            ImageFormat::Png => ("png", "image/png"),
            ImageFormat::WebP => ("webp", "image/webp"),
            _ => {
                return Err(ApiError::BadRequest {
                    message: "cover file must be JPEG, PNG, or WebP".to_string(),
                });
            }
        };
        let image = image::load_from_memory_with_format(bytes, format).map_err(|_| {
            ApiError::BadRequest {
                message: "cover file is not a valid image".to_string(),
            }
        })?;
        let (width, height) = (image.width() as i32, image.height() as i32);
        if width == 0 || height == 0 || width > 10_000 || height > 10_000 {
            return Err(ApiError::BadRequest {
                message: "cover image dimensions are invalid".to_string(),
            });
        }
        let hash = format!("{:x}", Sha256::digest(bytes));
        let key = format!("covers/v1/{hash}.{extension}");
        let path = self.root.join(&key);
        if fs::metadata(&path).await.is_err() {
            let parent = path.parent().expect("cover key has parent");
            fs::create_dir_all(parent).await.map_err(ApiError::from)?;
            let temp = parent.join(format!(".{}.tmp", Uuid::new_v4()));
            let mut file = fs::File::create(&temp).await.map_err(ApiError::from)?;
            file.write_all(bytes).await.map_err(ApiError::from)?;
            file.sync_all().await.map_err(ApiError::from)?;
            fs::rename(temp, path).await.map_err(ApiError::from)?;
        }
        let thumbnail = image.thumbnail(512, 512).to_rgb8();
        let mut thumbnail_bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut thumbnail_bytes, 82)
            .encode_image(&thumbnail)
            .map_err(|_| ApiError::BadRequest {
                message: "cover thumbnail could not be encoded".to_string(),
            })?;
        let thumbnail_hash = format!("{:x}", Sha256::digest(&thumbnail_bytes));
        let thumbnail_key = format!("covers/v1/{thumbnail_hash}.512.jpg");
        let thumbnail_path = self.root.join(&thumbnail_key);
        if fs::metadata(&thumbnail_path).await.is_err() {
            let parent = thumbnail_path.parent().expect("thumbnail key has parent");
            fs::create_dir_all(parent).await.map_err(ApiError::from)?;
            fs::write(&thumbnail_path, &thumbnail_bytes)
                .await
                .map_err(ApiError::from)?;
        }
        Ok(StoredImage {
            storage_key: key,
            content_type: content_type.to_string(),
            byte_size: bytes.len() as i64,
            width,
            height,
            sha256: hash,
            thumbnail_storage_key: thumbnail_key,
        })
    }
}

pub struct StoredImage {
    pub storage_key: String,
    pub content_type: String,
    pub byte_size: i64,
    pub width: i32,
    pub height: i32,
    pub sha256: String,
    pub thumbnail_storage_key: String,
}

pub async fn record_upload(pool: &PgPool, image: &StoredImage) -> Result<Uuid, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO cover_assets (storage_key, thumbnail_storage_key, status, content_type, byte_size, width, height, sha256) VALUES ($1, $2, 'ready', $3, $4, $5, $6, $7) ON CONFLICT (storage_key) DO UPDATE SET thumbnail_storage_key=EXCLUDED.thumbnail_storage_key, status='ready' RETURNING id",
    )
    .bind(&image.storage_key).bind(&image.thumbnail_storage_key).bind(&image.content_type).bind(image.byte_size).bind(image.width).bind(image.height).bind(&image.sha256).fetch_one(pool).await
}

pub async fn enqueue_source_asset(
    tx: &mut Transaction<'_, Postgres>,
    source_url: &str,
) -> Result<Uuid, sqlx::Error> {
    sqlx::query_scalar(
        r#"INSERT INTO cover_assets (source_url) VALUES ($1)
           ON CONFLICT (source_url) DO UPDATE SET next_attempt_at = LEAST(cover_assets.next_attempt_at, now())
           RETURNING id"#,
    ).bind(source_url).fetch_one(&mut **tx).await
}

pub async fn enqueue_existing_mangadex_assets(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO cover_assets (source_url) SELECT DISTINCT source_url FROM manga_covers WHERE asset_id IS NULL AND source_url LIKE 'https://uploads.mangadex.org/%' ON CONFLICT (source_url) DO NOTHING",
    ).execute(pool).await?;
    sqlx::query(
        "INSERT INTO cover_assets (source_url) SELECT DISTINCT source_url FROM manga_volumes WHERE asset_id IS NULL AND source_url LIKE 'https://uploads.mangadex.org/%' ON CONFLICT (source_url) DO NOTHING",
    ).execute(pool).await?;
    sqlx::query("UPDATE manga_covers AS cover SET asset_id = asset.id FROM cover_assets AS asset WHERE cover.asset_id IS NULL AND cover.source_url = asset.source_url")
        .execute(pool).await?;
    sqlx::query("UPDATE manga_volumes AS volume SET asset_id = asset.id FROM cover_assets AS asset WHERE volume.asset_id IS NULL AND volume.source_url = asset.source_url")
        .execute(pool).await?;
    Ok(())
}

pub async fn process_next(
    pool: &PgPool,
    storage: &CoverStorage,
    client: &reqwest::Client,
) -> Result<bool, sqlx::Error> {
    let mut connection = pool.acquire().await?;
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(COVER_DOWNLOAD_LOCK_KEY)
        .fetch_one(&mut *connection)
        .await?;
    if !locked {
        return Ok(false);
    }

    let result = process_next_locked(&mut connection, storage, client).await;
    let unlock = sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock($1)")
        .bind(COVER_DOWNLOAD_LOCK_KEY)
        .fetch_one(&mut *connection)
        .await;
    unlock?;
    result
}

async fn process_next_locked(
    connection: &mut PgConnection,
    storage: &CoverStorage,
    client: &reqwest::Client,
) -> Result<bool, sqlx::Error> {
    let asset = sqlx::query_as::<_, PendingAsset>(
        r#"UPDATE cover_assets SET locked_until = now() + interval '2 minutes', attempt_count = attempt_count + 1
           WHERE id = (SELECT id FROM cover_assets WHERE source_url IS NOT NULL AND status IN ('pending', 'failed')
             AND next_attempt_at <= now() AND (locked_until IS NULL OR locked_until < now()) ORDER BY next_attempt_at LIMIT 1 FOR UPDATE SKIP LOCKED)
           RETURNING id, source_url, attempt_count"#,
    )
    .fetch_optional(&mut *connection)
    .await?;
    let Some(asset) = asset else { return Ok(false) };
    wait_for_download_slot(connection).await?;
    let outcome = mirror_asset(storage, client, &asset.source_url).await;
    match outcome {
        Ok(image) => {
            sqlx::query("UPDATE cover_assets SET storage_key=$2, thumbnail_storage_key=$3, status='ready', content_type=$4, byte_size=$5, width=$6, height=$7, sha256=$8, locked_until=NULL, last_error=NULL WHERE id=$1")
                .bind(asset.id).bind(image.storage_key).bind(image.thumbnail_storage_key).bind(image.content_type).bind(image.byte_size).bind(image.width).bind(image.height).bind(image.sha256).execute(&mut *connection).await?;
        }
        Err(error) => {
            let delay = error.retry_delay(asset.attempt_count);
            sqlx::query("UPDATE cover_assets SET status='failed', locked_until=NULL, next_attempt_at=now() + $2::bigint * interval '1 second', last_error=$3 WHERE id=$1")
                .bind(asset.id).bind(delay.as_secs() as i64).bind(error.message).execute(&mut *connection).await?;
        }
    }
    Ok(true)
}

async fn wait_for_download_slot(connection: &mut PgConnection) -> Result<(), sqlx::Error> {
    let scheduled_at = sqlx::query_scalar::<_, DateTime<Utc>>(
        "UPDATE cover_download_schedule SET next_download_at=GREATEST(next_download_at, now()) + $1::bigint * interval '1 second' WHERE id RETURNING next_download_at - $1::bigint * interval '1 second'",
    )
    .bind(COVER_DOWNLOAD_INTERVAL.as_secs() as i64)
    .fetch_one(&mut *connection)
    .await?;
    if let Ok(delay) = (scheduled_at - Utc::now()).to_std() {
        sleep(delay).await;
    }
    Ok(())
}

async fn mirror_asset(
    storage: &CoverStorage,
    client: &reqwest::Client,
    source_url: &str,
) -> Result<StoredImage, MirrorError> {
    let url = Url::parse(source_url).map_err(|_| MirrorError::new("invalid source URL"))?;
    if url.scheme() != "https" || url.host_str() != Some(ALLOWED_MANGADEX_HOST) {
        return Err(MirrorError::new("source host is not allowed"));
    }
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| MirrorError::new("cover download failed"))?;
    if !response.status().is_success() {
        return Err(MirrorError {
            message: "cover download returned an error".to_string(),
            retry_after: retry_after(response.headers().get(RETRY_AFTER)),
            throttled: matches!(
                response.status(),
                StatusCode::TOO_MANY_REQUESTS | StatusCode::FORBIDDEN
            ),
        });
    }
    if response
        .content_length()
        .is_some_and(|length| length > storage.max_bytes as u64)
    {
        return Err(MirrorError::new("cover download exceeds size limit"));
    }
    let mut body = Vec::new();
    let mut response = response;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| MirrorError::new("cover download failed"))?
    {
        if body.len().saturating_add(chunk.len()) > storage.max_bytes {
            return Err(MirrorError::new("cover download exceeds size limit"));
        }
        body.extend_from_slice(&chunk);
    }
    storage
        .store_upload(&body)
        .await
        .map_err(|error| MirrorError::new(error.to_string()))
}

struct MirrorError {
    message: String,
    retry_after: Option<Duration>,
    throttled: bool,
}

impl MirrorError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retry_after: None,
            throttled: false,
        }
    }

    fn retry_delay(&self, attempt_count: i32) -> Duration {
        let retry_delay = COVER_RETRY_BASE_DELAY
            .checked_mul(1_u32 << attempt_count.saturating_sub(1).clamp(0, 7))
            .unwrap_or(COVER_RETRY_MAX_DELAY)
            .min(COVER_RETRY_MAX_DELAY);
        let minimum_delay = if self.throttled {
            COVER_THROTTLE_DELAY
        } else {
            retry_delay
        };
        self.retry_after.unwrap_or_default().max(minimum_delay)
    }
}

fn retry_after(value: Option<&reqwest::header::HeaderValue>) -> Option<Duration> {
    value
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
}

#[derive(sqlx::FromRow)]
struct PendingAsset {
    id: Uuid,
    source_url: String,
    attempt_count: i32,
}

pub fn ready_image_url(
    base: Option<&CoverStorage>,
    storage_key: Option<&str>,
    status: Option<&str>,
) -> Option<String> {
    if status == Some("ready") {
        Some(base?.public_url(storage_key?))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_download_retries_back_off_to_one_hour() {
        let error = MirrorError::new("temporary failure");
        assert_eq!(error.retry_delay(1), Duration::from_secs(30));
        assert_eq!(error.retry_delay(8), Duration::from_secs(60 * 60));
    }

    #[test]
    fn throttled_download_waits_at_least_thirty_minutes() {
        let error = MirrorError {
            message: "throttled".to_string(),
            retry_after: Some(Duration::from_secs(45 * 60)),
            throttled: true,
        };
        assert_eq!(error.retry_delay(1), Duration::from_secs(45 * 60));
    }
}
