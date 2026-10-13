# docling-serve

The HTTP conversion API of [docling.rs](https://github.com/docling-project/docling.rs)
— the analogue of Python's `docling-serve`. A long-running
[axum](https://github.com/tokio-rs/axum) server over the `docling` converter:
one warm PDF/image pipeline (layout, OCR, TableFormer stay loaded) shared
across requests, a semaphore bounding concurrent conversions, streamed
Markdown responses, async jobs, batch and ZIP inputs, and drop-in
compatibility with clients written against Python docling-serve
(Open WebUI, n8n, Dify, LangChain).

```bash
cargo run --release -p docling-serve                 # 127.0.0.1:5001
# or, from the CLI crate: cargo run --release -p docling-cli --features serve -- serve

curl -F file=@paper.pdf localhost:5001/v1/convert                # Markdown
curl -F file=@report.docx 'localhost:5001/v1/convert?to=json'    # docling JSON
curl -F file=@page.html   'localhost:5001/v1/convert?to=chunks'  # chunk records
curl -F file=@a.pdf -F file=@b.docx localhost:5001/v1/convert    # batch → JSON results array
curl -H 'content-type: application/json' \
     -d '{"url": "https://example.com/doc.pdf", "to": "md"}' \
     localhost:5001/v1/convert     # fetch a URL (needs --allow-url-fetch)

id=$(curl -F file=@big.pdf localhost:5001/v1/convert/async | jq -r .task_id)
curl localhost:5001/v1/status/$id                                # pending|started|success|failure
curl localhost:5001/v1/result/$id                                # the output, once done
```

`GET /` serves the API docs plus an interactive test form, `GET /openapi.yaml`
the OpenAPI 3.1 description, `GET /health` and `GET /ready` the container
probes, `GET /metrics` Prometheus text. SIGTERM drains in-flight requests
before exit.

## Routes

| Route | Purpose |
|---|---|
| `POST /v1/convert` | one `file` part, several (a batch), a ZIP, or a JSON body with `url` / `sources` / `target`; options as query parameters, multipart fields or JSON keys |
| `POST /v1/convert/async` · `GET /v1/status/{id}` · `GET /v1/result/{id}` | the same request as a job: a task id now, the result when done (kept `--result-ttl` seconds) |
| `POST /v1/convert/file` · `POST /v1/convert/source` (+ `/async`, `/v1alpha/…`) | Python docling-serve's routes and response shape (`ConvertDocumentResponse`), for its clients |
| `GET /v1/config` | the server's policies (`allow_url_fetch`, limits) |
| `GET /` · `GET /openapi.yaml` · `GET /health` · `GET /ready` · `GET /metrics` | docs and test form · OpenAPI · liveness · readiness (503 until `--warmup` finishes) · Prometheus |

The output is selected with `to`: `md` (default, streamed) · `json` · `html`
· `text` · `dclx` · `latex` · `pandoc` · `vtt` · `chunks` · `images` (a PDF's
pages as PNG). Every conversion option of the shared
[`ConvertOptions`](../../docs/OPTIONS.md) is accepted under its wire name
(`strict`, `pages`, `no_ocr`, `password`, `ocr_*`, `pipeline=vlm`, the
`do_*_enrichment` and `redact_*` switches, `document_timeout`, …).

## Server flags

| Flag | Default | Meaning |
|---|---|---|
| `--addr HOST:PORT` | `127.0.0.1:5001` | bind address — loopback by default; front it with a policy proxy for anything wider |
| `--concurrency N` | 2 | conversions in flight; further requests queue |
| `--max-body-mb N` | 256 | upload cap |
| `--queue-size N` · `--result-ttl SECS` | 16 · 600 | async jobs queued or unfetched at once · how long a finished result stays fetchable |
| `--warmup` | off | load the PDF/image models at startup |
| `--allow-url-fetch` | off | URL inputs, remote images, caller-supplied VLM endpoints, `put` targets — behind an SSRF guard (private/loopback targets refused unless `DOCLING_RS_ALLOW_PRIVATE_IP_FETCH=1`; downloads capped by `DOCLING_RS_MAX_FETCH_BYTES`) |
| `--allow-local-images` | off | accept `image_sources=local` |
| `--strict` | off | strict Markdown for requests that don't say |
| `--api-key KEY` | unset | `X-Api-Key` on every `/v1` route (`DOCLING_SERVE_API_KEY` when the flag is absent) |
| `--max-memory-mb N` | the cgroup limit | admission control: above 85 % new conversions get 503 + Retry-After |

## Cargo features

| Feature | Default | What it adds |
|---|---|---|
| `chunking` | on | tokenization-aware hybrid chunk records in `to=chunks` |
| `cloud` | off | S3 / Azure Blob / GCS request sources and output targets (`object_store`) |
| `otel` | off | OpenTelemetry trace export over OTLP (`OTEL_EXPORTER_OTLP_ENDPOINT`) |
| `cuda` · `tensorrt` · `directml` · `coreml` · `xnnpack` · `ort-load-dynamic` | off | ONNX Runtime execution providers for the ML pipeline, selected at runtime with `DOCLING_RS_EP` |
| `pdfium` · `heif` | off | pdfium as an opt-in PDF renderer · HEIC/HEIF input (links libheif) |

PDF, image, audio and video conversion need the ONNX models — fetch them with
`scripts/install/download_dependencies.sh` from the repository, or use the
container images, which carry them.

## Documentation

- [README § HTTP conversion API](../../README.md#http-conversion-api--docling-rs-serve)
  — request options, the docling-serve drop-in routes, Open WebUI / n8n setup
- [`docs/DEPLOYMENT.md`](../../docs/DEPLOYMENT.md) — container images, Compose
  files, GPU stacks, resource sizing, air-gapped deployment
- [`docs/SECURITY.md`](../../docs/SECURITY.md) — the outbound-fetch policies,
  limits and every `DOCLING_RS_*` environment variable
- [`docs/OPTIONS.md`](../../docs/OPTIONS.md) — the conversion options and their
  wire names

## License

MIT, like the rest of docling.rs.
