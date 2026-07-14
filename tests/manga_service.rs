use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

use mangako_api::{
    manga_service::MangaService,
    mangadex::{
        MangaDexApi, MangaDexAttributes, MangaDexCover, MangaDexCoverAttributes, MangaDexManga,
    },
};

#[derive(Clone, Default)]
struct FakeMangaDex {
    calls: Arc<Mutex<Vec<String>>>,
    search_results: Vec<MangaDexManga>,
    covers: Vec<MangaDexCover>,
}

impl MangaDexApi for FakeMangaDex {
    async fn get_manga(&self, id: Uuid) -> Result<Option<MangaDexManga>, reqwest::Error> {
        self.calls.lock().unwrap().push(format!("get_manga:{id}"));
        Ok(None)
    }

    async fn search_mangas(
        &self,
        title: Option<&str>,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<MangaDexManga>, reqwest::Error> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("search_mangas:{title:?}:{offset}:{limit}"));
        Ok(self.search_results.clone())
    }

    async fn list_covers(
        &self,
        manga_id: Uuid,
        _offset: u32,
        _limit: u32,
    ) -> Result<Vec<MangaDexCover>, reqwest::Error> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("list_covers:{manga_id}"));
        if _offset == 0 {
            Ok(self.covers.clone())
        } else {
            Ok(Vec::new())
        }
    }
}

#[tokio::test]
async fn search_uses_mangadex_as_canonical_source() {
    let pool = test_pool().await;
    let mangadex_id = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();

    cleanup_manga(&pool, mangadex_id).await;
    sqlx::query(
        r#"
        INSERT INTO mangas (mangadex_id, slug, primary_title)
        VALUES ($1, 'local-hit-11111111', 'Local Hit')
        "#,
    )
    .bind(mangadex_id)
    .execute(&pool)
    .await
    .unwrap();

    let fake = FakeMangaDex {
        calls: Arc::default(),
        search_results: vec![mangadex_manga(mangadex_id, "Remote Hit")],
        covers: Vec::new(),
    };
    let service = MangaService::new(pool.clone(), fake.clone());

    let results = service
        .search_mangas(Some("Local Hit"), 6, 12)
        .await
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].primary_title, "Remote Hit");
    assert_eq!(
        fake.calls.lock().unwrap().as_slice(),
        ["search_mangas:Some(\"Local Hit\"):12:6"]
    );

    cleanup_manga(&pool, mangadex_id).await;
}

#[tokio::test]
async fn search_persists_and_returns_mangadex_fallback_results() {
    let pool = test_pool().await;
    let mangadex_id = Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap();

    cleanup_manga(&pool, mangadex_id).await;

    let fake = FakeMangaDex {
        calls: Arc::default(),
        search_results: vec![mangadex_manga(mangadex_id, "Remote Hit")],
        covers: Vec::new(),
    };
    let service = MangaService::new(pool.clone(), fake.clone());

    let results = service
        .search_mangas(Some("Remote Hit"), 10, 0)
        .await
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].mangadex_id, Some(mangadex_id));
    assert_eq!(results[0].primary_title, "Remote Hit");
    assert_eq!(results[0].latest_volume_number.as_deref(), Some("12"));
    assert_eq!(
        fake.calls.lock().unwrap().as_slice(),
        ["search_mangas:Some(\"Remote Hit\"):0:10"]
    );

    let persisted: Option<String> = sqlx::query_scalar(
        "SELECT primary_title FROM mangas WHERE mangadex_id = $1 AND deleted_at IS NULL",
    )
    .bind(mangadex_id)
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(persisted.as_deref(), Some("Remote Hit"));

    cleanup_manga(&pool, mangadex_id).await;
}

#[tokio::test]
async fn empty_search_uses_mangadex_popular_page_parameters() {
    let pool = test_pool().await;
    let mangadex_id = Uuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap();
    cleanup_manga(&pool, mangadex_id).await;

    let fake = FakeMangaDex {
        calls: Arc::default(),
        search_results: vec![mangadex_manga(mangadex_id, "Popular Hit")],
        covers: Vec::new(),
    };
    let service = MangaService::new(pool.clone(), fake.clone());

    let results = service.search_mangas(None, 6, 18).await.unwrap();

    assert_eq!(results[0].primary_title, "Popular Hit");
    assert_eq!(
        fake.calls.lock().unwrap().as_slice(),
        ["search_mangas:None:18:6"]
    );
    cleanup_manga(&pool, mangadex_id).await;
}

