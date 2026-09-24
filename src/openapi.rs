use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

use crate::{
    error::ErrorResponse,
    manga::{
        CreateMangaAliasRequest, CreateMangaCoverRequest, CreateMangaLocalizationRequest,
        CreateMangaRequest, CreateMangaVolumeRequest, MangaAliasResponse, MangaCoverResponse,
        MangaCreatorResponse, MangaLocalizationResponse, MangaResponse, MangaVolumeResponse,
        UpdateMangaRequest, UpdateMangaVolumeRequest,
    },
    operations::FallbackStatsResponse,
};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "MangaKo API",
        version = env!("CARGO_PKG_VERSION"),
        description = "Backend API for the MangaKo catalog, canonical MangaDex search, local fallback, covers, volumes, and incremental refresh."
    ),
    paths(
        crate::manga::search_mangas,
        crate::manga::create_manga,
        crate::manga::get_manga,
        crate::manga::update_manga,
        crate::manga::delete_manga,
        crate::manga::create_manga_cover,
        crate::manga::upload_manga_cover,
        crate::manga::get_manga_volumes,
        crate::manga::create_manga_volume,
        crate::manga::upload_manga_volume,
        crate::manga::update_manga_volume,
        crate::manga::delete_manga_volume,
        crate::manga::mangadex_fallback_stats
    ),
    components(
        schemas(
            ErrorResponse,
            CreateMangaAliasRequest,
            CreateMangaCoverRequest,
            CreateMangaLocalizationRequest,
            CreateMangaRequest,
            CreateMangaVolumeRequest,
            UpdateMangaRequest,
            UpdateMangaVolumeRequest,
            MangaAliasResponse,
            MangaCoverResponse,
            MangaCreatorResponse,
            MangaLocalizationResponse,
            MangaResponse,
            MangaVolumeResponse,
            FallbackStatsResponse
        )
    ),
    modifiers(&SecurityAddon),
    tags(
        (name = "manga", description = "Manga catalog search and detail endpoints")
    )
)]
pub struct ApiDoc;

struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "read_token",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("API token")
                    .build(),
            ),
        );
        components.add_security_scheme(
            "write_token",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("Catalog write token")
                    .build(),
            ),
        );
    }
}
