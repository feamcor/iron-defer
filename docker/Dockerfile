FROM rust:1.94-slim AS builder

WORKDIR /build

# Copy workspace manifest and lock file first for dependency caching
COPY Cargo.toml Cargo.lock ./

# Copy all crate manifests
COPY crates/domain/Cargo.toml crates/domain/Cargo.toml
COPY crates/application/Cargo.toml crates/application/Cargo.toml
COPY crates/infrastructure/Cargo.toml crates/infrastructure/Cargo.toml
COPY crates/api/Cargo.toml crates/api/Cargo.toml

# Create stub sources so cargo can resolve the workspace and cache dependencies
RUN mkdir -p crates/domain/src && echo "pub fn _stub() {}" > crates/domain/src/lib.rs && \
    mkdir -p crates/application/src && echo "pub fn _stub() {}" > crates/application/src/lib.rs && \
    mkdir -p crates/infrastructure/src && echo "pub fn _stub() {}" > crates/infrastructure/src/lib.rs && \
    mkdir -p crates/api/src && echo "pub fn _stub() {}" > crates/api/src/lib.rs && \
    echo "fn main() {}" > crates/api/src/main.rs

ENV SQLX_OFFLINE=true

# Build dependencies only (cached layer)
RUN cargo build --release || true

# Copy .cargo/config.toml for build aliases and future build settings
COPY .cargo/config.toml /build/.cargo/config.toml

# Copy .sqlx offline query cache, migrations, and real source code
COPY .sqlx/ .sqlx/
COPY migrations/ migrations/
COPY crates/ crates/

# Rebuild with real sources (dependencies already cached)
RUN touch crates/domain/src/lib.rs \
         crates/application/src/lib.rs \
         crates/infrastructure/src/lib.rs \
         crates/api/src/lib.rs \
         crates/api/src/main.rs && \
    cargo build --release

FROM gcr.io/distroless/cc-debian12

COPY --from=builder /build/target/release/iron-defer /iron-defer

EXPOSE 8080

ENTRYPOINT ["/iron-defer"]
