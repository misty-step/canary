FROM rust:1.94.0-bookworm@sha256:365468470075493dc4583f47387001854321c5a8583ea9604b297e67f01c5a4f AS build

# Deploy passes the CI runner's `git describe` output (a real tag, or a real
# tag plus the commits/sha ahead of it — see .github/workflows/deploy.yml).
# Builds outside that pipeline (this default, dagger's image smoke test)
# report the honest, recognizable "0.0.0-dev" dev version.
ARG CANARY_VERSION=0.0.0-dev
ENV CANARY_VERSION=${CANARY_VERSION}

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY priv priv

RUN cargo build --release --locked -p canary-server

FROM debian:bookworm-slim

RUN apt-get update -y && \
    apt-get install -y --no-install-recommends ca-certificates curl && \
    rm -rf /var/lib/apt/lists/*

COPY --from=litestream/litestream@sha256:5572700ba18710cb010a0e415e36abf5cc0b4d74a2ad7b6d6a387142c0c99604 /usr/local/bin/litestream /usr/local/bin/litestream

WORKDIR /app

RUN useradd --create-home app && \
    mkdir -p /app/bin /data && \
    chown -R app:app /app /data

COPY --from=build --chown=app:app /app/target/release/canary-server /app/bin/canary-server
COPY --chown=app:app litestream.yml /etc/litestream.yml
COPY --chown=app:app bin/entrypoint.sh /app/bin/entrypoint.sh
RUN chmod +x /app/bin/entrypoint.sh /app/bin/canary-server

USER app

ENV CANARY_DB_PATH=/data/canary.db
ENV PORT=4000

CMD ["/app/bin/entrypoint.sh"]
