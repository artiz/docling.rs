# docling-wasm — in-browser document conversion

A `wasm32-unknown-unknown` build of docling.rs's **declarative** converters
(issue #79): DOCX, HTML, Markdown, XLSX, PPTX, CSV, AsciiDoc, EPUB, ODF,
WebVTT, Email, MHTML, JATS, USPTO, XBRL, LaTeX, JSON and DocLang, converted to
Markdown / docling JSON / DocLang XML **entirely client-side** — no server,
the file never leaves the page. Python docling cannot do this.

The crate is `docling` with `default-features = false`: the PDF/image/ASR ML
pipelines (pdfium + ONNX Runtime) and the HTTP image fetcher are compiled out.
Those formats stay detectable and are rejected at convert time with a clear
"rebuild with the `pdf` feature" error. Remote `<img src>` images stay
placeholders (no network in the module); embedded images work normally.

**Size: ~5.2 MB raw, ~1.8 MB gzipped** (measured on this crate at 0.41.x,
`--release` with the workspace's `lto = "thin"`; no `wasm-opt` pass — one
typically shaves another 10–15%). No models are involved: the declarative
converters are pure Rust.

## API

```ts
convert(bytes: Uint8Array, filename: string, to?: "md" | "json" | "doclang"): string
supported_extensions(): string   // JSON array, e.g. for <input accept=…>
version(): string
```

The filename's extension drives format detection, same as the CLI.

## Build

```bash
rustup target add wasm32-unknown-unknown

# Either wasm-pack (bundles the JS glue + package.json):
wasm-pack build crates/docling-wasm --target web

# ...or plain cargo + wasm-bindgen (what CI and the demo below use):
cargo build -p docling-wasm --target wasm32-unknown-unknown --release
wasm-bindgen --target web --out-dir crates/docling-wasm/www/pkg \
    target/wasm32-unknown-unknown/release/docling_wasm.wasm
```

## Demo

[`www/index.html`](./www/index.html) is a drop-a-file demo page over the
module (output selector, conversion timing, automated-test hook). After the
`wasm-bindgen` step above:

```bash
python3 -m http.server -d crates/docling-wasm/www 8901
# open http://127.0.0.1:8901/
```

Verified end-to-end in headless Chromium: Markdown/DOCX→md, DOCX→JSON, and
the PDF rejection path all exercised through the real wasm module.

## Host-side tests

`cargo test -p docling-wasm` runs the conversion body natively (the
`JsError` boundary only exists on wasm), including a real corpus DOCX.
