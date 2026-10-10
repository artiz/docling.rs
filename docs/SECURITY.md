# Security notes

docling.rs converts **untrusted documents** (PDF, DOCX, XLSX, PPTX, HTML,
EPUB, images, audio, …). The parsers are the primary attack surface: a
document is adversarial input, and a converter must fail gracefully rather
than crash the process or reach out to the network unexpectedly. This note
records the hardening in place, how to deploy the servers safely, and the
known residual items.

## Threat model

- **In scope:** a crafted document must not cause remote code execution,
  out-of-process crashes (uncatchable aborts — allocation failure, stack
  overflow), unbounded memory/CPU (DoS), server-side request forgery (SSRF),
  local-file disclosure, or path traversal.
- **Out of scope / operational:** an attacker who can already write to the
  process working directory or set its environment (they can plant a
  malicious `.models/*.onnx` (or, under the `pdfium` feature, a `.pdfium/lib`) that the loader would pick up —
  run with a fixed, non-writable CWD and absolute `DOCLING_*` model paths).
  The `web-browser` feature runs system Chromium **unsandboxed** on the input
  HTML and is off by default; enable it only for content you accept that risk
  for.

## Hardening in place

Resource limits that turn a crafted document from a process abort into a
recoverable error or a skipped part. Every one is overridable by environment
variable (or server flag) for the rare legitimate outlier; `docs/env-vars.csv`
is the registry of every variable the code reads, and
`crates/docling/tests/security_doc.rs` fails when a limit, subprocess,
secret or network variable is missing from this page.

