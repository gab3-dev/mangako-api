-- Server-only OAuth state. No catalog endpoint exposes this table.
CREATE TABLE mercadolivre_oauth_tokens (
    client_id TEXT PRIMARY KEY,
    access_token TEXT NOT NULL,
    refresh_token TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    retry_after TIMESTAMPTZ,
    reauthorization_required BOOLEAN NOT NULL DEFAULT false
);
