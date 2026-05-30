# Sample Dockerfile to exercise syntax highlighting, indents,
# and folds in vorto. Open with `vorto assets/samples/Dockerfile`.

FROM rust:1-slim AS builder

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --release

FROM debian:stable-slim

LABEL org.opencontainers.image.title="hello" \
      org.opencontainers.image.description="A tiny greeting program"

ENV GREETING="Hello" \
    RUST_LOG="info"

WORKDIR /opt/hello
COPY --from=builder /app/target/release/hello ./

EXPOSE 8080
USER 1000:1000

ENTRYPOINT ["./hello"]
CMD ["--greeting", "Hello"]
