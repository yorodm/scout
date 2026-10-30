FROM rust:1-slim-bookworm AS build
WORKDIR /src

# Cache dependency compilation separately from the application sources.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && printf 'fn main() {}' > src/main.rs
RUN cargo build --release

COPY src ./src
COPY web ./web
RUN touch src/main.rs && cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 scout

COPY --from=build /src/target/release/scout /usr/local/bin/scout

USER scout
ENV PORT=8080 \
    RUST_LOG=info
EXPOSE 8080
CMD ["scout"]
