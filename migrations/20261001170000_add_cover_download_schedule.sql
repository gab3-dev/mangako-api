CREATE TABLE cover_download_schedule (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    next_download_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO cover_download_schedule (id) VALUES (TRUE);
