FROM rust:1.94.0-bookworm@sha256:365468470075493dc4583f47387001854321c5a8583ea9604b297e67f01c5a4f AS build

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY priv priv

RUN cargo build --release --locked -p canary-server

FROM debian:bookworm-slim

RUN apt-get update -y && \
    apt-get install -y --no-install-recommends ca-certificates curl && \
    rm -rf /var/lib/apt/lists/*

COPY --from=litestream/litestream:latest /usr/local/bin/litestream /usr/local/bin/litestream

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
