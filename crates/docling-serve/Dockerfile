# docling-serve — container image for the HTTP conversion API.
#
# Build (from the repository root):
#   docker build -f crates/docling-serve/Dockerfile -t docling-serve .
#
# Run. The ML assets (ONNX models + pdfium) are fetched into the image by
# `download_dependencies.sh` at build time; to keep the image slim instead,
# build with `--build-arg FETCH_ASSETS=0` and mount them:
#   docker run -p 5001:5001 docling-serve
#   docker run -p 5001:5001 -v ./models:/app/models -v ./.pdfium:/app/.pdfium docling-serve
#
# The server binds 0.0.0.0 inside the container (the container boundary is the
# network policy); URL fetching is enabled — front with a policy proxy or add
# --no-url-fetch to CMD if the service should accept uploads only.

# trixie, not bookworm (#246): ort's prebuilt static onnxruntime is built
# with GCC 13 and references GLIBCXX_3.4.31 — bookworm's libstdc++ (GCC 12)
# tops out at 3.4.30, so the release link fails with undefined references.
# The runtime stage below must stay on the same Debian release: the binary
# links that libstdc++ (and trixie's glibc) dynamically.
#
# Layering for CI cache hits (docker-publish.yml exports every stage to the
# GitHub Actions cache): a plain `COPY . . && cargo build` invalidates the
# whole dependency compile on any commit, so the ~10 min build ran from
# scratch on every PR and push (`Cached 0%`). cargo-chef splits it — the
# `planner` stage reduces the workspace to its manifests (recipe.json), and
# `cook` compiles only the dependency graph from that recipe, so that layer
# is reused until Cargo.toml/Cargo.lock change. A source change then rebuilds
# just the workspace crates. The ML assets are fetched in their own stage
# that depends on nothing but the download script, so ~900 MB of models are
# not re-downloaded per build either.
FROM rust:1.96-trixie AS chef
RUN cargo install cargo-chef --locked
WORKDIR /src

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
COPY --from=planner /src/recipe.json recipe.json
# Same profile/package/flags as the real build below, or cargo recompiles.
RUN cargo chef cook --release -p docling-serve --locked --recipe-path recipe.json
COPY . .
RUN cargo build --release -p docling-serve --locked

FROM debian:trixie-slim AS assets
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY scripts/install/download_dependencies.sh scripts/install/download_dependencies.sh
ARG FETCH_ASSETS=1
RUN if [ "$FETCH_ASSETS" = "1" ]; then sh scripts/install/download_dependencies.sh; \
    else mkdir -p .models .pdfium; fi

FROM debian:trixie-slim
# ffmpeg powers video frame sampling (#138 Phase 2); without it a video input
# still converts, transcript-only.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates ffmpeg curl \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /src/target/release/docling-serve /usr/local/bin/docling-serve
# Assets resolve relative to the working directory (`.models/`, `.pdfium/lib`).
COPY --from=assets /src/.models ./.models
COPY --from=assets /src/.pdfium ./.pdfium
ENV PDFIUM_DYNAMIC_LIB_PATH=/app/.pdfium/lib
EXPOSE 5001
HEALTHCHECK --interval=15s --timeout=5s --start-period=45s --retries=3 \
    CMD curl -f http://localhost:5001/ready || exit 1
# Liveness: GET /health; readiness: GET /ready (503 until --warmup finishes).
CMD ["docling-serve", "--addr", "0.0.0.0:5001", "--warmup"]
