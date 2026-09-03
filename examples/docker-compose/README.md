# Docker Compose Deployment Examples for `docling-serve`

This directory provides ready-to-use Docker Compose configurations for running the **`docling-serve`** HTTP conversion service.

## Quick Start (Standalone Service)

1. Copy `.env.example` to `.env` (optional, to customize settings):

   ```bash
   cp .env.example .env
   ```

2. Start the service:

   ```bash
   docker compose up -d
   ```

3. Verify the service is ready (models warm):

   ```bash
   curl http://localhost:5001/ready
   # => {"status":"ready"}

4. Convert a document:

   ```bash
   curl -F file=@document.pdf http://localhost:5001/v1/convert
   ```

5. Stop the service:

   ```bash
   docker compose down
   ```

---

## Production Deployment with Caddy (Automatic HTTPS + Auth)

In production environments, `docling-serve` should typically be fronted by a reverse proxy for TLS termination, authentication, and rate limiting:

1. Configure your domain in `.env`:

   ```ini
   DOMAIN=docling.example.com
   CADDY_EMAIL=admin@example.com
   ```

2. Start the stack:

   ```bash
   docker compose -f docker-compose.caddy.yml up -d
   ```

3. Caddy will automatically provision TLS certificates via Let's Encrypt and forward requests to `docling-serve`.

---

## Mounting Pre-downloaded Models & Custom Assets (Optional)

If you prefer to mount models from the host instead of using the baked-in assets, add volume mounts to `docker-compose.yml`:

```yaml
    volumes:
      - /path/to/host/models:/app/.models:ro
      - /path/to/host/pdfium:/app/.pdfium:ro
```

---

## Resource Tuning & Configuration

Key environment variables:

| Variable | Default | Purpose |
|---|---|---|
| `CONCURRENCY` | `2` | Max simultaneous document conversions in flight |
| `DOCLING_RS_MAX_MEMORY_MB` | `4096` | Process RSS ceiling before returning HTTP 503 (prevents OOM) |
| `DOCLING_RS_NO_ARENA` | `1` | Disables ONNX Runtime CPU arena to keep memory compact |
| `RUST_LOG` | `info` | Logging verbosity (`error`, `warn`, `info`, `debug`, `trace`) |
| `PORT` | `5001` | Host port mapping |

For full deployment documentation and advanced options, see [`docs/DEPLOYMENT.md`](../../docs/DEPLOYMENT.md).
