# Servitor Dockerfile - Build from source

FROM rust:1.85-bookworm AS builder

WORKDIR /build

# Copy everything and build
COPY . .
RUN cargo build --release

# Runtime image
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/servitor /usr/local/bin/servitor

# Create data directory
RUN mkdir -p /var/lib/servitor

VOLUME /var/lib/servitor

EXPOSE 8765 9091

ENTRYPOINT ["servitor"]
CMD ["-c", "/etc/servitor/servitor.toml", "run"]
