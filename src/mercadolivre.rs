use chrono::{DateTime, Utc};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::ApiError;

const DEFAULT_API_BASE_URL: &str = "https://api.mercadolibre.com/";

#[derive(Clone)]
pub struct MercadoLivreClient {
    http: Client,
    access_token: Option<String>,
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
            base_url: Url::parse(base_url).expect("valid Mercado Livre API base URL"),
        }
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
        if volume.is_some_and(|number| !number.is_finite() || number < 0.0 || number > 9999.0) {
            return Err(ApiError::BadRequest {
                message: "volume must be a number between 0 and 9999".to_string(),
            });
        }

        let token = self
            .access_token
            .as_deref()
            .ok_or(ApiError::MercadoLivreUnavailable)?;
        let query = build_search_query(title, volume, locale);
        let mut url = self
            .base_url
            .join("products/search")
            .expect("valid Mercado Livre search URL");
        url.query_pairs_mut()
            .append_pair("status", "active")
            .append_pair("site_id", "MLB")
            .append_pair("q", &query)
            .append_pair("limit", "10");

        let response = self
            .http
            .get(url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|error| {
                tracing::warn!(error = %error, "Mercado Livre search request failed");
                ApiError::MercadoLivreUpstream
            })?;
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
            .filter(|product| volume.is_none_or(|number| product.matches_volume(number)))
            .collect::<Vec<_>>();
        if products
            .iter()
            .any(|product| product.title_match_count(title) > 0)
        {
            products.retain(|product| product.title_match_count(title) > 0);
        }
        products.sort_by_key(|product| std::cmp::Reverse(product.title_match_count(title)));

        let mut offers = Vec::new();
        for product in products {
            if offers.len() >= 20 {
                break;
            }
            let items = self.get_product_items(token, &product.id).await?;
            offers.extend(
                items
                    .into_iter()
                    .filter_map(|item| Offer::from_catalog_item(&product, item))
                    .take(20 - offers.len()),
            );
        }
        offers.sort_by(|left, right| left.price.total_cmp(&right.price));

        Ok(OffersResponse {
            query,
            fetched_at: Utc::now(),
            volume_specific: volume.is_some(),
            offers,
        })
    }

    async fn get_product_items(
        &self,
        token: &str,
        product_id: &str,
    ) -> Result<Vec<CatalogItem>, ApiError> {
        let url = self
            .base_url
            .join(&format!("products/{product_id}/items"))
            .expect("valid Mercado Livre product items URL");
        let response = self
            .http
            .get(url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|error| {
                tracing::warn!(error = %error, product_id, "Mercado Livre product items request failed");
                ApiError::MercadoLivreUpstream
            })?;
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
        self.name
            .split(|character: char| !character.is_ascii_digit() && character != '.')
            .filter_map(|value| value.parse::<f32>().ok())
            .any(|value| (value - volume).abs() < f32::EPSILON)
    }

    fn title_match_count(&self, title: &str) -> usize {
        let name = self.name.to_lowercase();
        title
            .split(|character: char| !character.is_alphanumeric())
            .filter(|word| word.chars().count() >= 3)
            .filter(|word| name.contains(&word.to_lowercase()))
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
            pictures: vec![],
        };

        assert!(product.matches_volume(15.0));
        assert!(!product.matches_volume(1.0));
        assert_eq!(product.title_match_count("Frieren e a jornada"), 1);
    }
}
