ARG RUST_VERSION=1.97
FROM rust:${RUST_VERSION}-slim-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential pkg-config libssl-dev ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /usr/src/app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY README.md ./README.md
RUN cargo build --locked --release -p skillsrs --bin skills

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 libstdc++6 python3 nodejs bash git curl bubblewrap \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 --shell /bin/bash skills \
    && mkdir -p /data/skills /data/cache /data/logs /etc/skills \
    && chown -R skills:skills /data
COPY --from=builder /usr/src/app/target/release/skills /usr/local/bin/skills
COPY docs/config.example.yaml /etc/skills/config.yaml

USER skills
WORKDIR /data
ENV SKILLS_PATH=/data
EXPOSE 8000
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \
    CMD curl --fail --silent --show-error --header "Authorization: Bearer ${SKILLS_HTTP_TOKEN}" http://127.0.0.1:8000/health || exit 1
ENTRYPOINT ["skills", "--config", "/etc/skills/config.yaml", "--path", "/data"]
CMD ["server", "http", "--bind", "0.0.0.0:8000"]
