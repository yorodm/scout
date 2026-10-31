FROM rust:1-slim-bookworm AS backend-build
WORKDIR /src

# Cache dependency compilation separately from the application sources.
COPY Cargo.toml Cargo.lock ./
COPY ui ./ui
RUN mkdir -p src && printf 'fn main() {}' > src/main.rs
RUN cargo build --release --package scout

COPY src ./src
RUN touch src/main.rs && cargo build --release --package scout

FROM rust:1-slim-bookworm AS ui-build
ARG TRUNK_VERSION=0.22.1
WORKDIR /src

RUN apt-get update \
    && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && curl -sSL "https://github.com/trunk-rs/trunk/releases/download/v${TRUNK_VERSION}/trunk-x86_64-unknown-linux-gnu.tar.gz" \
        | tar -xzf - -C /usr/local/bin \
    && rustup target add wasm32-unknown-unknown

COPY Cargo.toml Cargo.lock ./
COPY ui ./ui
RUN mkdir -p src && printf 'fn main() {}' > src/main.rs
RUN cd ui && trunk build --release

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 scout

COPY --from=backend-build /src/target/release/scout /usr/local/bin/scout
COPY --from=ui-build /src/ui/dist /app/ui/dist

USER scout
ENV PORT=8080 \
    RUST_LOG=info \
    SCOUT_UI_DIR=/app/ui/dist
EXPOSE 8080
CMD ["scout"]
