use axum::{body::Body, http::Request};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
async fn volumes_combine_global_specials_with_regular_language_before_pagination() {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://mangako:mangako@localhost:5432/mangako_api".to_string());
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("PostgreSQL must be available for integration tests");
    mangako_api::db::migrate(&pool).await.unwrap();

    sqlx::query("DELETE FROM mangas WHERE slug = 'global-specials-test'")
        .execute(&pool)
        .await
        .unwrap();
    let manga_id: Uuid = sqlx::query_scalar(
        "INSERT INTO mangas (slug, primary_title, original_language, last_synced_at)
         VALUES ('global-specials-test', 'Global specials', 'ko', now()) RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    for (name, volume, locale, special, deleted) in [
        ("ja-normal", Some("2"), "ja", false, false),
        ("pt-normal", Some("2"), "pt-br", false, false),
        ("ko-normal", Some("2"), "ko", false, false),
        ("en-normal", Some("2"), "en", false, false),
        ("ja-special", Some("1.5"), "ja", true, false),
        ("pt-special", Some("1.5"), "pt-br", true, false),
        ("ko-special", Some("1.5"), "ko", true, false),
        ("en-special", Some("1.5"), "en", true, false),
        ("und-special-a", None, "und", true, false),
        ("und-special-b", None, "und", true, false),
        ("deleted-special", Some("0.5"), "ja", true, true),
    ] {
        sqlx::query(
            "INSERT INTO manga_volumes
             (manga_id, file_name, source_url, volume, volume_key, locale, is_special_edition, deleted_at)
             VALUES ($1, $2, 'https://example.com/cover.jpg', $3, $3, $4, $5,
                     CASE WHEN $6 THEN now() END)",
        )
        .bind(manga_id)
        .bind(name)
        .bind(volume)
        .bind(locale)
        .bind(special)
        .bind(deleted)
        .execute(&pool)
        .await
        .unwrap();
    }

    let app = mangako_api::api::router(pool.clone());
    for japanese_normals_deleted in [false, true] {
        if japanese_normals_deleted {
            sqlx::query(
                "UPDATE manga_volumes SET deleted_at = now()
                 WHERE manga_id = $1 AND locale = 'ja' AND is_special_edition = false",
            )
            .bind(manga_id)
            .execute(&pool)
            .await
            .unwrap();
        }
        for (query, expected_normal) in [
            ("", Some(if japanese_normals_deleted { "ko" } else { "ja" })),
            (
                "&locale=ja",
                if japanese_normals_deleted {
                    None
                } else {
                    Some("ja")
                },
            ),
            ("&locale=pt", Some("pt-br")),
            ("&locale=original", Some("ko")),
            ("&locale=fr", None),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/mangas/{manga_id}/volumes?limit=100{query}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), axum::http::StatusCode::OK);
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let volumes: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
            let normals: Vec<_> = volumes
                .iter()
                .filter(|volume| volume["isSpecialEdition"] == false)
                .map(|volume| volume["locale"].as_str().unwrap())
                .collect();
            assert_eq!(
                normals,
                expected_normal.into_iter().collect::<Vec<_>>(),
                "{query}"
            );
            let specials: Vec<_> = volumes
                .iter()
                .filter(|volume| volume["isSpecialEdition"] == true)
                .map(|volume| volume["locale"].as_str().unwrap())
                .collect();
            assert_eq!(specials, ["en", "ja", "ko", "pt-br", "und", "und"]);
            let ids: std::collections::HashSet<_> = volumes
                .iter()
                .map(|volume| volume["id"].as_str().unwrap())
                .collect();
            assert_eq!(ids.len(), volumes.len());
            assert!(
                volumes
                    .iter()
                    .all(|volume| volume["fileName"] != "deleted-special")
            );

            let mut paged = Vec::new();
            for offset in (0..=volumes.len() + 1).step_by(2) {
                let response = app
                    .clone()
                    .oneshot(
                        Request::builder()
                            .uri(format!(
                                "/mangas/{manga_id}/volumes?limit=2&offset={offset}{query}"
                            ))
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(response.status(), axum::http::StatusCode::OK);
                let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap();
                let page: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
                assert!(page.len() <= 2);
                paged.extend(page);
            }
            assert_eq!(paged, volumes, "{query}");
        }
    }
    sqlx::query("DELETE FROM mangas WHERE id = $1")
        .bind(manga_id)
        .execute(&pool)
        .await
        .unwrap();
}

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
        INSERT INTO mangas (mangadex_id, slug, primary_title, original_language, last_synced_at)
        VALUES ($1, 'frieren-b0b721ff', 'Frieren', 'ja', now())
        RETURNING id
        "#,
    )
    .bind(mangadex_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("DELETE FROM creators WHERE mangadex_id IN ('a4010401-0401-0401-0401-040104010401', 'b4010401-0401-0401-0401-040104010401')")
        .execute(&pool)
        .await
        .unwrap();
    let author_id: Uuid = sqlx::query_scalar(
        "INSERT INTO creators (mangadex_id, name) VALUES ('a4010401-0401-0401-0401-040104010401', 'Test Author') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let artist_id: Uuid = sqlx::query_scalar(
        "INSERT INTO creators (mangadex_id, name) VALUES ('b4010401-0401-0401-0401-040104010401', 'Test Artist') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    for (creator_id, role) in [(author_id, "author"), (artist_id, "artist")] {
        sqlx::query("INSERT INTO manga_creators (manga_id, creator_id, role) VALUES ($1, $2, $3)")
            .bind(manga_id)
            .bind(creator_id)
            .bind(role)
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
        VALUES ($1, 'e3010301-0301-0301-0301-030103010301', 'cover-special.jpg',
                'https://example.com/cover-special.jpg', '99.2', '99.2', 'en', true)
        "#,
    )
    .bind(manga_id)
    .execute(&pool)
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
        ("a3010301-0301-0301-0301-030103010301", "1"),
        ("b3010301-0301-0301-0301-030103010301", "2"),
        ("c3010301-0301-0301-0301-030103010301", "3"),
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
        VALUES ($1, 'd3010301-0301-0301-0301-030103010301', 'cover-4.jpg',
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

    let default_volumes = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/mangas/{mangadex_id}/volumes?limit=10"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(default_volumes.status(), axum::http::StatusCode::OK);
    let default_volumes_body = axum::body::to_bytes(default_volumes.into_body(), usize::MAX)
        .await
        .unwrap();
    let default_volumes: serde_json::Value = serde_json::from_slice(&default_volumes_body).unwrap();
    let default_volumes = default_volumes.as_array().unwrap();
    assert_eq!(default_volumes.len(), 4);
    assert!(
        default_volumes
            .iter()
            .all(|volume| volume["locale"] == "ja" || volume["isSpecialEdition"] == true)
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
    assert_eq!(manga["authors"][0]["name"], "Test Author");
    assert_eq!(manga["artists"][0]["name"], "Test Artist");

    let portuguese_detail = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/mangas/{mangadex_id}?locale=pt"))
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
    assert_eq!(unavailable_locale_manga["latestVolumeNumber"], "3");

    let portuguese_volumes = app
        .oneshot(
            Request::builder()
                .uri(format!("/mangas/{mangadex_id}/volumes?locale=pt"))
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
    assert_eq!(portuguese_volumes.as_array().unwrap().len(), 2);
    assert!(
        portuguese_volumes
            .as_array()
            .unwrap()
            .iter()
            .all(|volume| volume["locale"] == "pt-br" || volume["isSpecialEdition"] == true)
    );

    sqlx::query("DELETE FROM mangas WHERE mangadex_id = $1")
        .bind(mangadex_id)
        .execute(&pool)
        .await
        .unwrap();
}
