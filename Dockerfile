ARG ELIXIR_VERSION=1.17.3
ARG OTP_VERSION=27.1.2
ARG DEBIAN_VERSION=bookworm-20240904-slim

ARG BUILDER_IMAGE="hexpm/elixir:${ELIXIR_VERSION}-erlang-${OTP_VERSION}-debian-${DEBIAN_VERSION}"
ARG RUNNER_IMAGE="debian:${DEBIAN_VERSION}"

# --- Build stage ---
FROM ${BUILDER_IMAGE} AS build

RUN apt-get update -y && apt-get install -y build-essential git && rm -rf /var/lib/apt/lists/*

WORKDIR /app

RUN mix local.hex --force && mix local.rebar --force

ENV MIX_ENV=prod

COPY mix.exs mix.lock ./
RUN mix deps.get --only prod && mix deps.compile

COPY config config
COPY lib lib
COPY priv priv

RUN mix compile
RUN mix release

# --- Runtime stage ---
FROM ${RUNNER_IMAGE}

RUN apt-get update -y && \
    apt-get install -y libstdc++6 openssl libncurses5 locales ca-certificates curl && \
    rm -rf /var/lib/apt/lists/*

RUN sed -i '/en_US.UTF-8/s/^# //g' /etc/locale.gen && locale-gen

ENV LANG=en_US.UTF-8
ENV LANGUAGE=en_US:en
ENV LC_ALL=en_US.UTF-8

# Litestream for SQLite replication
COPY --from=litestream/litestream:latest /usr/local/bin/litestream /usr/local/bin/litestream

WORKDIR /app

RUN useradd --create-home app
RUN mkdir -p /data && chown app:app /data

COPY --from=build --chown=app:app /app/_build/prod/rel/canary ./
COPY --chown=app:app litestream.yml /etc/litestream.yml
COPY --chown=app:app bin/entrypoint.sh /app/bin/entrypoint.sh
RUN chmod +x /app/bin/entrypoint.sh

USER app

ENV PHX_SERVER=true

CMD ["/app/bin/entrypoint.sh"]
