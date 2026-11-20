# syntax=docker/dockerfile:1

FROM rust:1.89-bookworm AS builder
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release --locked -p muninn-aether-vault --bin aether-mcp

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates git \
  && rm -rf /var/lib/apt/lists/*

RUN useradd --create-home --home-dir /home/aether --shell /usr/sbin/nologin aether \
  && mkdir -p /vault \
  && chown -R aether:aether /vault

COPY --from=builder /src/target/release/aether-mcp /usr/local/bin/aether-mcp

USER aether
WORKDIR /vault
VOLUME ["/vault"]
EXPOSE 4771

ENTRYPOINT ["aether-mcp"]
CMD ["--vault", "/vault", "--http-addr", "0.0.0.0:4771", "--http-only"]
