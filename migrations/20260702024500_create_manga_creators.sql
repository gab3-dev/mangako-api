CREATE TABLE creators (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    mangadex_id UUID NOT NULL UNIQUE,
    name TEXT NOT NULL,
    image_url TEXT,
    mangadex_version INTEGER,
    source_updated_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    CONSTRAINT creators_name_not_blank CHECK (length(trim(name)) > 0)
);

CREATE TABLE manga_creators (
    manga_id UUID NOT NULL REFERENCES mangas(id) ON DELETE CASCADE,
    creator_id UUID NOT NULL REFERENCES creators(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (manga_id, creator_id, role),
    CONSTRAINT manga_creators_role_valid CHECK (role IN ('author', 'artist'))
);

CREATE INDEX creators_name_trgm_idx ON creators USING gin (name gin_trgm_ops);
CREATE INDEX creators_updated_at_idx ON creators (updated_at);
CREATE INDEX manga_creators_creator_id_idx ON manga_creators (creator_id);
CREATE INDEX manga_creators_role_idx ON manga_creators (role);

CREATE TRIGGER creators_set_updated_at
    BEFORE UPDATE ON creators
    FOR EACH ROW
    EXECUTE FUNCTION set_updated_at();
