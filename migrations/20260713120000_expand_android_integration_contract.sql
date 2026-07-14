ALTER TABLE mangas
    ADD COLUMN mangadex_last_volume TEXT,
    ADD CONSTRAINT mangas_mangadex_last_volume_not_blank
        CHECK (mangadex_last_volume IS NULL OR length(trim(mangadex_last_volume)) > 0);

ALTER TABLE manga_source_syncs
    ADD COLUMN volumes_last_checked_at TIMESTAMPTZ,
    ADD COLUMN volumes_last_success_at TIMESTAMPTZ,
    ADD COLUMN volumes_last_error TEXT;

WITH ranked AS (
    SELECT id,
           row_number() OVER (
               PARTITION BY manga_id, lower(language)
               ORDER BY is_primary DESC, updated_at DESC, id
           ) AS position
    FROM manga_localizations
)
DELETE FROM manga_localizations
WHERE id IN (SELECT id FROM ranked WHERE position > 1);

UPDATE manga_localizations SET language = lower(language);

WITH ranked AS (
    SELECT id,
           row_number() OVER (
               PARTITION BY manga_id, lower(language), normalized_title
               ORDER BY created_at DESC, id
           ) AS position
    FROM manga_aliases
)
DELETE FROM manga_aliases
WHERE id IN (SELECT id FROM ranked WHERE position > 1);

UPDATE manga_aliases SET language = lower(language);

ALTER TABLE manga_localizations
    ALTER COLUMN title DROP NOT NULL;

ALTER TABLE manga_localizations
    DROP CONSTRAINT manga_localizations_title_not_blank,
    ADD CONSTRAINT manga_localizations_title_not_blank
        CHECK (title IS NULL OR length(trim(title)) > 0),
    ADD CONSTRAINT manga_localizations_has_content
        CHECK (title IS NOT NULL OR description IS NOT NULL);
