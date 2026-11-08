# Rift Online: the axum server plus the WebAssembly client it serves.
#
#   docker build -t rift-online .
#   docker run -p 8080:8080 -v rift-data:/data rift-online
#
# Persistent state (accounts database, card image cache) lives in /data.

ARG RUST_VERSION=1.99.1
ARG WASM_BINDGEN_VERSION=0.2.130

FROM rust:${RUST_VERSION}-slim-bookworm AS build
ARG WASM_BINDGEN_VERSION
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && rustup target add wasm32-unknown-unknown
# wasm-bindgen-cli must match the wasm-bindgen crate in Cargo.lock exactly.
RUN curl -fsSL "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/${WASM_BINDGEN_VERSION}/wasm-bindgen-${WASM_BINDGEN_VERSION}-x86_64-unknown-linux-musl.tar.gz" \
    | tar -xz -C /usr/local/bin --strip-components=1 "wasm-bindgen-${WASM_BINDGEN_VERSION}-x86_64-unknown-linux-musl/wasm-bindgen" \
    && wasm-bindgen --version
WORKDIR /src
COPY . .
# build.sh compiles the client to dist/ and the release server binary.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    ./build.sh \
    && cp target/release/rift-server /rift-server

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /rift-server /app/rift-server
COPY --from=build /src/dist /app/dist
COPY --from=build /src/data/cards.json /app/data/cards.json
ENV RIFT_BIND=0.0.0.0:8080 \
    RIFT_DATA=/app/data \
    RIFT_DIST=/app/dist \
    RIFT_ACCOUNTS_DB=/data/accounts.db \
    RIFT_IMG_CACHE=/data/imgcache
VOLUME ["/data"]
EXPOSE 8080
USER nonroot:nonroot
ENTRYPOINT ["/app/rift-server"]
