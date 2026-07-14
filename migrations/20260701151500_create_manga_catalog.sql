CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE FUNCTION set_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TABLE mangas (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    mangadex_id UUID UNIQUE,
    slug TEXT NOT NULL UNIQUE,
    primary_title TEXT NOT NULL,
    description TEXT,
    original_language TEXT,
    publication_demographic TEXT,
    status TEXT,
    year INTEGER,
    content_rating TEXT,
    mangadex_version INTEGER,
    source_updated_at TIMESTAMPTZ,
    last_synced_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    CONSTRAINT mangas_slug_not_blank CHECK (length(trim(slug)) > 0),
    CONSTRAINT mangas_primary_title_not_blank CHECK (length(trim(primary_title)) > 0),
    CONSTRAINT mangas_year_reasonable CHECK (year IS NULL OR year BETWEEN 1900 AND 2200)
);

CREATE TABLE manga_localizations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    manga_id UUID NOT NULL REFERENCES mangas(id) ON DELETE CASCADE,
    language TEXT NOT NULL,
    title TEXT NOT NULL,
    description TEXT,
    is_primary BOOLEAN NOT NULL DEFAULT false,
    normalized_title TEXT GENERATED ALWAYS AS (lower(regexp_replace(trim(title), '[[:space:]]+', ' ', 'g'))) STORED,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT manga_localizations_language_not_blank CHECK (length(trim(language)) > 0),
    CONSTRAINT manga_localizations_title_not_blank CHECK (length(trim(title)) > 0),
    UNIQUE (manga_id, language)
);

CREATE TABLE manga_aliases (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    manga_id UUID NOT NULL REFERENCES mangas(id) ON DELETE CASCADE,
    language TEXT NOT NULL,
    title TEXT NOT NULL,
    normalized_title TEXT GENERATED ALWAYS AS (lower(regexp_replace(trim(title), '[[:space:]]+', ' ', 'g'))) STORED,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT manga_aliases_language_not_blank CHECK (length(trim(language)) > 0),
    CONSTRAINT manga_aliases_title_not_blank CHECK (length(trim(title)) > 0),
    UNIQUE (manga_id, language, normalized_title)
);

CREATE TABLE manga_covers (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    manga_id UUID NOT NULL REFERENCES mangas(id) ON DELETE CASCADE,
    mangadex_cover_id UUID UNIQUE,
    file_name TEXT,
    source_url TEXT,
    storage_key TEXT,
    locale TEXT,
    volume TEXT,
    is_primary BOOLEAN NOT NULL DEFAULT false,
    mangadex_version INTEGER,
    source_updated_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    CONSTRAINT manga_covers_has_location CHECK (source_url IS NOT NULL OR storage_key IS NOT NULL),
    CONSTRAINT manga_covers_file_name_not_blank CHECK (file_name IS NULL OR length(trim(file_name)) > 0)
);

CREATE TABLE manga_volumes (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    manga_id UUID NOT NULL REFERENCES mangas(id) ON DELETE CASCADE,
    mangadex_cover_id UUID UNIQUE,
    file_name TEXT NOT NULL,
    source_url TEXT NOT NULL,
    volume TEXT,
    volume_key TEXT,
    locale TEXT NOT NULL DEFAULT 'und',
    is_special_edition BOOLEAN NOT NULL DEFAULT false,
    mangadex_version INTEGER,
    source_created_at TIMESTAMPTZ,
    source_updated_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    CONSTRAINT manga_volumes_file_name_not_blank CHECK (length(trim(file_name)) > 0),
    CONSTRAINT manga_volumes_source_url_not_blank CHECK (length(trim(source_url)) > 0),
    CONSTRAINT manga_volumes_locale_not_blank CHECK (length(trim(locale)) > 0),
    CONSTRAINT manga_volumes_volume_key_not_blank CHECK (volume_key IS NULL OR length(trim(volume_key)) > 0)
);

CREATE TABLE manga_source_syncs (
    manga_id UUID PRIMARY KEY REFERENCES mangas(id) ON DELETE CASCADE,
    source TEXT NOT NULL DEFAULT 'mangadex',
    source_id UUID NOT NULL,
    last_checked_at TIMESTAMPTZ,
    last_success_at TIMESTAMPTZ,
    last_error_at TIMESTAMPTZ,
    last_error TEXT,
    next_check_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT manga_source_syncs_source_valid CHECK (source IN ('mangadex')),
    UNIQUE (source, source_id)
);

CREATE INDEX mangas_updated_at_idx ON mangas (updated_at);
CREATE INDEX mangas_deleted_at_idx ON mangas (deleted_at) WHERE deleted_at IS NOT NULL;
CREATE INDEX mangas_mangadex_id_idx ON mangas (mangadex_id) WHERE mangadex_id IS NOT NULL;
CREATE INDEX mangas_primary_title_trgm_idx ON mangas USING gin (primary_title gin_trgm_ops);

CREATE UNIQUE INDEX manga_localizations_one_primary_per_manga_idx
    ON manga_localizations (manga_id)
    WHERE is_primary;
CREATE INDEX manga_localizations_manga_id_idx ON manga_localizations (manga_id);
CREATE INDEX manga_localizations_language_idx ON manga_localizations (language);
CREATE INDEX manga_localizations_normalized_title_idx ON manga_localizations (normalized_title);
CREATE INDEX manga_localizations_title_trgm_idx ON manga_localizations USING gin (title gin_trgm_ops);

CREATE INDEX manga_aliases_manga_id_idx ON manga_aliases (manga_id);
CREATE INDEX manga_aliases_language_idx ON manga_aliases (language);
CREATE INDEX manga_aliases_normalized_title_idx ON manga_aliases (normalized_title);
CREATE INDEX manga_aliases_title_trgm_idx ON manga_aliases USING gin (title gin_trgm_ops);

CREATE UNIQUE INDEX manga_covers_one_primary_per_manga_idx
    ON manga_covers (manga_id)
    WHERE is_primary AND deleted_at IS NULL;
CREATE INDEX manga_covers_manga_id_idx ON manga_covers (manga_id);
CREATE INDEX manga_covers_updated_at_idx ON manga_covers (updated_at);

CREATE UNIQUE INDEX manga_volumes_numbered_identity_idx
    ON manga_volumes (manga_id, volume_key, locale)
    WHERE volume_key IS NOT NULL AND deleted_at IS NULL;
CREATE INDEX manga_volumes_manga_id_idx ON manga_volumes (manga_id);
CREATE INDEX manga_volumes_updated_at_idx ON manga_volumes (updated_at);
CREATE INDEX manga_volumes_deleted_at_idx ON manga_volumes (deleted_at) WHERE deleted_at IS NOT NULL;

CREATE INDEX manga_source_syncs_next_check_at_idx ON manga_source_syncs (next_check_at);

CREATE TRIGGER mangas_set_updated_at
    BEFORE UPDATE ON mangas
    FOR EACH ROW
    EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER manga_localizations_set_updated_at
    BEFORE UPDATE ON manga_localizations
    FOR EACH ROW
    EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER manga_covers_set_updated_at
    BEFORE UPDATE ON manga_covers
    FOR EACH ROW
    EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER manga_volumes_set_updated_at
    BEFORE UPDATE ON manga_volumes
    FOR EACH ROW
    EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER manga_source_syncs_set_updated_at
    BEFORE UPDATE ON manga_source_syncs
    FOR EACH ROW
    EXECUTE FUNCTION set_updated_at();
