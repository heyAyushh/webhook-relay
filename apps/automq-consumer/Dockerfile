FROM rust:1.92-alpine3.22 AS builder

WORKDIR /app
RUN apk add --no-cache musl-dev pkgconfig cmake make gcc g++ perl

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY apps ./apps

RUN cargo build --release -p automq-consumer

FROM alpine:3.22

RUN addgroup -S automq \
    && adduser -S -G automq automq \
    && apk add --no-cache ca-certificates

COPY --from=builder /app/target/release/automq-consumer /usr/local/bin/automq-consumer

USER automq
ENTRYPOINT ["/usr/local/bin/automq-consumer"]
