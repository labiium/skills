# Multi-stage Dockerfile for skills.rs
# Build stage
FROM rust:1.92-slim as builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Create app directory
WORKDIR /usr/src/app

# Copy manifests
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

# Copy source code
COPY src ./src
COPY tests ./tests

# Build release binary
RUN cargo build --release --bin skills

# Runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    python3 \
    python3-pip \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -m -u 1000 -s /bin/bash skills

# Create necessary directories
RUN mkdir -p /data/skills /data/cache /data/logs && \
    chown -R skills:skills /data

# Copy binary from builder
COPY --from=builder /usr/src/app/target/release/skills /usr/local/bin/skills

# Copy example config
COPY config.example.yaml /etc/skills/config.yaml

# Set ownership
RUN chown -R skills:skills /etc/skills

# Switch to non-root user
USER skills

# Set working directory
WORKDIR /data

# Expose default port
EXPOSE 8000

# Health check
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD skills --version || exit 1

# Set entrypoint
ENTRYPOINT ["skills"]

# Default command
CMD ["server", "--bind", "0.0.0.0:8000"]
