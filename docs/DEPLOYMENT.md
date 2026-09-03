# Deploying `docling-serve` in Containers

This guide covers running the [`docling-serve`](../crates/docling-serve) HTTP conversion service using Docker, Docker Compose, and Kubernetes/OpenShift.

---

## Container Registry & Prebuilt Images

The following container images are published on **GitHub Container Registry (GHCR)** on every release and master commit:

#### 📦 Distributed Images

| Image | Description | Architectures |
|---|---|---|
| [`ghcr.io/docling-project/docling-rs-serve`](https://github.com/docling-project/docling.rs/pkgs/container/docling-rs-serve) | High-performance document conversion HTTP API with PDF, DOCX, PPTX, XLSX, HTML, images, and audio/video models pre-installed (zero Python runtime dependencies, CPU). | `linux/amd64`, `linux/arm64` |
| [`ghcr.io/docling-project/docling-rs`](https://github.com/docling-project/docling.rs/pkgs/container/docling-rs) | The `docling-rs` CLI with the same models baked in: batch conversion of local files without installing Rust (CPU). Entrypoint `docling-rs`, working directory `/data`. | `linux/amd64`, `linux/arm64` |
| [`ghcr.io/docling-project/docling-rs-serve-cuda`](https://github.com/docling-project/docling.rs/pkgs/container/docling-rs-serve-cuda) | NVIDIA CUDA 12 GPU-accelerated HTTP conversion API (CUDA 12 + cuDNN 9, Linux x86_64). Explicit version tags (`v1.28.0`, `master`), no `:latest`. | `linux/amd64` |
| [`ghcr.io/docling-project/docling-rs-cuda`](https://github.com/docling-project/docling.rs/pkgs/container/docling-rs-cuda) | NVIDIA CUDA 12 GPU-accelerated `docling-rs` CLI. Explicit version tags (`v1.28.0`, `master`), no `:latest`. | `linux/amd64` |
> [!NOTE]
> **Image Naming**: The `-rs` in `docling-rs-serve` / `docling-rs` differentiates this native Rust implementation from the Python `docling-project/docling-serve` image repository on GHCR. Both images are targets of the same [`Dockerfile`](../crates/docling-serve/Dockerfile) and share every layer but the final binary.

```bash
# Pull the docling-serve HTTP API image (Rust engine):
docker pull ghcr.io/docling-project/docling-rs-serve:latest

# Pull the CLI image and convert files from the current directory:
docker pull ghcr.io/docling-project/docling-rs:latest
docker run --rm -v "$PWD:/data" ghcr.io/docling-project/docling-rs:latest report.pdf --to md
docker run --rm -v "$PWD:/data" ghcr.io/docling-project/docling-rs:latest --input docs --output converted --to json
```

### Supported Architectures & Tags

| Architecture | Platform | Notes |
|---|---|---|
| `linux/amd64` | x86-64 | Standard 64-bit Linux |
| `linux/arm64` | ARM64 / aarch64 | Apple Silicon, AWS Graviton, Ampere |

#### Tagging Scheme

- **CPU images (`docling-rs-serve`, `docling-rs`)**:
  - `latest`: Latest release or master build
  - `v1.28.0`, `1.28.0`: Exact release version
  - `1.28`, `1`: Floating major/minor tags
  - `master`: Bleeding-edge master branch build
- **CUDA GPU images (`docling-rs-serve-cuda`, `docling-rs-cuda`)**:
  - `v1.28.0`, `1.28.0`, `1.28`, `1`: Explicit release version tags
  - `master`: Master branch build
  - *(No floating `:latest` tag, following upstream docling-serve CUDA tagging policy)*

---

## Quick Start with Docker

Run the container exposing port `5001`:

```bash
docker run -d \
  --name docling-serve \
  -p 5001:5001 \
  --restart unless-stopped \
  ghcr.io/docling-project/docling-rs-serve:latest
```

Check health:

```bash
curl http://localhost:5001/health
# => {"status":"ok"}
```

Convert a document (PDF, DOCX, PPTX, XLSX, HTML, Images, Audio/Video) to Markdown:

```bash
curl -F file=@document.pdf http://localhost:5001/v1/convert
```

---

## Deployment with Docker Compose

Preconfigured compose files are available in [`examples/docker-compose/`](../examples/docker-compose/):

### Standalone Service (`docker-compose.yml`)

```yaml
services:
  docling-serve:
    image: ghcr.io/docling-project/docling-rs-serve:latest
    container_name: docling-serve
    restart: unless-stopped
    ports:
      # Bind to loopback interface only (127.0.0.1) for unauthenticated access
      - "127.0.0.1:5001:5001"
    environment:
      - DOCLING_RS_NO_ARENA=1
      - DOCLING_RS_MAX_MEMORY_MB=4096
      - DOCLING_RS_MEMORY_WATERMARK_PCT=85
      - RUST_LOG=info
    command:
      - "docling-serve"
      - "--addr"
      - "0.0.0.0:5001"
      - "--concurrency"
      - "2"
      - "--warmup"
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:5001/ready"]
      interval: 15s
      timeout: 5s
      retries: 5
      start_period: 45s
    deploy:
      resources:
        limits:
          cpus: "4.0"
          memory: 4G
        reservations:
          cpus: "1.0"
          memory: 1G
```

Launch with:

```bash
cd examples/docker-compose
docker compose up -d
```

### Production Stack with Caddy Reverse Proxy (`docker-compose.caddy.yml`)

For production setups needing automatic TLS certificates, Basic Auth, or header security:

```bash
cd examples/docker-compose
docker compose -f docker-compose.caddy.yml up -d
```
### NVIDIA CUDA GPU Stack (`docker-compose.cuda.yml`)

For hosts equipped with NVIDIA GPUs and the NVIDIA Container Toolkit:

```bash
cd examples/docker-compose
docker compose -f docker-compose.cuda.yml up -d
```

---
## Configuration & Environment Variables

`docling-serve` is configured via CLI arguments and environment variables:

### CLI Arguments

| Argument | Default | Description |
|---|---|---|
| `--addr HOST:PORT` | `127.0.0.1:5001` | Server bind address (`0.0.0.0:5001` inside container) |
| `--concurrency N` | `2` | Max conversions processed in parallel; excess requests queue |
| `--max-body-mb N` | `256` | Maximum request body size for uploads (MiB) |
| `--queue-size N` | `16` | Maximum async jobs held in queue / unfetched before returning HTTP 429 |
| `--result-ttl SECS` | `600` | Retention duration for finished async job results (seconds) |
| `--max-memory-mb N` | auto | RSS ceiling for admission control (MB); 0 disables |
| `--warmup` | off | Pre-load models at startup; `/ready` returns 503 until complete |
| `--allow-url-fetch` | off | Enable `{"url": "..."}` inputs and remote fetching (SSRF protected) |
| `--strict` | off | Default output to clean strict Markdown dialect |

### Environment Variables

| Variable | Default | Purpose |
|---|---|---|
| `DOCLING_RS_NO_ARENA` | `1` | Disables ONNX Runtime CPU arena to prevent RSS heap ratcheting (#263) |
| `DOCLING_RS_MAX_MEMORY_MB` | `0` (or cgroup) | Memory ceiling for admission control in MiB |
| `DOCLING_RS_MEMORY_WATERMARK_PCT` | `85` | Watermark % above which new requests get HTTP 503 Retry-After |
| `DOCLING_RS_PDF_WORKERS` | CPU count | Worker pool size for concurrent PDF page processing |
| `DOCLING_RS_TF_INTRA` | auto (#262) | Derived from cgroup CPU quota (#262); explicitly narrows ONNX intra-op threads for TableFormer decoder |
| `DOCLING_RS_MAX_RASTER_PAGES` | `100` | Max page count for `to=images` PDF page rasterization |
| `DOCLING_RS_MAX_FETCH_BYTES` | `268435456` (256MB) | Max response size for remote URL fetching |
| `DOCLING_RS_OCR_LANG` | `en` | Default OCR language (`en` or `ch` multilingual) |
| `DOCLING_RS_OCR_MODE` | `default` | OCR region selection (`default`, `full_page`, `layout_regions`) |
| `DOCLING_RS_FP32` | `0` | Force FP32 model precision instead of INT8 |
| `DOCLING_RS_MODELS_DIR` | `.models` | Directory override for ONNX model weights |
| `PDFIUM_DYNAMIC_LIB_PATH` | `/app/.pdfium/lib` | Path to dynamic `libpdfium.so` / `libpdfium.dylib` |
| `DOCLING_FFMPEG` | `ffmpeg` | Path to `ffmpeg` binary for video frame extraction |
| `RUST_LOG` | `info` | Logging verbosity (`error`, `warn`, `info`, `debug`, `trace`) |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | unset | OTLP/gRPC endpoint for distributed trace export (#297) |
| `OTEL_SERVICE_NAME` | `docling-serve` | OpenTelemetry service name |

---

## Resource Sizing & Performance Tuning

1. **Memory Arena & Admission Control (#263)**:
   By default, `DOCLING_RS_NO_ARENA=1` is enabled. Without this, ONNX Runtime's allocator retains mapped heap memory indefinitely. Combined with automatic `malloc_trim` calls, process memory drops ~3× after heavy PDF conversions.
   Setting `DOCLING_RS_MAX_MEMORY_MB=4096` ensures the server sheds load with `503 + Retry-After` when under memory pressure, rather than being OOM-killed.

2. **Concurrency (`--concurrency`)**:
   One warm ML pipeline is shared across concurrent requests. For typical workloads, `--concurrency 2` or `--concurrency 4` provides high throughput without CPU saturation.

3. **CPU Sizing**:
   Allocate at least 2 vCPUs (recommended 4+ vCPUs) for production workloads with heavy scanned PDFs and OCR.

---

## Offline & Air-Gapped Deployment

The container image is fully self-contained by default. If you need to build a custom slim image or mount pre-cached assets from host storage:

### Building with `--build-arg FETCH_ASSETS=0`

```bash
docker build -f crates/docling-serve/Dockerfile --build-arg FETCH_ASSETS=0 -t docling-serve-slim .
```

### Running with Mounted Assets

```bash
docker run -d \
  -p 5001:5001 \
  -v /host/path/.models:/app/.models:ro \
  -v /host/path/.pdfium:/app/.pdfium:ro \
  docling-serve-slim
```

---

## Endpoints & Health Checks

| Endpoint | Method | Description |
|---|---|---|
| `/health` | `GET` | Liveness probe: returns `{"status":"ok"}` immediately |
| `/ready` | `GET` | Readiness probe: returns `200` once models are pre-warmed, `503` during startup |
| `/v1/config` | `GET` | Server capabilities, active memory, and URL fetch status |
| `/metrics` | `GET` | Prometheus metrics (request counters, latency histograms, active conversions) |
| `/v1/convert` | `POST` | Synchronous conversion (multipart upload or JSON URL) |
| `/v1/convert/async` | `POST` | Asynchronous conversion: returns `202 {"task_id":"..."}` |
| `/v1/status/{id}` | `GET` | Poll status of an async conversion task |
| `/v1/result/{id}` | `GET` | Retrieve the completed async conversion result |
| `/openapi.yaml` | `GET` | OpenAPI 3.1 schema specification |

---

## Observability

### Prometheus Metrics

`docling-serve` exports Prometheus metrics out of the box at `GET /metrics`:

- `docling_http_requests_total`: Total HTTP requests partitioned by status code and method
- `docling_http_in_flight_requests`: Gauge of current in-flight requests
- `docling_http_request_duration_seconds`: Request latency histogram
- `docling_conversions_total`: Conversions partitioned by format and status
- `docling_conversion_duration_seconds`: Conversion execution duration histogram

### OpenTelemetry Tracing

Set `OTEL_EXPORTER_OTLP_ENDPOINT` to stream distributed tracing spans over gRPC:

```bash
-e OTEL_EXPORTER_OTLP_ENDPOINT=http://jaeger:4317 \
-e OTEL_SERVICE_NAME=docling-serve
```

---

## Security Considerations

1. **URL Fetching & SSRF Protection**:
   URL inputs (`{"url": "..."}`) are **disabled by default**. Passing `--allow-url-fetch` enables them. Even when enabled, requests to private, loopback, link-local, and cloud metadata addresses (`169.254.169.254`, `127.0.0.1`, `10.0.0.0/8`, etc.) are blocked, and redirects are forbidden.

2. **Authentication**:
   `docling-serve` does not provide built-in authentication. Always bind to `127.0.0.1` or front the container with a reverse proxy (such as Caddy, NGINX, Traefik, or an API gateway) with authentication before exposing it publicly.

---

## API Usage Examples

### 1. Synchronous File Conversion (Markdown Output)

```bash
curl -X POST http://localhost:5001/v1/convert \
  -F "file=@annual_report.pdf"
```

### 2. Convert to Docling JSON with Embedded Images

```bash
curl -X POST "http://localhost:5001/v1/convert?to=json&images=embedded" \
  -F "file=@document.docx"
```

### 3. Convert to Chunk Records for RAG

```bash
curl -X POST "http://localhost:5001/v1/convert?to=chunks&chunker=hybrid" \
  -F "file=@paper.pdf"
```

### 4. Asynchronous Conversion (for Large Documents)

```bash
# Submit conversion
TASK=$(curl -s -X POST http://localhost:5001/v1/convert/async \
  -F "file=@large_book.pdf")
TASK_ID=$(echo $TASK | jq -r .task_id)

# Poll status
curl http://localhost:5001/v1/status/$TASK_ID
# => {"status":"success"}

# Retrieve result
curl http://localhost:5001/v1/result/$TASK_ID
```
