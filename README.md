# MangaKo API

Rust API for the MangaKo app.

## Database

Start PostgreSQL:

```sh
docker compose up -d postgres
```

Start PostgreSQL and the API:

```sh
docker compose up --build
```

Default local connection string:

```sh
postgres://mangako:mangako@localhost:5432/mangako_api
```

Migrations live in `migrations/` and are intended to be compatible with `sqlx`.

## Run The API

```sh
DATABASE_URL=postgres://mangako:mangako@localhost:5432/mangako_api API_TOKEN=change-me cargo run
```

The API listens on `127.0.0.1:3000` by default. Override it with `HTTP_ADDR`.
When running through Docker Compose, it is exposed on `localhost:3000`.

## Authentication

Catalog routes require an API token configured with `API_TOKEN`.

Send it as a Bearer token:

```sh
curl -H 'Authorization: Bearer change-me' 'http://localhost:3000/mangas?title=frieren'
```

Public routes do not require a token: `GET /health`, `GET /docs`, and `GET /api-docs/openapi.json`.

## Operations

Production startup enables structured request logs and an in-memory response cache for successful catalog `GET` requests.

Default settings:

```env
CACHE_TTL_SECONDS=60
CACHE_MAX_ENTRIES=1000
CACHE_MAX_BYTES=67108864
RUST_LOG=mangako_api=info,tower_http=info
```

- Cacheable responses include `X-Cache: MISS` on the first request and `X-Cache: HIT` while cached.
- Responses larger than the total byte budget include `X-Cache: BYPASS` and are not stored.
- `CACHE_TTL_SECONDS=0` disables response caching.
- Searches without a title use a response-specific cache TTL of 6 hours.
- Requests with `refresh=true` bypass and invalidate cached pages for that endpoint.

Cache state is local to each API process. A multi-instance deployment should move caching to a reverse proxy, CDN, API gateway, or shared Redis-backed implementation.

Rate limiting is intentionally not implemented inside the API. If it becomes necessary, configure it at the reverse proxy, CDN, or API gateway layer.

Every response receives an `X-Request-Id`, and request logs include request ID, method, URI, status, and latency. Authorization headers are not logged. MangaDex requests use a 5-second connection timeout and a 15-second total timeout.

## ARM64

The Docker base images and Rust dependencies support Linux ARM64. CI builds and publishes a single `linux/amd64,linux/arm64` image and runs an ARM64 smoke test. See `docs/arm64-deployment.md` for GHCR deployment, cache sizing, and PostgreSQL migration instructions.

## Endpoints

- `GET /docs`: Swagger UI.
- `GET /api-docs/openapi.json`: OpenAPI JSON specification.
- `GET /health`: returns `ok`.
- `GET /mangas?title={title}&limit=10&offset=0&locale={locale}`: requires API token. MangaDex defines search ordering and pagination; results are persisted locally. If MangaDex fails, the API returns a paginated local fallback. `locale` calculates `latestVolumeNumber` from that cover language; regional values are matched by base language (`pt` matches `pt-br`) and `original` selects the manga's original language. `/mangas/` with a trailing slash is also accepted.
- `GET /mangas?limit=10&offset=0`: requires API token. Returns MangaDex titles ordered by followed count. Successful pages are cached for 6 hours.
- `GET /mangas/{id_or_slug}?refresh=false&locale={locale}`: requires API token. Returns one manga by internal UUID, MangaDex UUID, or slug. Records older than one day are refreshed from MangaDex; `refresh=true` forces the attempt. Stale local data is served if MangaDex is unavailable. `locale` calculates `latestVolumeNumber` from that cover language, falling back to the original language when needed.
- `GET /mangas/{id_or_slug}/volumes?limit=50&offset=0&refresh=false&locale={locale}`: requires API token. Returns a stable page of regular volume covers for `locale`, plus every special edition regardless of language. Missing or stale volume data is synchronized from MangaDex. `refresh=true` forces a complete refresh and reconciles removed covers.

`MangaResponse.latestVolumeNumber` contains the highest regular synchronized numeric volume for `locale` when supplied. Without `locale`, it preserves the Japanese default and falls back to the original language, then MangaDex `lastVolume`, before cover synchronization.

## Initial Manga Catalog Model

- `mangas`: canonical manga records, optionally linked to MangaDex via `mangadex_id`.
- `manga_localizations`: one localized title and/or description per normalized language, for example `en`, `pt-br`, `ja`, and `ko`.
- `manga_aliases`: alternate titles per language for search and MangaDex ingestion.
- `manga_covers`: cover/image metadata, with either external `source_url` or future internal `storage_key`.
- `creators` and `manga_creators`: MangaDex authors/artists and their manga roles.
- `manga_volumes`: MangaDex cover records used as volume images, deduplicated by `(manga_id, volume_key, locale)` for numbered volumes.
- `manga_source_syncs`: source refresh bookkeeping for incremental sync from MangaDex.

User library, progress, and ownership data are intentionally not modeled in this API yet.

## Tests

PostgreSQL is required for integration tests. Start it before running the full suite:

```sh
docker compose up -d postgres
cargo test
```
