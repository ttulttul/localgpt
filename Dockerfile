FROM rust:1.85-bookworm AS builder

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY ui ./ui

RUN cargo build --release --locked

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates bash git ripgrep \
    && rm -rf /var/lib/apt/lists/*

RUN groupadd --system --gid 10001 localgpt \
    && useradd --system --uid 10001 --gid localgpt --create-home --home-dir /home/localgpt localgpt \
    && mkdir -p /home/localgpt/.localgpt /home/localgpt/.cache/localgpt \
    && chown -R localgpt:localgpt /home/localgpt

COPY --from=builder /app/target/release/localgpt /usr/local/bin/localgpt

USER localgpt:localgpt
WORKDIR /home/localgpt

ENTRYPOINT ["localgpt"]
