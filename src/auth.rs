use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, Request},
    middleware::Next,
    response::Response,
};

use crate::error::ApiError;

#[derive(Clone)]
pub struct AuthConfig {
    pub read_token: String,
    pub write_token: String,
}

pub async fn require_read_token(
    State(auth): State<AuthConfig>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Result<Response, ApiError> {
    authorize(&headers, &auth.read_token)?;
    Ok(next.run(request).await)
}

pub async fn require_read_or_write_token_for_refresh(
    State(auth): State<AuthConfig>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Result<Response, ApiError> {
    let expected = if request
        .uri()
        .query()
        .is_some_and(|query| query.split('&').any(|item| item == "refresh=true"))
    {
        &auth.write_token
    } else {
        &auth.read_token
    };
    authorize(&headers, expected)?;
    Ok(next.run(request).await)
}

pub async fn require_write_token(
    State(auth): State<AuthConfig>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Result<Response, ApiError> {
    authorize(&headers, &auth.write_token)?;
    Ok(next.run(request).await)
}

fn authorize(headers: &HeaderMap, expected: &str) -> Result<(), ApiError> {
    let header = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|header| header.to_str().ok())
        .ok_or(ApiError::Unauthorized)?;
    let token = header
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
        .ok_or(ApiError::Unauthorized)?;

    constant_time_eq(token.as_bytes(), expected.as_bytes())
        .then_some(())
        .ok_or(ApiError::Unauthorized)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    for index in 0..left.len().max(right.len()) {
        difference |= usize::from(*left.get(index).unwrap_or(&0) ^ *right.get(index).unwrap_or(&0));
    }
    difference == 0
}
