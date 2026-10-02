use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

fn json_request(method: Method, uri: String, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, "Bearer manual-write-token")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

async fn response_json(response: axum::response::Response) -> Value {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn manual_manga_can_include_metadata_covers_and_volumes() {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://mangako:mangako@localhost:5432/mangako_api".to_string());
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("PostgreSQL must be available for integration tests");
    mangako_api::db::migrate(&pool).await.unwrap();
    sqlx::query("DELETE FROM mangas WHERE slug = 'manual-catalog-test'")
        .execute(&pool)
        .await
        .unwrap();

    let app = mangako_api::api::router_with_api_tokens(
        pool.clone(),
        "manual-read-token".to_string(),
        "manual-write-token".to_string(),
    );
    let created = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/mangas".to_string(),
            json!({
                "slug": "manual-catalog-test",
                "primaryTitle": "Manual Catalog",
                "originalLanguage": "en",
                "year": 2026,
                "localizations": [{
                    "language": "pt-BR",
                    "title": "Catalogo Manual",
                    "description": "Criado fora do MangaDex",
                    "isPrimary": true
                }],
                "aliases": [{ "language": "en", "title": "Manual alias" }],
                "covers": [{
                    "sourceUrl": "https://example.com/manual-cover.jpg",
                    "locale": "en",
                    "isPrimary": true
                }]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = response_json(created).await;
    assert!(created["mangadexId"].is_null());
    assert_eq!(created["localizations"][0]["language"], "pt-br");
    assert_eq!(created["covers"][0]["isPrimary"], true);
    let manga_id = created["id"].as_str().unwrap();

    let search = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/mangas?title=Manual%20Catalog")
                .header(header::AUTHORIZATION, "Bearer manual-read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(search.status(), StatusCode::OK);
    let search = response_json(search).await;
    assert_eq!(search[0]["id"], manga_id);

    let cover = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            format!("/mangas/{manga_id}/covers"),
            json!({
                "sourceUrl": "https://example.com/replacement-cover.jpg",
                "isPrimary": true
            }),
        ))
        .await
        .unwrap();
    assert_eq!(cover.status(), StatusCode::CREATED);

    let volume = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            format!("/mangas/{manga_id}/volumes"),
            json!({
                "fileName": "volume-1.jpg",
                "sourceUrl": "https://example.com/volume-1.jpg",
                "volume": "1.0",
                "locale": "en"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(volume.status(), StatusCode::CREATED);
    let volume = response_json(volume).await;
    assert_eq!(volume["volumeKey"], "1");
    assert_eq!(volume["isSpecialEdition"], false);
    let volume_id = volume["id"].as_str().unwrap();

    let duplicate = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            format!("/mangas/{manga_id}/volumes"),
            json!({
                "fileName": "other-volume-1.jpg",
                "sourceUrl": "https://example.com/other-volume-1.jpg",
                "volume": "1",
                "locale": "en"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);

    let updated_manga = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            format!("/mangas/{manga_id}"),
            json!({
                "primaryTitle": "Updated Manual Catalog",
                "contentRating": null
            }),
        ))
        .await
        .unwrap();
    assert_eq!(updated_manga.status(), StatusCode::OK);
    let updated_manga = response_json(updated_manga).await;
    assert_eq!(updated_manga["primaryTitle"], "Updated Manual Catalog");
    assert!(updated_manga["contentRating"].is_null());

    let updated_volume = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            format!("/mangas/{manga_id}/volumes/{volume_id}"),
            json!({
                "sourceUrl": "https://example.com/updated-volume-1.jpg",
                "volume": null
            }),
        ))
        .await
        .unwrap();
    assert_eq!(updated_volume.status(), StatusCode::OK);
    let updated_volume = response_json(updated_volume).await;
    assert_eq!(
        updated_volume["sourceUrl"],
        "https://example.com/updated-volume-1.jpg"
    );
    assert!(updated_volume["volume"].is_null());
    assert!(updated_volume["volumeKey"].is_null());

    let detail = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/mangas/{manga_id}?locale=en"))
                .header(header::AUTHORIZATION, "Bearer manual-read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(detail.status(), StatusCode::OK);
    let detail = response_json(detail).await;
    assert!(detail["latestVolumeNumber"].is_null());
    assert_eq!(
        detail["covers"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|cover| cover["isPrimary"] == true)
            .count(),
        1
    );

    let volumes = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/mangas/{manga_id}/volumes?locale=en"))
                .header(header::AUTHORIZATION, "Bearer manual-read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(volumes.status(), StatusCode::OK);
    let volumes = response_json(volumes).await;
    assert_eq!(volumes.as_array().unwrap().len(), 1);

    let deleted_volume = app
        .clone()
        .oneshot(json_request(
            Method::DELETE,
            format!("/mangas/{manga_id}/volumes/{volume_id}"),
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(deleted_volume.status(), StatusCode::NO_CONTENT);

    let deleted_manga = app
        .clone()
        .oneshot(json_request(
            Method::DELETE,
            format!("/mangas/{manga_id}"),
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(deleted_manga.status(), StatusCode::NO_CONTENT);

    let missing_manga = app
        .oneshot(
            Request::builder()
                .uri("/mangas/manual-catalog-test")
                .header(header::AUTHORIZATION, "Bearer manual-read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_manga.status(), StatusCode::NOT_FOUND);

    sqlx::query("DELETE FROM mangas WHERE slug = 'manual-catalog-test'")
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn catalog_mutations_invalidate_mangadex_volume_sync() {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://mangako:mangako@localhost:5432/mangako_api".to_string());
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("PostgreSQL must be available for integration tests");
    mangako_api::db::migrate(&pool).await.unwrap();
    sqlx::query("DELETE FROM mangas WHERE slug = 'volume-sync-invalidation-test'")
        .execute(&pool)
        .await
        .unwrap();
    let manga_id: uuid::Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO mangas (mangadex_id, slug, primary_title, last_synced_at)
        VALUES ('a1111111-1111-1111-1111-111111111111', 'volume-sync-invalidation-test', 'Volume Sync Invalidation', now())
        RETURNING id
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO manga_source_syncs (manga_id, source, source_id, volumes_last_checked_at, volumes_last_success_at)
        VALUES ($1, 'mangadex', 'a1111111-1111-1111-1111-111111111111', now(), now())
        "#,
    )
    .bind(manga_id)
    .execute(&pool)
    .await
    .unwrap();

    let app = mangako_api::api::router_with_api_tokens(
        pool.clone(),
        "manual-read-token".to_string(),
        "manual-write-token".to_string(),
    );
    let updated_manga = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            format!("/mangas/{manga_id}"),
            json!({ "primaryTitle": "Updated Volume Sync" }),
        ))
        .await
        .unwrap();
    assert_eq!(updated_manga.status(), StatusCode::OK);
    assert_volume_sync_invalidated(&pool, manga_id).await;

    restore_volume_sync(&pool, manga_id).await;
    let cover = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            format!("/mangas/{manga_id}/covers"),
            json!({ "sourceUrl": "https://example.com/cache-invalidation-cover.jpg" }),
        ))
        .await
        .unwrap();
    assert_eq!(cover.status(), StatusCode::CREATED);
    assert_volume_sync_invalidated(&pool, manga_id).await;

    restore_volume_sync(&pool, manga_id).await;
    let volume = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            format!("/mangas/{manga_id}/volumes"),
            json!({
                "fileName": "cache-invalidation-volume.jpg",
                "sourceUrl": "https://example.com/cache-invalidation-volume.jpg",
                "volume": "1",
                "locale": "ja"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(volume.status(), StatusCode::CREATED);
    let volume_id = response_json(volume).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_volume_sync_invalidated(&pool, manga_id).await;

    restore_volume_sync(&pool, manga_id).await;
    let updated_volume = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            format!("/mangas/{manga_id}/volumes/{volume_id}"),
            json!({ "fileName": "updated-cache-invalidation-volume.jpg" }),
        ))
        .await
        .unwrap();
    assert_eq!(updated_volume.status(), StatusCode::OK);
    assert_volume_sync_invalidated(&pool, manga_id).await;

    restore_volume_sync(&pool, manga_id).await;
    let deleted_volume = app
        .clone()
        .oneshot(json_request(
            Method::DELETE,
            format!("/mangas/{manga_id}/volumes/{volume_id}"),
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(deleted_volume.status(), StatusCode::NO_CONTENT);
    assert_volume_sync_invalidated(&pool, manga_id).await;

    sqlx::query("DELETE FROM mangas WHERE id = $1")
        .bind(manga_id)
        .execute(&pool)
        .await
        .unwrap();
}

async fn restore_volume_sync(pool: &sqlx::PgPool, manga_id: uuid::Uuid) {
    sqlx::query(
        "UPDATE manga_source_syncs SET volumes_last_checked_at = now(), volumes_last_success_at = now() WHERE manga_id = $1",
    )
    .bind(manga_id)
    .execute(pool)
    .await
    .unwrap();
}

async fn assert_volume_sync_invalidated(pool: &sqlx::PgPool, manga_id: uuid::Uuid) {
    let invalidated: bool = sqlx::query_scalar(
        "SELECT volumes_last_checked_at IS NULL AND volumes_last_success_at IS NULL FROM manga_source_syncs WHERE manga_id = $1",
    )
    .bind(manga_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert!(invalidated);
}
