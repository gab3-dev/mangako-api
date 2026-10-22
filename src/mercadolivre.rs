use chrono::{DateTime, Utc};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::ApiError;
use crate::mercadolivre_oauth::MercadoLivreOAuth;

const DEFAULT_API_BASE_URL: &str = "https://api.mercadolibre.com/";

#[derive(Clone)]
pub struct MercadoLivreClient {
    http: Client,
    access_token: Option<String>,
    oauth: Option<MercadoLivreOAuth>,
    base_url: Url,
}

impl MercadoLivreClient {
    pub fn new(http: Client, access_token: Option<String>) -> Self {
        Self::with_base_url(http, access_token, DEFAULT_API_BASE_URL)
    }

    pub fn with_base_url(http: Client, access_token: Option<String>, base_url: &str) -> Self {
        Self {
            http,
            access_token: access_token
                .map(|token| token.trim().to_string())
                .filter(|token| !token.is_empty()),
            oauth: None,
            base_url: Url::parse(base_url).expect("valid Mercado Livre API base URL"),
        }
    }

    pub fn using_oauth(mut self, oauth: MercadoLivreOAuth) -> Self {
        self.oauth = Some(oauth);
        self
    }

    async fn authorized_get(&self, url: Url) -> Result<reqwest::Response, ApiError> {
        let token = if let Some(oauth) = &self.oauth {
            oauth.access_token().await?
        } else {
            self.access_token
                .clone()
                .ok_or(ApiError::MercadoLivreUnavailable)?
        };
        let response = self
            .http
            .get(url.clone())
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|_| ApiError::MercadoLivreUpstream)?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            && let Some(oauth) = &self.oauth
        {
            let refreshed = oauth.after_unauthorized(&token).await?;
            let response = self
                .http
                .get(url)
                .bearer_auth(&refreshed)
                .send()
                .await
                .map_err(|_| ApiError::MercadoLivreUpstream)?;
            if response.status() == reqwest::StatusCode::UNAUTHORIZED {
                oauth.mark_rejected(&refreshed).await?;
            }
            return Ok(response);
        }
        Ok(response)
    }

    pub async fn search_offers(
        &self,
        title: &str,
        volume: Option<f32>,
        locale: Option<&str>,
    ) -> Result<OffersResponse, ApiError> {
        let title = title.trim();
        if title.is_empty() || title.chars().count() > 180 {
            return Err(ApiError::BadRequest {
                message: "title must contain between 1 and 180 characters".to_string(),
            });
        }
        if volume.is_some_and(|number| !number.is_finite() || !(0.0..=9999.0).contains(&number)) {
            return Err(ApiError::BadRequest {
                message: "volume must be a number between 0 and 9999".to_string(),
            });
        }

        let query = build_search_query(title, volume, locale);
        let series_title = title.split(':').next().unwrap_or(title).trim();
        let mut url = self
            .base_url
            .join("products/search")
            .expect("valid Mercado Livre search URL");
        url.query_pairs_mut()
            .append_pair("status", "active")
            .append_pair("site_id", "MLB")
            .append_pair("domain_id", "MLB-BOOKS")
            .append_pair("q", series_title)
            .append_pair("limit", "50");

        let response = self.authorized_get(url).await?;
        if !response.status().is_success() {
            tracing::warn!(status = %response.status(), "Mercado Livre search returned an error");
            return Err(ApiError::MercadoLivreUpstream);
        }

        let result: ProductSearchResponse = response.json().await.map_err(|error| {
            tracing::warn!(error = %error, "Mercado Livre search response was invalid");
            ApiError::MercadoLivreUpstream
        })?;
        let mut products = result
            .results
            .into_iter()
            .filter(|product| product.is_manga_candidate())
            .filter(|product| product.matches_title(series_title))
            .filter(|product| volume.is_none_or(|number| product.matches_volume(number)))
            .collect::<Vec<_>>();
        products.sort_by_key(|product| std::cmp::Reverse(product.title_match_count(series_title)));

        let mut offers = Vec::new();
        for product in products.into_iter().take(6) {
            let items = self.get_product_items(&product.id).await?;
            offers.extend(
                items
                    .into_iter()
                    .filter_map(|item| Offer::from_catalog_item(&product, item)),
            );
        }
        offers.sort_by(|left, right| left.price.total_cmp(&right.price));
        let mut seen = std::collections::HashSet::new();
        offers.retain(|offer| seen.insert(offer.id.clone()));
        offers.truncate(20);

        Ok(OffersResponse {
            query,
            fetched_at: Utc::now(),
            volume_specific: volume.is_some(),
            offers,
        })
    }

    async fn get_product_items(&self, product_id: &str) -> Result<Vec<CatalogItem>, ApiError> {
        let url = self
            .base_url
            .join(&format!("products/{product_id}/items"))
            .expect("valid Mercado Livre product items URL");
        let response = self.authorized_get(url).await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        if !response.status().is_success() {
            tracing::warn!(status = %response.status(), product_id, "Mercado Livre product items returned an error");
            return Err(ApiError::MercadoLivreUpstream);
        }

        response
            .json::<ProductItemsResponse>()
            .await
            .map(|result| result.results)
            .map_err(|error| {
                tracing::warn!(error = %error, product_id, "Mercado Livre product items response was invalid");
                ApiError::MercadoLivreUpstream
            })
    }
}

