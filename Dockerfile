# ---- build stage -----------------------------------------------------------
FROM rust:1-slim AS builder
WORKDIR /app

# Warm the dependency layer: build with stub sources so `cargo` caches all
# crates.io deps, then copy the real sources. Every workspace member needs a
# stub or cargo refuses to resolve and the layer caches nothing — including
# fl-server's second binary, the optional GUI.
COPY Cargo.toml Cargo.lock ./
COPY crates/fl-fits/Cargo.toml crates/fl-fits/Cargo.toml
COPY crates/fl-index/Cargo.toml crates/fl-index/Cargo.toml
COPY crates/fl-extract/Cargo.toml crates/fl-extract/Cargo.toml
COPY crates/fl-solve/Cargo.toml crates/fl-solve/Cargo.toml
COPY crates/fl-sky/Cargo.toml crates/fl-sky/Cargo.toml
COPY crates/fl-server/Cargo.toml crates/fl-server/Cargo.toml
RUN mkdir -p crates/fl-fits/src crates/fl-index/src crates/fl-extract/src \
        crates/fl-solve/src crates/fl-sky/src crates/fl-server/src/bin \
    && echo "" > crates/fl-fits/src/lib.rs \
    && echo "" > crates/fl-index/src/lib.rs \
    && echo "" > crates/fl-extract/src/lib.rs \
    && echo "" > crates/fl-solve/src/lib.rs \
    && echo "" > crates/fl-sky/src/lib.rs \
    && echo "" > crates/fl-server/src/lib.rs \
    && echo "fn main() {}" > crates/fl-server/src/main.rs \
    && echo "fn main() {}" > crates/fl-server/src/bin/gui.rs \
    && cargo build --release -p fl-server 2>/dev/null || true

# The image ships the headless server; the GUI (`--features gui`) is a desktop
# build and pulls in a C++ toolchain, so it stays out of the container.
COPY VERSION ./
COPY crates ./crates
RUN touch crates/*/src/lib.rs crates/fl-server/src/main.rs \
    && cargo build --release -p fl-server

# ---- runtime stage ----------------------------------------------------------
FROM debian:bookworm-slim
COPY --from=builder /app/target/release/faint-light /usr/local/bin/faint-light

# Mount your astrometry.net index files here (index-*.fits).
VOLUME /index
ENV FAINT_LIGHT_INDEX_DIR=/index
EXPOSE 7222

USER nobody
CMD ["faint-light"]
