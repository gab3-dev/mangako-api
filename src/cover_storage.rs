use std::{path::PathBuf, time::Duration};

use image::ImageFormat;
use reqwest::Url;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use tokio::{fs, io::AsyncWriteExt};
use uuid::Uuid;

use crate::error::ApiError;

const ALLOWED_MANGADEX_HOST: &str = "uploads.mangadex.org";

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
    let asset = sqlx::query_as::<_, PendingAsset>(
        r#"UPDATE cover_assets SET locked_until = now() + interval '2 minutes', attempt_count = attempt_count + 1
           WHERE id = (SELECT id FROM cover_assets WHERE source_url IS NOT NULL AND status IN ('pending', 'failed')
             AND next_attempt_at <= now() AND (locked_until IS NULL OR locked_until < now()) ORDER BY next_attempt_at LIMIT 1 FOR UPDATE SKIP LOCKED)
           RETURNING id, source_url, attempt_count"#,
    ).fetch_optional(pool).await?;
    let Some(asset) = asset else { return Ok(false) };
    let outcome = mirror_asset(storage, client, &asset.source_url).await;
    match outcome {
        Ok(image) => {
            sqlx::query("UPDATE cover_assets SET storage_key=$2, thumbnail_storage_key=$3, status='ready', content_type=$4, byte_size=$5, width=$6, height=$7, sha256=$8, locked_until=NULL, last_error=NULL WHERE id=$1")
                .bind(asset.id).bind(image.storage_key).bind(image.thumbnail_storage_key).bind(image.content_type).bind(image.byte_size).bind(image.width).bind(image.height).bind(image.sha256).execute(pool).await?;
        }
        Err(error) => {
            let delay =
                Duration::from_secs(2_u64.saturating_pow(asset.attempt_count.min(8) as u32));
            sqlx::query("UPDATE cover_assets SET status='failed', locked_until=NULL, next_attempt_at=now() + $2::bigint * interval '1 second', last_error=$3 WHERE id=$1")
                .bind(asset.id).bind(delay.as_secs() as i64).bind(error).execute(pool).await?;
        }
    }
    Ok(true)
}

async fn mirror_asset(
    storage: &CoverStorage,
    client: &reqwest::Client,
    source_url: &str,
) -> Result<StoredImage, String> {
    let url = Url::parse(source_url).map_err(|_| "invalid source URL".to_string())?;
    if url.scheme() != "https" || url.host_str() != Some(ALLOWED_MANGADEX_HOST) {
        return Err("source host is not allowed".to_string());
    }
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| "cover download failed".to_string())?
        .error_for_status()
        .map_err(|_| "cover download returned an error".to_string())?;
    if response
        .content_length()
        .is_some_and(|length| length > storage.max_bytes as u64)
    {
        return Err("cover download exceeds size limit".to_string());
    }
    let mut body = Vec::new();
    let mut response = response;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "cover download failed".to_string())?
    {
        if body.len().saturating_add(chunk.len()) > storage.max_bytes {
            return Err("cover download exceeds size limit".to_string());
        }
        body.extend_from_slice(&chunk);
    }
    storage
        .store_upload(&body)
        .await
        .map_err(|error| error.to_string())
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