fn build_search_query(title: &str, volume: Option<f32>, locale: Option<&str>) -> String {
    let title = title.trim();
    let mut query = format!("{title} mangá");
    if let Some(volume) = volume {
        let label = if locale.is_some_and(|value| value.to_ascii_lowercase().starts_with("es")) {
            "tomo"
        } else {
            "volume"
        };
        query.push_str(&format!(" {label} {}", format_volume_number(volume)));
    }
    query
}

fn format_volume_number(volume: f32) -> String {
    if volume.fract() == 0.0 {
        format!("{volume:.0}")
    } else {
        volume.to_string()
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OffersResponse {
    pub query: String,
    pub fetched_at: DateTime<Utc>,
    pub volume_specific: bool,
    pub offers: Vec<Offer>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Offer {
    pub id: String,
    pub title: String,
    pub price: f64,
    pub original_price: Option<f64>,
    pub discount_percent: Option<u8>,
    pub currency_id: String,
    pub permalink: String,
    pub thumbnail: Option<String>,
    pub seller_name: Option<String>,
    pub condition: Option<String>,
}

impl Offer {
    fn from_catalog_item(product: &CatalogProduct, item: CatalogItem) -> Option<Self> {
        if item.id.trim().is_empty()
            || product.name.trim().is_empty()
            || !item.price.is_finite()
            || item.price < 0.0
            || item.currency_id.trim().is_empty()
        {
            return None;
        }
        let original_price = item
            .original_price
            .filter(|value| value.is_finite() && *value > item.price);
        let discount_percent = original_price.map(|original| {
            (((original - item.price) / original) * 100.0)
                .round()
                .clamp(0.0, 100.0) as u8
        });

        Some(Self {
            id: item.id,
            title: product.name.clone(),
            price: item.price,
            original_price,
            discount_percent,
            currency_id: item.currency_id,
            permalink: format!("https://www.mercadolivre.com.br/p/{}", product.id),
            thumbnail: product.thumbnail(),
            seller_name: None,
            condition: item.condition,
        })
    }
}

#[derive(Deserialize)]
struct ProductSearchResponse {
    #[serde(default)]
    results: Vec<CatalogProduct>,
}

#[derive(Deserialize)]
struct CatalogProduct {
    id: String,
    name: String,
    domain_id: Option<String>,
    #[serde(default)]
    pictures: Vec<ProductPicture>,
}

impl CatalogProduct {
    fn thumbnail(&self) -> Option<String> {
        self.pictures.iter().find_map(|picture| {
            picture
                .url
                .strip_prefix("https://")
                .map(|_| picture.url.clone())
        })
    }

    fn matches_volume(&self, volume: f32) -> bool {
        let name = self.name.to_lowercase().replace("vol.", "vol ");
        let words = name
            .split(|character: char| !character.is_alphanumeric() && character != '.')
            .map(|word| word.trim_matches('.'))
            .filter(|word| !word.is_empty())
            .collect::<Vec<_>>();
        let marker = words
            .iter()
            .position(|word| matches!(*word, "vol" | "volume" | "tomo"));
        let numbers = &words[marker.map_or(0, |index| index + 1)..];
        numbers
            .iter()
            .find_map(|value| value.parse::<f32>().ok())
            .is_some_and(|number| (number - volume).abs() < f32::EPSILON)
    }

    fn is_manga_candidate(&self) -> bool {
        let name = self.name.to_lowercase();
        self.domain_id
            .as_deref()
            .is_none_or(|domain| domain == "MLB-BOOKS")
            && ![
                "guide", "art book", "artbook", "postcard", "sticker", "novel", "kit ", "box ",
            ]
            .iter()
            .any(|excluded| name.contains(excluded))
    }

    fn matches_title(&self, title: &str) -> bool {
        let required = title
            .split(|c: char| !c.is_alphanumeric())
            .filter(|word| {
                word.chars().count() >= 3
                    && !matches!(
                        word.to_lowercase().as_str(),
                        "the" | "and" | "manga" | "mangá"
                    )
            })
            .count();
        required > 0 && self.title_match_count(title) >= required
    }

    fn title_match_count(&self, title: &str) -> usize {
        let name = self.name.to_lowercase();
        let words = name
            .split(|c: char| !c.is_alphanumeric())
            .collect::<Vec<_>>();
        title
            .split(|character: char| !character.is_alphanumeric())
            .filter(|word| {
                word.chars().count() >= 3
                    && !matches!(
                        word.to_lowercase().as_str(),
                        "the" | "and" | "manga" | "mangá"
                    )
            })
            .filter(|word| words.contains(&word.to_lowercase().as_str()))
            .count()
    }
}

#[derive(Deserialize)]
struct ProductPicture {
    url: String,
}

#[derive(Deserialize)]
struct ProductItemsResponse {
    #[serde(default)]
    results: Vec<CatalogItem>,
}

#[derive(Deserialize)]
struct CatalogItem {
    #[serde(rename = "item_id")]
    id: String,
    price: f64,
    original_price: Option<f64>,
    currency_id: String,
    condition: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{build_search_query, format_volume_number};

    #[test]
    fn search_query_uses_manga_title_volume_and_locale() {
        assert_eq!(
            build_search_query(" One Piece ", Some(1.5), Some("pt-br")),
            "One Piece mangá volume 1.5"
        );
        assert_eq!(
            build_search_query("Berserk", Some(2.0), Some("es")),
            "Berserk mangá tomo 2"
        );
        assert_eq!(build_search_query("Naruto", None, None), "Naruto mangá");
    }

    #[test]
    fn volume_number_omits_unneeded_decimal_places() {
        assert_eq!(format_volume_number(4.0), "4");
        assert_eq!(format_volume_number(4.5), "4.5");
    }

    #[test]
    fn catalog_product_matches_exact_volume_number() {
        let product = super::CatalogProduct {
            id: "MLB123".to_string(),
            name: "Frieren - Vol. 15".to_string(),
            domain_id: Some("MLB-BOOKS".to_string()),
            pictures: vec![],
        };

        assert!(product.matches_volume(15.0));
        assert!(!product.matches_volume(1.0));
        assert_eq!(product.title_match_count("Frieren e a jornada"), 1);
    }

    #[test]
    fn matching_rejects_other_series_and_companion_books() {
        let mut product = super::CatalogProduct {
            id: "MLB123".to_string(),
            name: "Dandadan Volume 15".to_string(),
            domain_id: Some("MLB-BOOKS".to_string()),
            pictures: vec![],
        };
        assert!(!product.matches_title("Frieren"));
        product.name = "Frieren e a jornada para o além - 01 - Panini 2022".to_string();
        assert!(product.matches_title("Frieren"));
        assert!(product.matches_volume(1.0));
        assert!(!product.matches_volume(2022.0));
        product.name = "Frieren Art Book Vol. 1".to_string();
        assert!(!product.is_manga_candidate());
        product.name = "Kit Frieren Vol. 1 e 2".to_string();
        assert!(!product.is_manga_candidate());
    }
}