#[tokio::test]
async fn alternate_titles_populate_localized_titles_without_losing_descriptions() {
    let pool = test_pool().await;
    let mangadex_id = Uuid::parse_str("44444444-4444-4444-4444-444444444444").unwrap();
    cleanup_manga(&pool, mangadex_id).await;

    let mut remote = mangadex_manga(mangadex_id, "English Title");
    remote.attributes.alt_titles = vec![
        HashMap::from([("en".to_string(), Some("English Alias".to_string()))]),
        HashMap::from([("pt-BR".to_string(), Some("Titulo em portugues".to_string()))]),
        HashMap::from([("es".to_string(), Some("Titulo en espanol".to_string()))]),
        HashMap::from([("ar".to_string(), Some("عنوان عربي".to_string()))]),
    ];
    remote.attributes.description.insert(
        "pt-BR".to_string(),
        Some("Descricao em portugues".to_string()),
    );
    remote
        .attributes
        .description
        .insert("es".to_string(), Some("Descripcion sin titulo".to_string()));
    let fake = FakeMangaDex {
        calls: Arc::default(),
        search_results: vec![remote],
        covers: Vec::new(),
    };
    let service = MangaService::new(pool.clone(), fake);

    let results = service
        .search_mangas(Some("English Title"), 10, 0)
        .await
        .unwrap();
    let portuguese = results[0]
        .localizations
        .iter()
        .find(|localization| localization.language == "pt-br")
        .unwrap();

    assert_eq!(portuguese.title.as_deref(), Some("Titulo em portugues"));
    assert_eq!(
        portuguese.description.as_deref(),
        Some("Descricao em portugues")
    );
    let spanish = results[0]
        .localizations
        .iter()
        .find(|localization| localization.language == "es")
        .unwrap();
    assert_eq!(spanish.title.as_deref(), Some("Titulo en espanol"));
    assert_eq!(
        spanish.description.as_deref(),
        Some("Descripcion sin titulo")
    );
    let english = results[0]
        .localizations
        .iter()
        .find(|localization| localization.language == "en")
        .unwrap();
    assert_eq!(english.title.as_deref(), Some("English Title"));
    let arabic = results[0]
        .localizations
        .iter()
        .find(|localization| localization.language == "ar")
        .unwrap();
    assert_eq!(arabic.title.as_deref(), Some("عنوان عربي"));
    assert_eq!(arabic.description, None);
    cleanup_manga(&pool, mangadex_id).await;
}

#[tokio::test]
async fn forced_volume_refresh_reconciles_removed_covers() {
    let pool = test_pool().await;
    let mangadex_id = Uuid::parse_str("55555555-5555-5555-5555-555555555555").unwrap();
    let current_cover_id = Uuid::parse_str("dddddddd-dddd-dddd-dddd-dddddddddddd").unwrap();
    let removed_cover_id = Uuid::parse_str("eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee").unwrap();
    cleanup_manga(&pool, mangadex_id).await;

    let manga_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO mangas (mangadex_id, slug, primary_title, last_synced_at)
        VALUES ($1, 'refresh-test-55555555', 'Refresh Test', now())
        RETURNING id
        "#,
    )
    .bind(mangadex_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO manga_volumes (
            manga_id, mangadex_cover_id, file_name, source_url,
            volume, volume_key, locale, is_special_edition
        )
        VALUES ($1, $2, 'removed.jpg', 'https://example.com/removed.jpg', '1', '1', 'ja', false)
        "#,
    )
    .bind(manga_id)
    .bind(removed_cover_id)
    .execute(&pool)
    .await
    .unwrap();

    let fake = FakeMangaDex {
        calls: Arc::default(),
        search_results: Vec::new(),
        covers: vec![MangaDexCover {
            id: current_cover_id,
            attributes: MangaDexCoverAttributes {
                file_name: "current.jpg".to_string(),
                volume: Some("2".to_string()),
                locale: Some("ja".to_string()),
                created_at: None,
                updated_at: None,
                version: Some(1),
            },
        }],
    };
    let service = MangaService::new(pool.clone(), fake);

    let volumes = service
        .get_manga_volumes(&mangadex_id.to_string(), 50, 0, true)
        .await
        .unwrap();

    assert_eq!(volumes.len(), 1);
    assert_eq!(volumes[0].mangadex_cover_id, Some(current_cover_id));
    let removed_at: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT deleted_at FROM manga_volumes WHERE mangadex_cover_id = $1")
            .bind(removed_cover_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(removed_at.is_some());
    cleanup_manga(&pool, mangadex_id).await;
}

async fn test_pool() -> PgPool {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://mangako:mangako@localhost:5432/mangako_api".to_string());
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("PostgreSQL must be available for integration tests");

    mangako_api::db::migrate(&pool).await.unwrap();
    pool
}

async fn cleanup_manga(pool: &PgPool, mangadex_id: Uuid) {
    sqlx::query("DELETE FROM mangas WHERE mangadex_id = $1")
        .bind(mangadex_id)
        .execute(pool)
        .await
        .unwrap();
}

fn mangadex_manga(id: Uuid, title: &str) -> MangaDexManga {
    let mut titles = HashMap::new();
    titles.insert("en".to_string(), Some(title.to_string()));

    MangaDexManga {
        id,
        kind: Some("manga".to_string()),
        attributes: MangaDexAttributes {
            title: titles,
            alt_titles: Vec::new(),
            description: HashMap::new(),
            original_language: Some("ja".to_string()),
            publication_demographic: None,
            status: Some("ongoing".to_string()),
            year: Some(2024),
            content_rating: Some("safe".to_string()),
            last_volume: Some("12".to_string()),
            updated_at: None,
            version: Some(1),
        },
        relationships: Vec::new(),
    }
}
