use axum::{body::Body, http::Request};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
async fn volumes_can_be_loaded_by_mangadex_id() {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://mangako:mangako@localhost:5432/mangako_api".to_string());
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("PostgreSQL must be available for integration tests");

    mangako_api::db::migrate(&pool).await.unwrap();

    let mangadex_id = Uuid::parse_str("b0b721ff-c388-4486-aa0f-c2b0bb321512").unwrap();

    sqlx::query("DELETE FROM mangas WHERE mangadex_id = $1")
        .bind(mangadex_id)
        .execute(&pool)
        .await
        .unwrap();

    let manga_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO mangas (mangadex_id, slug, primary_title, last_synced_at)
        VALUES ($1, 'frieren-b0b721ff', 'Frieren', now())
        RETURNING id
        "#,
    )
    .bind(mangadex_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    sqlx::query(
        r#"
        INSERT INTO manga_source_syncs (
            manga_id, source, source_id, volumes_last_checked_at, volumes_last_success_at
        )
        VALUES ($1, 'mangadex', $2, now(), now())
        "#,
    )
    .bind(manga_id)
    .bind(mangadex_id)
    .execute(&pool)
    .await
    .unwrap();

    for (cover_id, volume) in [
        ("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", "1"),
        ("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb", "2"),
        ("cccccccc-cccc-cccc-cccc-cccccccccccc", "3"),
    ] {
        let file_name = format!("cover-{volume}.jpg");
        let source_url =
            format!("https://uploads.mangadex.org/covers/{mangadex_id}/{file_name}.512.jpg");
        sqlx::query(
            r#"
            INSERT INTO manga_volumes (
                manga_id, mangadex_cover_id, file_name, source_url,
                volume, volume_key, locale, is_special_edition
            )
            VALUES ($1, $2, $3, $4, $5, $5, 'ja', false)
            "#,
        )
        .bind(manga_id)
        .bind(Uuid::parse_str(cover_id).unwrap())
        .bind(file_name)
        .bind(source_url)
        .bind(volume)
        .execute(&pool)
        .await
        .unwrap();
    }
    sqlx::query(
        r#"
        INSERT INTO manga_volumes (
            manga_id, mangadex_cover_id, file_name, source_url,
            volume, volume_key, locale, is_special_edition
        )
        VALUES ($1, 'dddddddd-dddd-dddd-dddd-dddddddddddd', 'cover-4.jpg',
                'https://example.com/cover-4.jpg', '4', '4', 'pt-br', false)
        "#,
    )
    .bind(manga_id)
    .execute(&pool)
    .await
    .unwrap();

    let app = mangako_api::api::router(pool.clone());
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/mangas/{mangadex_id}/volumes?limit=1&offset=1"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        status,
        axum::http::StatusCode::OK,
        "response body: {}",
        String::from_utf8_lossy(&body)
    );
    let volumes: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let volumes = volumes.as_array().unwrap();
    assert_eq!(volumes.len(), 1);
    assert_eq!(volumes[0]["volume"], "2");
    assert!(
        volumes[0]["sourceUrl"]
            .as_str()
            .unwrap()
            .starts_with("https://")
    );

    let detail = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/mangas/{mangadex_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(detail.status(), axum::http::StatusCode::OK);
    let detail_body = axum::body::to_bytes(detail.into_body(), usize::MAX)
        .await
        .unwrap();
    let manga: serde_json::Value = serde_json::from_slice(&detail_body).unwrap();
    assert_eq!(manga["latestVolumeNumber"], "3");

    let portuguese_detail = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/mangas/{mangadex_id}?locale=PT-BR"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(portuguese_detail.status(), axum::http::StatusCode::OK);
    let portuguese_body = axum::body::to_bytes(portuguese_detail.into_body(), usize::MAX)
        .await
        .unwrap();
    let portuguese_manga: serde_json::Value = serde_json::from_slice(&portuguese_body).unwrap();
    assert_eq!(portuguese_manga["latestVolumeNumber"], "4");

    let unavailable_locale_detail = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/mangas/{mangadex_id}?locale=en"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let unavailable_locale_body =
        axum::body::to_bytes(unavailable_locale_detail.into_body(), usize::MAX)
            .await
            .unwrap();
    let unavailable_locale_manga: serde_json::Value =
        serde_json::from_slice(&unavailable_locale_body).unwrap();
    assert!(unavailable_locale_manga["latestVolumeNumber"].is_null());

    let portuguese_volumes = app
        .oneshot(
            Request::builder()
                .uri(format!("/mangas/{mangadex_id}/volumes?locale=pt-BR"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(portuguese_volumes.status(), axum::http::StatusCode::OK);
    let portuguese_volumes_body = axum::body::to_bytes(portuguese_volumes.into_body(), usize::MAX)
        .await
        .unwrap();
    let portuguese_volumes: serde_json::Value =
        serde_json::from_slice(&portuguese_volumes_body).unwrap();
    assert_eq!(portuguese_volumes.as_array().unwrap().len(), 1);
    assert_eq!(portuguese_volumes[0]["locale"], "pt-br");

    sqlx::query("DELETE FROM mangas WHERE mangadex_id = $1")
        .bind(mangadex_id)
        .execute(&pool)
        .await
        .unwrap();
}
