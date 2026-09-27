CREATE TABLE request_metrics_hourly (
    bucket_start TIMESTAMPTZ PRIMARY KEY,
    request_count BIGINT NOT NULL DEFAULT 0 CHECK (request_count >= 0),
    mangadex_attempt_count BIGINT NOT NULL DEFAULT 0 CHECK (mangadex_attempt_count >= 0),
    mangadex_success_count BIGINT NOT NULL DEFAULT 0 CHECK (mangadex_success_count >= 0),
    mangadex_failure_count BIGINT NOT NULL DEFAULT 0 CHECK (mangadex_failure_count >= 0)
);