| Surface | Guard (default) | On exceed | Override |
|---------|-----------------|-----------|----------|
| Standalone image decode (`convert_image`, METS) | ONNX-free decode with `image::Limits`: 30000 px per side, 256 MiB allocation — a few-KB image declaring 60000×60000 no longer allocates ~10 GB | the image fails with an error | `DOCLING_RS_MAX_IMAGE_PIXELS` |
| Rendered PDF page bitmap | 15000 px per side — a few-hundred-byte PDF declaring a huge `MediaBox` would otherwise ask for a multi-GB bitmap | the page fails with an error before anything is allocated | `DOCLING_RS_MAX_RENDER_PIXELS` |
| Resolved images (HTML/EPUB/MHTML/JATS/AsciiDoc/ODF/Markdown/email, #646) | 32 MiB per image (also the remote fetch's read cap); per document: images resolved, their total bytes, a size floor — unlimited / unlimited / 0 by default | the picture stays a placeholder, one warning per document; the conversion never fails | `DOCLING_RS_MAX_IMAGE_BYTES`, `DOCLING_RS_MAX_IMAGES`, `DOCLING_RS_MAX_IMAGE_TOTAL_MB`, `DOCLING_RS_MIN_IMAGE_BYTES` (and the `max_image_bytes` / `max_images` / `max_image_total_mb` / `min_image_bytes` options) |
| DjVu scan render (OCR fallback) | the page is rendered into a 2500 px box | never fails: the bitmap is scaled down | `DOCLING_RS_DJVU_RENDER_PX` |
| OOXML / EPUB part inflation (DOCX, PPTX, XLSX, EPUB) | 512 MiB per decompressed part (checked against the declared size, then while reading, so a lying header cannot get past it) | the part is **skipped** (never truncated, no message) and the conversion continues without it | `DOCLING_RS_MAX_PART_BYTES` |
| ZIP inputs (a `.zip` given to the CLI or uploaded to serve, #557) | 10 000 entries · 256 MiB per entry · 1024 MiB in total · compression ratio 200 for an entry over 1 MiB — checked on the central directory before anything is inflated | the entry is skipped with its reason in the log; the other entries convert | `DOCLING_RS_ZIP_MAX_ENTRIES`, `DOCLING_RS_ZIP_MAX_ENTRY_MB`, `DOCLING_RS_ZIP_MAX_TOTAL_MB`, `DOCLING_RS_ZIP_MAX_RATIO` |
| XML element nesting (`roxmltree` recurses per level) | 512 levels, counted by an iterative scan before parsing | an OOXML / EPUB part is skipped (`docling: …; part skipped` on stderr); a standalone XML input (JATS, USPTO, XBRL, DocLang, AbiWord, SVG) fails with an error | `DOCLING_RS_MAX_XML_DEPTH` |
| HTML / EPUB DOM walk | 2000 levels, checked iteratively | the document is emitted as flattened text instead of overflowing the recursion stack | `DOCLING_RS_MAX_HTML_DEPTH` |
| Spreadsheet sheet size (calamine allocates the dense used area) | 10 000 000 cells in a sheet's used area — a 4 KB file with values in `A1` and `XFD1048576` asked for 17 billion | the sheet is skipped with a warning; the other sheets convert | `DOCLING_RS_SHEET_MAX_CELLS` |
| Remote VLM page request (`--pipeline vlm`) | 600 s per page request (connect: 10 s) | the page fails with an error naming the cap; a timeout is not retried | `DOCLING_RS_VLM_TIMEOUT` |
| Audio sample rate (ASR) | header-declared rate clamped to 8 kHz–768 kHz, so the resampler can't be steered into an OOM-sized upsample | clamped | — |
| Encrypted Office documents (#625) | the Agile spin count is capped at the spec's 10 000 000, key/salt/block/hash sizes are checked against the spec before any hashing or allocation, and the declared plaintext size of an `EncryptedPackage` may not exceed its ciphertext (the stream itself is under the part budget above) | the document fails with "encryption header is damaged" naming the field | — |
| TableFormer matching | `median()` guards the empty slice (a crafted table row/column with zero matched cells no longer panics) | — | — |

`docling-serve` adds request-level bounds:

| Guard (default) | On exceed | Override |
|-----------------|-----------|----------|
| Request body: 256 MiB (multipart uploads and JSON bodies alike) | **413** before anything converts | `--max-body-mb` |
| URL fetch response: 256 MiB | the request fails (`exceeds N bytes`) | `DOCLING_RS_MAX_FETCH_BYTES` |
| `to=images` rasterization: 100 pages per request | **400** naming the variable (narrow with `pages=A-B`) | `DOCLING_RS_MAX_RASTER_PAGES` |
| Memory admission control: the container's cgroup limit, else none; new conversions stop at 85 % of it | **503** + `Retry-After` while the process RSS is above the watermark; in-flight conversions keep running | `--max-memory-mb` / `DOCLING_RS_MAX_MEMORY_MB` (`0` disables), `DOCLING_RS_MEMORY_WATERMARK_PCT` |
| Async jobs: 16 queued or unfetched at once, results kept 600 s | **429** | `--queue-size`, `--result-ttl` |

What is **not** bounded: there is no global page-count or input-size cap like
Python docling's `convert(max_num_pages=…, max_file_size=…)` (both default to
unlimited there); use `--page-range` to convert a window, and the server's
`--max-body-mb` for uploads. An OOXML package has a per-part cap but no
total or ratio cap across its parts (docling's strict OOXML path caps
512 MiB per member and 2 GiB in total).

### External programs

Some inputs and options run a program the converter does not ship, on
content derived from the untrusted document:

- **`ffmpeg`** — audio and video inputs. Audio decodes in-process
  (symphonia) first; what symphonia cannot read (Ogg Opus, AVI, …) is piped
  through `ffmpeg` (input on stdin, PCM on stdout). Video frames are always
  extracted by `ffmpeg`, from a temporary copy of the file.
  `DOCLING_FFMPEG` names the binary, else `ffmpeg` on `PATH`; without it
  those audio files fail and video converts as a transcript only.
- **`tesseract`** — only with `--ocr-engine tesseract` (`DOCLING_RS_OCR_ENGINE`):
  one process per page crop. `DOCLING_TESSERACT` names the binary,
  `DOCLING_RS_TESSDATA_DIR` its language data.
- **Chromium** — only with the `web-browser` Cargo feature and
  `--use-web-browser`, **unsandboxed** (see the threat model).
  `DOCLING_RS_CHROME` names the binary.

These calls carry **no timeout**: a decoder that hangs on a crafted file
blocks that conversion (and, in `docling-serve`, holds one of its
`--concurrency` slots) until the process is killed, and the PCM `ffmpeg`
writes for a long audio track is buffered in memory without a cap. The trust
assumption is the same as for Chromium: they run with the converter's
privileges, on input you accepted. Pin the binary paths to a directory the
service user cannot write, and when converting untrusted media at scale run
the converter (or the whole server) under an external CPU-time / memory
limit — a container cgroup or `systemd` `RuntimeMaxSec` / `MemoryMax`.

### Secrets

- `DOCLING_RS_VLM_API_KEY` is sent as a bearer token to the VLM endpoint —
  the operator's `DOCLING_RS_VLM_ENDPOINT`, or a request-supplied
  `vlm_endpoint` (behind `--allow-url-fetch`). A caller who may choose the
  endpoint therefore receives the server's key: pin the endpoint
  server-side when a key is set.
- `DOCLING_SERVE_API_KEY` (or `--api-key`) is docling-serve's own access key
  (below). Prefer the variable: a command-line flag shows up in the process
  list.
- A document password (`password`; CLI `--password`) is only handed to
  the decryption — never logged or echoed in an error. On a shared machine
  give the CLI `--password-file PATH` instead: `--password` shows up in the
  process list. Office documents also get their format's **published
  default password** tried after the given one (#625): PowerPoint's
  `/01Hannes Ruescher/01` and Excel's `VelvetSweatshop` — the passwords
  Office itself uses for files protected only against editing, which it
  opens without prompting. A file "protected" that way is therefore
  converted without a password, exactly as Office would show it; this
  includes such files sent as attachments.

XML safety (verified, no change needed): the DOM parser (`roxmltree`) never
resolves external entities (no XXE) and caps entity-reference depth/count (no
"billion laughs"); OOXML parts are read **into memory by name**, never
extracted to disk, so there is no zip-slip path.

### `docling-serve` (HTTP conversion API)

- **URL fetch is off by default** (`--allow-url-fetch` to enable). When
  enabled, the target host is resolved and refused if it maps to a
  loopback / private / link-local / unique-local / CGNAT address (blocks
  `169.254.169.254` cloud metadata and internal services); HTTP redirects
  are disabled (no public→internal bounce); and the fetched response is size-
  capped (256 MiB, `DOCLING_RS_MAX_FETCH_BYTES`). For local development,
  `DOCLING_RS_ALLOW_PRIVATE_IP_FETCH=1` disables the IP block-list so a
  `localhost` target can be fetched — leave it unset in production.
- A crafted PDF/image that panics inside the pipeline no longer **poisons**
  the shared mutex — the lock recovers, so one bad document can't turn into a
  permanent outage of the endpoint.
- **Image sources (`image_sources`, #646; `fetch_images` is its `remote`
  alias)** — which `<img src>` / `![…](…)` / `cid:` references the
  declarative backends resolve. `embedded` (`data:` URIs and parts of the
  same container — EPUB/MHTML entries, email `cid:` attachments) touches
  neither the filesystem nor the network and is the tier to run on untrusted
  input. `local` reads files only **under the source file's directory**: a
  relative path is canonicalized (so `..` and symlinks resolve) and must
  still be inside; absolute paths and `file://` are never read (a document
  naming `/etc/hosts` gets a placeholder). `remote` is outbound fetch, so it
  lives behind the **same `--allow-url-fetch` gate** as URL inputs: with the
  flag off (the default) the request is held to `embedded` (pictures that
  need the network stay placeholders) and the web UI greys the checkbox; and
  `local` is refused (400) unless the operator started the server with
  `--allow-local-images`. A remote fetch carries the same SSRF guard — a host
  resolving to a private/loopback/link-local address is skipped — plus an
  optional **host allow-list** (`image_hosts`: exact names or `*.suffix`)
  that every redirect hop is held to, a connect/overall timeout (5 s / 20 s)
  and a redirect cap, so one slow or hostile image can't hang the conversion
  (and thus the server). `DOCLING_RS_ALLOW_PRIVATE_IP_FETCH=1` opts out of the
  IP block-list for local/intranet image servers, same as the URL fetch above;
  a host on the allow-list does **not** lift it.
- **`pipeline=vlm` with a request-supplied `vlm_endpoint`** (#304) points
  the server's outbound traffic (page images included) wherever the caller
  says, so it sits behind the **same `--allow-url-fetch` gate** and the same
  resolve-and-refuse private-address check as URL inputs. The safer
  deployment is the operator-pinned mode: set `DOCLING_RS_VLM_ENDPOINT` /
  `DOCLING_RS_VLM_MODEL` on the server and let requests send only
  `pipeline=vlm` — the pinned endpoint is the operator's own choice, so it
  may be local (e.g. an Ollama on loopback) and needs no gate.
- **Authentication is optional and off by default.** With
  `DOCLING_SERVE_API_KEY` / `--api-key` set (#615, docling-serve's own
  scheme), every `/v1` and `/v1alpha` route requires a matching `X-Api-Key`
  header (401 otherwise); `/health`, `/ready`, `/metrics` and the docs page
  stay open. Without a key, bind to loopback (the default) or place an
  authenticating/policy proxy in front before exposing it.

### `docling-rag` (retrieval API)

Fail-closed API-key auth (`X-Api-Key` / `Bearer` / `?api_key=` for browser
links); all vector-store SQL is parameterized (no injection); uploaded
filenames are reduced to a single path segment and the markdown-dump path
keeps only `Normal` components (no traversal); served markdown is
`text/markdown` (not rendered HTML) and the UI escapes all interpolated
fields.

## Known residual

- **`quick-xml` DoS advisories (RUSTSEC-2026-0194 / -0195) via `calamine`.**
  Our direct XML parsing is on `quick-xml` ≥ 0.41 (patched). The XLSX reader
  `calamine` still pins `quick-xml` 0.31 transitively; until `calamine`
  upstream bumps, a crafted **XLSX** can still trigger the quadratic-attribute
  / namespace-allocation DoS. Impact is bounded by the process/deployment
  (single-document DoS, not RCE); mitigate by not exposing XLSX conversion to
  untrusted callers without a per-request resource bound, or run conversions
  in a resource-limited sandbox.

## Running the audit

```sh
cargo audit         # known-CVE scan of the dependency tree
```

CI runs the same scan weekly over both committed lockfiles (the `audit` job in
`.github/workflows/deps-update.yml`, with the "Known residual" advisories
above passed as `--ignore`; keep the two lists in sync). A new finding fails
that job — the red run is the alarm — and triggers `audit-fix`, which runs
`cargo audit fix` on both lockfiles and opens a `fix(deps):` PR with the
semver-compatible bump when one exists; an advisory that needs a new major
version is reported in the job log and stays a manual change.
