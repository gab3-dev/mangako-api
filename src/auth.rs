use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, Request},
    middleware::Next,
    response::Response,
};

use crate::error::ApiError;

pub async fn require_api_token(
    State(api_token): State<String>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Result<Response, ApiError> {
    let Some(header) = headers.get(axum::http::header::AUTHORIZATION) else {
        return Err(ApiError::Unauthorized);
    };
    let Ok(value) = header.to_str() else {
        return Err(ApiError::Unauthorized);
    };
    let Some(token) = value.strip_prefix("Bearer ") else {
        return Err(ApiError::Unauthorized);
    };

    if token != api_token {
        return Err(ApiError::Unauthorized);
    }

    Ok(next.run(request).await)
}
