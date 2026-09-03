# docling.rs container images: the `docling-serve` HTTP conversion API and the
# `docling-rs` CLI, built from one Dockerfile as multiple targets over shared
# layers (same dependency compile, same runtime base, same ML assets).
#
# Standard CPU targets (built on Debian Trixie):
#   docker build -f crates/docling-serve/Dockerfile --target serve -t docling-rs-serve .
#   docker build -f crates/docling-serve/Dockerfile --target cli   -t docling-rs .
# (`serve` is the default CPU target if --target is omitted.)
#
# NVIDIA CUDA 12 GPU targets (built with --features cuda on Ubuntu 24.04 + cuDNN 9):
#   docker build -f crates/docling-serve/Dockerfile --target serve-cuda -t docling-rs-serve-cuda .
#   docker build -f crates/docling-serve/Dockerfile --target cli-cuda   -t docling-rs-cuda .
#
# CLI image: the entrypoint is `docling-rs`, the working directory /data — mount
# the documents there and pass the usual flags:
#   docker run --rm -v "$PWD:/data" docling-rs report.pdf --to md
#   docker run --rm -v "$PWD:/data" docling-rs --input in --output out --to latex
#
# Run. The ML assets (ONNX models + pdfium) are fetched into the image by
# `download_dependencies.sh` at build time; to keep the image slim instead,
# build with `--build-arg FETCH_ASSETS=0` and mount them:
#   docker run -p 5001:5001 docling-rs-serve
#   docker run -p 5001:5001 -v ./models:/app/.models -v ./.pdfium:/app/.pdfium docling-rs-serve
#
# GPU run: pass --gpus all to expose the host NVIDIA GPU:
#   docker run --gpus all -p 5001:5001 docling-rs-serve-cuda
#   docker run --gpus all --rm -v "$PWD:/data" docling-rs-cuda report.pdf --to md
#
# The server binds 0.0.0.0 inside the container (the container boundary is the
# network policy); URL fetching is enabled — front with a policy proxy or add
# --no-url-fetch to CMD if the service should accept uploads only.
#
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

# ------------------------------------------------------------------------------
# Stage: chef & planner (shared across CPU and CUDA builds)
# ------------------------------------------------------------------------------
FROM rust:1.96-trixie AS chef
RUN cargo install cargo-chef --locked
WORKDIR /src

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ------------------------------------------------------------------------------
# Stage: CPU build
# ------------------------------------------------------------------------------
FROM chef AS build
COPY --from=planner /src/recipe.json recipe.json
# Same profile/packages/flags as the real build below, or cargo recompiles.
RUN cargo chef cook --release -p docling-serve -p docling-cli --locked --recipe-path recipe.json
COPY . .
RUN cargo build --release -p docling-serve -p docling-cli --locked

