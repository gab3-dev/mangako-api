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
DATABASE_URL=postgres://mangako:mangako@localhost:5432/mangako_api \
API_READ_TOKEN=replace-with-a-unique-32-character-read-token \
API_WRITE_TOKEN=replace-with-a-different-32-character-write-token \
cargo run
```

The API listens on `127.0.0.1:3000` by default. Override it with `HTTP_ADDR`.
When running through Docker Compose, it is exposed on `localhost:3000`.

## Authentication

Read routes require `API_READ_TOKEN`. Catalog creation routes require the separate server-only `API_WRITE_TOKEN`; do not distribute it to the Android app.

Send it as a Bearer token:

```sh
curl -H 'Authorization: Bearer replace-with-the-read-token' 'http://localhost:3000/mangas?title=frieren'
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
- `GET /stats/mangadex-fallback` requires an API token and reports `catalogRequests`, `fallbackRequests`, and `fallbackRate` since the API process started. A request counts as a fallback at most once, including when serving stale local manga or volumes after MangaDex fails.

Cache state is local to each API process. A multi-instance deployment should move caching to a reverse proxy, CDN, API gateway, or shared Redis-backed implementation.

Fallback statistics are also process-local and reset when the API restarts. Poll and retain this endpoint externally when longer historical analysis is required.

Rate limiting is intentionally not implemented inside the API. If it becomes necessary, configure it at the reverse proxy, CDN, or API gateway layer.

Every response receives an `X-Request-Id`, and request logs include request ID, method, URI, status, and latency. Authorization headers are not logged. MangaDex requests use a 5-second connection timeout and a 15-second total timeout.

## ARM64

CI publishes a `linux/amd64` image and runs an ARM64 smoke test. See `docs/arm64-deployment.md` for GHCR deployment, cache sizing, and PostgreSQL migration instructions.

## Endpoints

- `GET /docs`: Swagger UI.
- `GET /api-docs/openapi.json`: OpenAPI JSON specification.
- `GET /health`: returns `ok`.
- `GET /stats/mangadex-fallback`: requires API token. Returns MangaDex fallback statistics since process start.
- `POST /mangas`: requires API token. Creates a local-only manga, including metadata, localizations, aliases, and general covers. Local-only manga are never refreshed from MangaDex and are returned by title search before MangaDex is queried.
- `POST /mangas/{id_or_slug}/covers`: requires API token. Adds a general cover by external `sourceUrl` or internal `storageKey`; setting `isPrimary` replaces the current primary cover.
- `POST /mangas/{id_or_slug}/volumes`: requires API token. Adds a local volume cover by external `sourceUrl`. Numbered volumes are unique per normalized number and locale; fractional and unnumbered entries are special editions.
- `GET /mangas?title={title}&limit=10&offset=0&locale={locale}`: requires API token. Matching local-only manga are returned first; otherwise MangaDex defines search ordering and pagination, and its results are persisted locally. If MangaDex fails, the API returns a paginated local fallback. `locale` calculates `latestVolumeNumber` from that cover language; regional values are matched by base language (`pt` matches `pt-br`) and `original` selects the manga's original language. `/mangas/` with a trailing slash is also accepted.
- `GET /mangas?limit=10&offset=0`: requires API token. Returns MangaDex titles ordered by followed count. Successful pages are cached for 6 hours.
- `GET /mangas/{id_or_slug}?refresh=false&locale={locale}`: requires the read token. Returns one manga by internal UUID, MangaDex UUID, or slug. Records older than one day are refreshed from MangaDex; `refresh=true` forces the attempt and requires the write token. Stale local data is served if MangaDex is unavailable. `locale` calculates `latestVolumeNumber` from that cover language, falling back to the original language when needed.
- `GET /mangas/{id_or_slug}/volumes?limit=50&offset=0&refresh=false&locale={locale}`: requires API token. Returns regular volumes for `locale` plus active special editions in all languages, combined before stable ordering and pagination. Regional values match by base language (`pt` matches `pt-br`); `original` selects the manga's original language. An explicit `locale` does not fall back to another language for regular volumes. Without `locale`, it uses Japanese and falls back to the original language only when no active regular Japanese volumes exist; Japanese special editions do not block fallback. Deleted covers are excluded. Missing or stale volume data is synchronized from MangaDex. `refresh=true` forces a complete refresh and reconciles removed covers.

`MangaResponse.latestVolumeNumber` contains the highest regular synchronized numeric volume for `locale` when supplied. Without `locale`, it preserves the Japanese default and falls back to the original language, then MangaDex `lastVolume`, before cover synchronization.

## Initial Manga Catalog Model

- `mangas`: canonical manga records, optionally linked to MangaDex via `mangadex_id`.
- `manga_localizations`: one localized title and/or description per normalized language, for example `en`, `pt-br`, `ja`, and `ko`.
- `manga_aliases`: alternate titles per language for search and MangaDex ingestion.
- `manga_covers`: cover/image metadata, with either external `source_url` or future internal `storage_key`.
- `creators` and `manga_creators`: MangaDex authors/artists and their manga roles.
- `manga_volumes`: MangaDex or local cover records used as volume images, deduplicated by `(manga_id, volume_key, locale)` for numbered volumes.
- `manga_source_syncs`: source refresh bookkeeping for incremental sync from MangaDex.

User library, progress, and ownership data are intentionally not modeled in this API yet.

## Tests

PostgreSQL is required for integration tests. Start it before running the full suite:

```sh
docker compose up -d postgres
cargo test
```
