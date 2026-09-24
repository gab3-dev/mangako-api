CREATE TABLE cover_assets (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_url TEXT UNIQUE,
    storage_key TEXT UNIQUE,
    status TEXT NOT NULL DEFAULT 'pending',
    content_type TEXT,
    byte_size BIGINT,
    width INTEGER,
    height INTEGER,
    sha256 TEXT,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    locked_until TIMESTAMPTZ,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT cover_assets_status_valid CHECK (status IN ('pending', 'ready', 'failed')),
    CONSTRAINT cover_assets_source_or_storage CHECK (source_url IS NOT NULL OR storage_key IS NOT NULL),
    CONSTRAINT cover_assets_size_valid CHECK (byte_size IS NULL OR byte_size > 0)
);

ALTER TABLE manga_covers ADD COLUMN asset_id UUID REFERENCES cover_assets(id) ON DELETE SET NULL;
ALTER TABLE manga_volumes ADD COLUMN asset_id UUID REFERENCES cover_assets(id) ON DELETE SET NULL;
ALTER TABLE manga_volumes ADD COLUMN storage_key TEXT;
ALTER TABLE manga_volumes ALTER COLUMN source_url DROP NOT NULL;
ALTER TABLE manga_volumes DROP CONSTRAINT manga_volumes_source_url_not_blank;
ALTER TABLE manga_volumes ADD CONSTRAINT manga_volumes_has_location
    CHECK (source_url IS NOT NULL OR storage_key IS NOT NULL OR asset_id IS NOT NULL);

CREATE INDEX cover_assets_pending_idx
    ON cover_assets (next_attempt_at)
    WHERE status IN ('pending', 'failed');
CREATE INDEX manga_covers_asset_id_idx ON manga_covers (asset_id) WHERE asset_id IS NOT NULL;
CREATE INDEX manga_volumes_asset_id_idx ON manga_volumes (asset_id) WHERE asset_id IS NOT NULL;

CREATE TRIGGER cover_assets_set_updated_at
    BEFORE UPDATE ON cover_assets
    FOR EACH ROW
    EXECUTE FUNCTION set_updated_at();
