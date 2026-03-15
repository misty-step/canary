FROM elixir:1.17-slim AS build

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

FROM debian:bookworm-slim

RUN apt-get update -y && \
    apt-get install -y libstdc++6 openssl libncurses6 locales ca-certificates && \
    rm -rf /var/lib/apt/lists/*

RUN sed -i '/en_US.UTF-8/s/^# //g' /etc/locale.gen && locale-gen

ENV LANG=en_US.UTF-8
WORKDIR /app

RUN useradd --create-home app
COPY --from=build --chown=app:app /app/_build/prod/rel/canary_triage ./

USER app
ENV PHX_SERVER=true

CMD ["/app/bin/canary_triage", "start"]
