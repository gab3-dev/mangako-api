use axum::{
    Json,
    extract::{Query, State},
};
use serde::Deserialize;
use utoipa::IntoParams;

use crate::{api::AppState, error::ApiError, mercadolivre::OffersResponse};

#[derive(Debug, Deserialize, IntoParams)]
pub struct OfferSearchQuery {
    pub title: String,
    pub volume: Option<f32>,
    pub locale: Option<String>,
    /// Optional catalog reference used to resolve localized display titles to a series name.
    #[serde(rename = "mangaRef")]
    pub manga_ref: Option<String>,
}

#[utoipa::path(
    get,
    path = "/shopping/mercado-livre/offers",
    params(OfferSearchQuery),
    responses(
        (status = 200, description = "Mercado Livre offers for a manga volume", body = OffersResponse),
        (status = 400, description = "Invalid offer search query", body = crate::error::ErrorResponse),
        (status = 502, description = "Mercado Livre is temporarily unavailable", body = crate::error::ErrorResponse),
        (status = 503, description = "Mercado Livre access is not configured", body = crate::error::ErrorResponse)
    ),
    security(("read_token" = [])),
    tag = "shopping"
)]
pub async fn search_mercadolivre_offers(
    State(state): State<AppState>,
    Query(query): Query<OfferSearchQuery>,
) -> Result<Json<OffersResponse>, ApiError> {
    let client = state
        .mercadolivre_client
        .as_ref()
        .ok_or(ApiError::MercadoLivreUnavailable)?;
    let title = if let Some(reference) = query.manga_ref.as_deref() {
        if reference.is_empty() || reference.len() > 128 || reference.chars().any(char::is_control)
        {
            return Err(ApiError::BadRequest {
                message: "mangaRef is invalid".to_string(),
            });
        }
        let manga = crate::manga::load_manga_response(
            &state.manga_service.pool_for_assets(),
            reference,
            None,
        )
        .await?;
        manga
            .map(|manga| {
                manga
                    .localizations
                    .iter()
                    .find(|entry| {
                        entry.language == "en"
                            && entry
                                .title
                                .as_ref()
                                .is_some_and(|title| !title.trim().is_empty())
                    })
                    .and_then(|entry| entry.title.clone())
                    .unwrap_or(manga.primary_title)
            })
            .unwrap_or(query.title)
    } else {
        query.title
    };
    client
        .search_offers(&title, query.volume, query.locale.as_deref())
        .await
        .map(Json)
}
