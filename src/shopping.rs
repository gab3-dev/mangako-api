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
    client
        .search_offers(&query.title, query.volume, query.locale.as_deref())
        .await
        .map(Json)
}
