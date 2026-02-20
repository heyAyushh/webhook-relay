FROM rust:1.92-alpine3.22 AS builder

WORKDIR /app
RUN apk add --no-cache musl-dev pkgconfig

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo build --release

FROM alpine:3.22

RUN addgroup -S relay \
    && adduser -S -G relay relay \
    && apk add --no-cache ca-certificates curl

WORKDIR /srv
COPY --from=builder /app/target/release/webhook-relay /usr/local/bin/webhook-relay

RUN mkdir -p /var/lib/webhook-relay \
    && chown -R relay:relay /var/lib/webhook-relay

USER relay

ENV WEBHOOK_BIND_ADDR=0.0.0.0:9000 \
    WEBHOOK_DB_PATH=/var/lib/webhook-relay/relay.redb \
    RUST_LOG=info

EXPOSE 9000
VOLUME ["/var/lib/webhook-relay"]

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD curl -fsS http://127.0.0.1:9000/health || exit 1

ENTRYPOINT ["/usr/local/bin/webhook-relay"]