# ------------------------------------------------------------------------------
# Stage: CUDA build (Linux x86_64, ort/cuda + copy-dylibs on Ubuntu 24.04 devel)
#
# Ubuntu 24.04 (glibc 2.39 + GCC 14) matches the runtime-cuda base below (#333),
# avoiding glibc version mismatches between build and runtime environments.
# ------------------------------------------------------------------------------
FROM nvidia/cuda:12.6.2-cudnn-devel-ubuntu24.04 AS chef-cuda
RUN apt-get update && apt-get install -y --no-install-recommends \
    curl build-essential pkg-config ca-certificates \
    && rm -rf /var/lib/apt/lists/*
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.96.0 --profile minimal \
    && cargo install cargo-chef --locked
WORKDIR /src

FROM chef-cuda AS build-cuda
COPY --from=planner /src/recipe.json recipe.json
# Build dependency tree with --features cuda so ONNX Runtime with CUDA is fetched and cooked.
RUN cargo chef cook --release -p docling-serve -p docling-cli --features cuda --locked --recipe-path recipe.json
COPY . .
RUN cargo build --release -p docling-serve -p docling-cli --features cuda --locked \
    && mkdir -p /src/cuda-dylibs \
    && test $(find target/release -maxdepth 1 -name "libonnxruntime_providers_*.so" | wc -l) -ge 2 \
    && find target/release -maxdepth 1 -name "libonnxruntime_providers_*.so" -exec cp -av {} /src/cuda-dylibs/ \;
# ------------------------------------------------------------------------------
# Stage: assets (models + pdfium, shared across CPU and CUDA)
# ------------------------------------------------------------------------------
FROM debian:trixie-slim AS assets
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY scripts/install/download_dependencies.sh scripts/install/download_dependencies.sh
ARG FETCH_ASSETS=1
RUN if [ "$FETCH_ASSETS" = "1" ]; then sh scripts/install/download_dependencies.sh; \
    else mkdir -p .models .pdfium; fi

# ------------------------------------------------------------------------------
# Stage: runtime (CPU base, Debian Trixie slim)
# ------------------------------------------------------------------------------
FROM debian:trixie-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates ffmpeg curl \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=assets /src/.models ./.models
COPY --from=assets /src/.pdfium ./.pdfium
ENV PDFIUM_DYNAMIC_LIB_PATH=/app/.pdfium/lib

# ------------------------------------------------------------------------------
# Stage: runtime-cuda (CUDA 12 + cuDNN 9 base on Ubuntu 24.04)
# ------------------------------------------------------------------------------
FROM nvidia/cuda:12.6.2-cudnn-runtime-ubuntu24.04 AS runtime-cuda
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates ffmpeg curl \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=assets /src/.models ./.models
COPY --from=assets /src/.pdfium ./.pdfium
COPY --from=build-cuda /src/cuda-dylibs/ /usr/local/lib/
ENV PDFIUM_DYNAMIC_LIB_PATH=/app/.pdfium/lib \
    LD_LIBRARY_PATH=/usr/local/lib:/app/.pdfium/lib:${LD_LIBRARY_PATH:-} \
    DOCLING_RS_EP=auto

# ==============================================================================
# Final targets: CPU
# ==============================================================================

# --- ghcr.io/docling-project/docling-rs: the CLI (CPU) ------------------------
FROM runtime AS cli
COPY --from=build /src/target/release/docling-rs /usr/local/bin/docling-rs
# The CLI runs from /data (the user's mount), so `.models/…` cannot resolve
# CWD-relative; point the whole-directory override (#285) at the baked assets.
ENV DOCLING_RS_MODELS_DIR=/app/.models
WORKDIR /data
ENTRYPOINT ["docling-rs"]

# --- ghcr.io/docling-project/docling-rs-serve: the HTTP API (CPU) -------------
FROM runtime AS serve
COPY --from=build /src/target/release/docling-serve /usr/local/bin/docling-serve
# Assets resolve relative to the working directory (`.models/`, `.pdfium/lib`).
EXPOSE 5001
HEALTHCHECK --interval=15s --timeout=5s --start-period=45s --retries=3 \
    CMD curl -f http://localhost:5001/ready || exit 1
# Liveness: GET /health; readiness: GET /ready (503 until --warmup finishes).
CMD ["docling-serve", "--addr", "0.0.0.0:5001", "--warmup"]

# ==============================================================================
# Final targets: CUDA GPU
# ==============================================================================

# --- ghcr.io/docling-project/docling-rs-cuda: the CLI (CUDA) ------------------
FROM runtime-cuda AS cli-cuda
COPY --from=build-cuda /src/target/release/docling-rs /usr/local/bin/docling-rs
ENV DOCLING_RS_MODELS_DIR=/app/.models
WORKDIR /data
ENTRYPOINT ["docling-rs"]

# --- ghcr.io/docling-project/docling-rs-serve-cuda: the HTTP API (CUDA) --------
FROM runtime-cuda AS serve-cuda
COPY --from=build-cuda /src/target/release/docling-serve /usr/local/bin/docling-serve
EXPOSE 5001
HEALTHCHECK --interval=15s --timeout=5s --start-period=45s --retries=3 \
    CMD curl -f http://localhost:5001/ready || exit 1
CMD ["docling-serve", "--addr", "0.0.0.0:5001", "--warmup"]
