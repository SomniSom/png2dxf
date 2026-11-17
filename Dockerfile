# syntax=docker/dockerfile:1

# ---------- build ----------
FROM rust:1.99-slim AS builder
WORKDIR /src

# Сначала только манифесты: слой зависимостей кэшируется между сборками.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && printf 'fn main(){}' > src/main.rs \
 && cargo build --release --locked \
 && rm -rf src target/release/.fingerprint/png2dxf-*

COPY src ./src
RUN cargo build --release --locked

# ---------- runtime ----------
FROM debian:bookworm-slim

RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
      tesseract-ocr tesseract-ocr-rus tesseract-ocr-eng \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --uid 10001 --create-home --shell /usr/sbin/nologin app

COPY --from=builder /src/target/release/png2dxf /usr/local/bin/png2dxf

ENV MODE=serve \
    OCR_LANG=rus \
    LISTEN_ADDR=0.0.0.0:8080 \
    WORKERS=2 \
    MAX_BODY_MB=64 \
    MAX_BATCH=8

USER 10001
EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD curl -fsS http://127.0.0.1:8080/healthz || exit 1

ENTRYPOINT ["/usr/local/bin/png2dxf"]
CMD ["serve"]
