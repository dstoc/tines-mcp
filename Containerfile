FROM rust:1.98.1-slim-bookworm AS build

WORKDIR /build

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY src ./src

RUN cargo build --release --locked

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=build /build/target/release/tines-mcp /usr/local/bin/tines-mcp

EXPOSE 8080
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/tines-mcp"]
