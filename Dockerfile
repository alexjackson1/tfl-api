# ---------- Build stage ----------
FROM rust:1.91.1-slim AS builder

# Create app directory
WORKDIR /app

# Install build dependencies
RUN apt-get update && \
    apt-get install -y --no-install-recommends build-essential pkg-config libssl-dev && \
    rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

# ---------- Runtime stage ----------
FROM debian:bookworm-slim AS runtime

# Create non-root user
RUN useradd -m appuser

WORKDIR /app

# Install minimal runtime deps (ca-certificates for TLS to TfL)
RUN apt-get update && \
  apt-get install -y --no-install-recommends ca-certificates && \
  rm -rf /var/lib/apt/lists/*

# Copy compiled binary from builder stage
COPY --from=builder /app/target/release/tfl-api /usr/local/bin/tfl-api

# Rocket config: bind on all interfaces inside container
ENV ROCKET_ADDRESS=0.0.0.0
ENV ROCKET_PORT=8000

ENV TFL_STOP_ID=""
ENV TFL_APP_ID=""
ENV TFL_APP_KEY=""

EXPOSE 8000

USER appuser

CMD ["tfl-api"]
