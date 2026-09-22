FROM rust:1.95-slim AS builder

WORKDIR /app
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY migrations ./migrations
COPY src ./src
RUN cargo build --release

FROM debian:trixie-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && groupadd --system --gid 10001 mangako \
    && useradd --system --uid 10001 --gid mangako --home-dir /nonexistent --shell /usr/sbin/nologin mangako \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder --chown=mangako:mangako /app/target/release/mangako-api /usr/local/bin/mangako-api

ENV HTTP_ADDR=0.0.0.0:3000
EXPOSE 3000
USER 10001:10001

CMD ["mangako-api"]
