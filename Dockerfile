FROM rust:1.92-alpine3.22 AS builder

WORKDIR /app
RUN apk add --no-cache musl-dev pkgconfig cmake make gcc g++ perl

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY src ./src
COPY apps ./apps

RUN cargo build --release -p webhook-relay

FROM alpine:3.22

RUN addgroup -S relay \
    && adduser -S -G relay relay \
    && apk add --no-cache ca-certificates curl

COPY --from=builder /app/target/release/webhook-relay /usr/local/bin/webhook-relay

USER relay

ENV RELAY_BIND=0.0.0.0:8080 \
    RELAY_MAX_PAYLOAD_BYTES=1048576 \
    RELAY_IP_RATE_PER_MINUTE=100 \
    RELAY_SOURCE_RATE_PER_MINUTE=500 \
    RUST_LOG=info

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD curl -fsS http://127.0.0.1:8080/health || exit 1

ENTRYPOINT ["/usr/local/bin/webhook-relay"]
