FROM rust:1.85-bookworm AS builder

WORKDIR /app

COPY Cargo.toml ./
COPY src ./src

RUN git clone --depth 1 https://github.com/the-social-proof-foundation/myso-core /myso-core \
    && sed -i 's|path = "../myso-core/crates/myso-sdk"|path = "/myso-core/crates/myso-sdk"|' Cargo.toml \
    && sed -i 's|path = "../myso-core/crates/shared-crypto"|path = "/myso-core/crates/shared-crypto"|' Cargo.toml

RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/myso-identity-verification /app/

RUN useradd -m -u 1001 appuser && chown -R appuser:appuser /app
USER appuser

EXPOSE 3000

HEALTHCHECK --interval=30s --timeout=3s --start-period=60s --retries=5 \
    CMD sh -c 'curl -f "http://127.0.0.1:${PORT:-3000}/health" || exit 1'

CMD ["./myso-identity-verification"]
