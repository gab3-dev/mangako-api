use std::collections::HashMap;
use std::future::Future;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

pub trait MangaDexApi: Clone + Send + Sync + 'static {
    fn get_manga(
        &self,
        id: Uuid,
    ) -> impl Future<Output = Result<Option<MangaDexManga>, reqwest::Error>> + Send;

    fn search_mangas(
        &self,
        title: Option<&str>,
        offset: u32,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<MangaDexManga>, reqwest::Error>> + Send;

    fn list_covers(
        &self,
        manga_id: Uuid,
        offset: u32,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<MangaDexCover>, reqwest::Error>> + Send;

    fn latest_volume_number(
        &self,
        manga_id: Uuid,
        language: &str,
    ) -> impl Future<Output = Result<Option<String>, reqwest::Error>> + Send;
}

#[derive(Clone)]
pub struct MangaDexClient {
    http: reqwest::Client,
    base_url: String,
}

impl MangaDexClient {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            base_url: "https://api.mangadex.org".to_string(),
        }
    }
}

impl MangaDexApi for MangaDexClient {
    async fn get_manga(&self, id: Uuid) -> Result<Option<MangaDexManga>, reqwest::Error> {
        let response = self
            .http
            .get(format!("{}/manga/{id}", self.base_url))
            .query(&[
                ("includes[]", "cover_art"),
                ("includes[]", "author"),
                ("includes[]", "artist"),
            ])
            .send()
            .await?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        Ok(Some(
            response
                .error_for_status()?
                .json::<MangaResponse>()
                .await?
                .data,
        ))
    }

    async fn search_mangas(
        &self,
        title: Option<&str>,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<MangaDexManga>, reqwest::Error> {
        let query = manga_search_query(title, offset, limit);

        let response = self
            .http
            .get(format!("{}/manga", self.base_url))
            .query(&query)
            .send()
            .await?;

        let body = response
            .error_for_status()?
            .json::<MangaListResponse>()
            .await?;
        Ok(body.data)
    }

    async fn list_covers(
        &self,
        manga_id: Uuid,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<MangaDexCover>, reqwest::Error> {
        let response = self
            .http
            .get(format!("{}/cover", self.base_url))
            .query(&[
                ("limit", limit.to_string()),
                ("offset", offset.to_string()),
                ("manga[]", manga_id.to_string()),
                ("order[volume]", "asc".to_string()),
            ])
            .send()
            .await?;

        Ok(response
            .error_for_status()?
            .json::<CoverListResponse>()
            .await?
            .data)
    }

    async fn latest_volume_number(
        &self,
        manga_id: Uuid,
        language: &str,
    ) -> Result<Option<String>, reqwest::Error> {
        let response = self
            .http
            .get(format!("{}/cover", self.base_url))
            .query(&latest_volume_query(manga_id))
            .send()
            .await?;

        Ok(response
            .error_for_status()?
            .json::<CoverListResponse>()
            .await?
            .data
            .into_iter()
            .find_map(|cover| {
                let volume = cover.attributes.volume?;
                (matches_language(cover.attributes.locale.as_deref(), language)
                    && is_regular_volume(&volume))
                .then_some(volume)
            }))
    }
}

#[derive(Debug, Deserialize)]
struct MangaResponse {
    data: MangaDexManga,
}

#[derive(Debug, Deserialize)]
struct MangaListResponse {
    data: Vec<MangaDexManga>,
}

#[derive(Debug, Deserialize)]
struct CoverListResponse {
    data: Vec<MangaDexCover>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MangaDexCover {
    pub id: Uuid,
    pub attributes: MangaDexCoverAttributes,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MangaDexCoverAttributes {
    pub file_name: String,
    pub volume: Option<String>,
    pub locale: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub version: Option<i32>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MangaDexManga {
    pub id: Uuid,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub attributes: MangaDexAttributes,
    #[serde(default)]
    pub relationships: Vec<MangaDexRelationship>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MangaDexAttributes {
    #[serde(default)]
    pub title: HashMap<String, Option<String>>,
    #[serde(default)]
    pub alt_titles: Vec<HashMap<String, Option<String>>>,
    #[serde(default)]
    pub description: HashMap<String, Option<String>>,
    pub original_language: Option<String>,
    pub publication_demographic: Option<String>,
    pub status: Option<String>,
    pub year: Option<i32>,
    pub content_rating: Option<String>,
    pub last_volume: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
    pub version: Option<i32>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MangaDexRelationship {
    pub id: Option<Uuid>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub attributes: Option<MangaDexRelationshipAttributes>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MangaDexRelationshipAttributes {
    pub name: Option<String>,
    pub image_url: Option<String>,
    pub file_name: Option<String>,
    pub locale: Option<String>,
    pub volume: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
    pub version: Option<i32>,
}

impl MangaDexManga {
    pub fn cover_art(&self) -> Option<&MangaDexRelationship> {
        self.relationships
            .iter()
            .find(|relationship| relationship.kind.as_deref() == Some("cover_art"))
    }
}

pub fn cover_url(manga_id: Uuid, file_name: &str) -> String {
    format!("https://uploads.mangadex.org/covers/{manga_id}/{file_name}.512.jpg")
}

fn manga_search_query(title: Option<&str>, offset: u32, limit: u32) -> Vec<(&str, String)> {
    let mut query = vec![
        ("limit", limit.to_string()),
        ("offset", offset.to_string()),
        ("includes[]", "cover_art".to_string()),
        ("includes[]", "author".to_string()),
        ("includes[]", "artist".to_string()),
    ];
    if let Some(title) = title.filter(|title| !title.is_empty()) {
        query.push(("title", title.to_string()));
    } else {
        query.push(("order[followedCount]", "desc".to_string()));
    }
    query
}

fn latest_volume_query(manga_id: Uuid) -> Vec<(&'static str, String)> {
    vec![
        ("limit", "100".to_string()),
        ("manga[]", manga_id.to_string()),
        ("order[volume]", "desc".to_string()),
    ]
}

fn matches_language(locale: Option<&str>, language: &str) -> bool {
    locale
        .map(|locale| locale.replace('_', "-").to_ascii_lowercase())
        .and_then(|locale| locale.split('-').next().map(str::to_string))
        .is_some_and(|locale| locale == language)
}

fn is_regular_volume(volume: &str) -> bool {
    volume
        .parse::<f64>()
        .is_ok_and(|number| number.is_finite() && number.fract() == 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_search_sends_title_and_pagination() {
        let query = manga_search_query(Some("Frieren"), 12, 6);

        assert!(query.contains(&("title", "Frieren".to_string())));
        assert!(query.contains(&("offset", "12".to_string())));
        assert!(query.contains(&("limit", "6".to_string())));
        assert!(!query.iter().any(|(key, _)| *key == "order[followedCount]"));
    }

    #[test]
    fn empty_search_orders_by_followed_count() {
        let query = manga_search_query(None, 0, 10);

        assert!(query.contains(&("order[followedCount]", "desc".to_string())));
        assert!(!query.iter().any(|(key, _)| *key == "title"));
    }

    #[test]
    fn latest_volume_query_requests_highest_volume_without_locale_filtering() {
        let manga_id = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
        let query = latest_volume_query(manga_id);

        assert!(query.contains(&("limit", "100".to_string())));
        assert!(query.contains(&("manga[]", manga_id.to_string())));
        assert!(!query.iter().any(|(key, _)| *key == "locales[]"));
        assert!(query.contains(&("order[volume]", "desc".to_string())));
    }
}
