use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

use crate::{
    error::ErrorResponse,
    manga::{
        MangaAliasResponse, MangaCoverResponse, MangaCreatorResponse, MangaLocalizationResponse,
        MangaResponse, MangaVolumeResponse,
    },
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
        crate::manga::get_manga,
        crate::manga::get_manga_volumes
    ),
    components(
        schemas(
            ErrorResponse,
            MangaAliasResponse,
            MangaCoverResponse,
            MangaCreatorResponse,
            MangaLocalizationResponse,
            MangaResponse,
            MangaVolumeResponse
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
        openapi
            .components
            .get_or_insert_with(Default::default)
            .add_security_scheme(
                "api_token",
                SecurityScheme::Http(
                    HttpBuilder::new()
                        .scheme(HttpAuthScheme::Bearer)
                        .bearer_format("API token")
                        .build(),
                ),
            );
    }
}
