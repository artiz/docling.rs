# docling-wasm — in-browser document conversion

A `wasm32-unknown-unknown` build of docling.rs's **declarative** converters
(issue #79): DOCX, HTML, Markdown, XLSX, PPTX, CSV, AsciiDoc, EPUB, ODF,
WebVTT, Email, MHTML, JATS, USPTO, XBRL, LaTeX, JSON, DocLang — and the
**embedded text layer of PDFs** — converted to Markdown / docling JSON /
DocLang XML **entirely client-side** — no server, the file never leaves the
page. Python docling cannot do this.

The crate is `docling` with `default-features = false` plus `pdf-text`:
digital PDFs convert through docling-pdf's pure-Rust content-stream parser —
the exact extraction the native `--no-ocr` flag does (flat, line-grouped
paragraphs in reading order; no headings/lists/tables/pictures, since those
need the layout model). The ML pipelines (pdfium + ONNX Runtime) and the HTTP
image fetcher are compiled out: scanned/image-only PDFs get a clear "no
embedded text layer … OCR needs a build with the `pdf` feature" error, and
images/audio/METS are rejected with a "rebuild with …" message. Remote
`<img src>` images stay placeholders (no network in the module); embedded
images work normally.

**Size: ~5.6 MB raw, ~1.9 MB gzipped** (measured on this crate at 0.41.x,
`--release` with the workspace's `lto = "thin"`; no `wasm-opt` pass — one
typically shaves another 10–15%). No models are involved: the declarative
converters and the PDF text parser are pure Rust.

## API

```ts
convert(
  bytes: Uint8Array,
  filename: string,
  to?: "md" | "json" | "doclang",          // default "md"
  images?: "placeholder" | "embedded",     // default "placeholder", Markdown only
): string
supported_extensions(): string   // JSON array, e.g. for <input accept=…>
version(): string
```

The filename's extension drives format detection, same as the CLI. `images`
mirrors docling-serve's option: `placeholder` emits docling's `<!-- image -->`,
`embedded` inlines the picture as a `data:` URI so the Markdown is
self-contained (a page has no filesystem, so there is no `referenced` mode).

### Convert a file the user picked

```js
import init, { convert } from "./pkg/docling_wasm.js";
await init();

const file = input.files[0];
const bytes = new Uint8Array(await file.arrayBuffer());
const markdown = convert(bytes, file.name, "md");
const json     = convert(bytes, file.name, "json");
const withPics = convert(bytes, file.name, "md", "embedded");
```

### Scanned pages (OCR)

Scanned PDFs and images need the ML models; everything else runs with no
network at all. The full wiring — model resolution, Web Worker, pdf.js
rasterization — is [`www/index.html`](./www/index.html); the short version:

```js
import { createOcr } from "./pipeline.js";

const ocr = createOcr({ onStatus: (msg) => console.log(msg) });
await ocr.boot();                        // wasm + layout model

// A standalone image is its own page.
const md = await ocr.convertImage(await file.arrayBuffer(), file.name, "en", "md");

// A scanned PDF: feed rasterized pages in order (2 px/point = the native
// pipeline's RENDER_SCALE), then finish.
await ocr.startDoc("en", /* TableFormer */ false);
for (const page of pages) await ocr.addPage(page.rgba, page.w, page.h, 2.0);
const doc = ocr.finishDoc(file.name, "md");
```

Scanned pictures are cropped out of the rendered page just like the native
pipeline, so `images: "embedded"` inlines real figure bytes on this path too
(`ocr.finishDoc(name, "md", "embedded")`).

Models resolve **device file → local `./models/` → Hugging Face**, so a page can
ship with no models and still work: `ocr.setProvidedModels({ "layout_heron_int8.onnx": buf })`
takes files the user picked, and anything not provided is fetched.

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

[`www/index.html`](./www/index.html) is the whole thing on one page: drop a
file, pick the output (Markdown / JSON / DocLang), pick how images render, and
optionally turn on OCR for scanned pages. After the `wasm-bindgen` step above:

```bash
python3 -m http.server -d crates/docling-wasm/www 8901
# open http://127.0.0.1:8901/
```

It is a plain static page — copy `www/` behind any web server (or into a mobile
app's webview) and it works as-is. The OCR half loads lazily, so a visitor who
only converts a DOCX never downloads ONNX Runtime, pdf.js or any model. PDFs
try their text layer first and fall back to OCR only when there isn't one.

Verified end-to-end in headless Chromium: Markdown/DOCX→md, DOCX→JSON, a
corpus PDF→md through the text-layer path, and the scanned-PDF error path all
exercised through the real wasm module.

## In-browser ML pipeline — OCR, layout, TableFormer (#157)

With the default `ocr` feature the module runs the **full scanned-document ML
pipeline** client-side. Every ONNX-free step — image preprocessing, line/word
segmentation, CTC decoding, RT-DETR box decoding, the TableFormer
autoregressive loop and its bbox bookkeeping, span merging, OTSL→grid layout,
cell matching, region refinement and reading-order assembly — runs in Rust,
**the exact same functions the native pipeline uses**
(`docling_pdf::{ocr_prep, layout, scanned, tf_core, tf_match, resample}`). Only
the four ONNX graphs are delegated to
[ONNX Runtime Web](https://onnxruntime.ai/docs/tutorials/web/) via small JS
wrappers around `ort.InferenceSession`. One implementation is what keeps the
wasm output matching native — drift can only come from the runtime kernels.

| Stage | What | Models (from the [`models-v1` release](https://github.com/docling-project/docling.rs/releases/tag/models-v1)) |
|---|---|---|
| **1** — `ocr_image` | OCR a single scanned **image** | PP-OCRv3 recognition (~10 MB) |
| **2** — `ScannedConverter` / `convert_scanned_image` | Scanned **PDF/image** → Markdown: layout + OCR + reading order, tables via geometric reconstruction | + RT-DETR layout (`layout_heron_int8.onnx`, ~68 MB) |
| **3** — `ScannedConverter.addPageTf` | Real **TableFormer** table structure instead of geometric | + `tableformer/{encoder,decoder_kv,bbox}.onnx` (~380 MB) |

Stage 1 recognition wrapper:

```js
const rec = {
  run: async (n, h, w, data) => {
    const out = await session.run({ [session.inputNames[0]]:
      new ort.Tensor("float32", data, [n, 3, h, w]) });
    const t = out[session.outputNames[0]];
    return { data: t.data, dims: Array.from(t.dims) };
  },
};
const markdown = await ocr_image(imageBytes, dictText, rec, "md");
```

Stage 2 rasterizes PDF pages with pdf.js at `{scale: 2}` (2 px/point — the
native `RENDER_SCALE`) and feeds bitmaps page by page:

```js
const conv = new ScannedConverter(dictText);
for (let p = 1; p <= pdf.numPages; p++) {
  const viewport = page.getViewport({ scale: 2 });
  // … render to canvas, then:
  await conv.add_page(rgba, canvas.width, canvas.height, 2.0, layout, rec);
}
const markdown = conv.finish(file.name, "md");
```

Stage 3 is the same but `conv.addPageTf(rgba, w, h, 2.0, layout, rec, tf)`,
where `tf` is a stateful session over the three TableFormer graphs (the heavy
cross-attention K/V and the growing decoder KV-cache stay on the JS side, so
each decode step marshals only the last tag and gets back logits+hidden — see
[`www/pipeline.js`](./www/pipeline.js)'s `JsTfSession`).
[`www/index.html`](./www/index.html) wires all three stages up behind the OCR
language selector, the TableFormer toggle and the model picker.

## Run it on your phone

The demo is a static page — the models are the only heavy part, and they load
same-origin, from a CORS host, **or straight from files on your device**.

1. **Fetch the models** (once), on a desktop or the phone:
   ```bash
   curl -fsSL https://raw.githubusercontent.com/docling-project/docling.rs/master/scripts/install/download_dependencies.sh | sh
   ```
   This writes `models/` (layout, OCR, TableFormer, …).

2. **Serve the page** from any static host that sends the right MIME types —
   GitHub Pages, `raw.githack.com`, or a local `python3 -m http.server -d
   crates/docling-wasm/www 8901`. (Plain `raw.githubusercontent.com` and most
   GitHub CDNs serve HTML as `text/plain`, so the page won't render — use
   Pages or githack.) Build the wasm `pkg/` first (see **Build** above).

3. **Provide the models to the page**, either:
   - **from your device** — tap the model picker under *"Scanned PDFs and
     images"* and select `layout_heron_int8.onnx` and, for TableFormer,
     `encoder.onnx`, `decoder_kv.onnx` (+`.data`), `bbox.onnx` (+`.data`). Each
     file is read to an `ArrayBuffer` on the client and used directly — no
     download, and a single allocation per file (a 225 MB encoder over
     `fetch` can OOM a mobile tab; a `File` read does not); or
   - **from a CORS mirror** — a Hugging Face model repo serves
     `Access-Control-Allow-Origin` (GitHub Release assets do not); point
     `MODEL_BASE` in `pipeline.js` at yours. The recognition model already
     streams from Hugging Face.

Resolution order for every model is **device file → local `./models/` →
`MODEL_BASE` (Hugging Face)**. Tables need no upload beyond the model files;
the image never leaves the page.

## Performance & threading

- **Threads.** ONNX Runtime Web runs single-threaded unless the page is
  [cross-origin isolated](https://web.dev/coop-coep/) (SharedArrayBuffer). A
  static host can't set the COOP/COEP headers, so [`www/coi.js`](./www/coi.js)
  is a service worker that adds them (`COEP: credentialless`, which keeps the
  CDN/HF fetches working); `pipeline.js` then sets
  `ort.env.wasm.numThreads` to the core count. The demo's ready line reports
  the thread count — if it says `1 thread`, isolation didn't take (some mobile
  browsers), and everything runs serially.
- **int8.** The layout model is the conv-only static-INT8 export (~68 MB,
  ~2.4× faster than fp32 at unchanged conformance). The TableFormer **encoder
  is fp32 (~225 MB) and runs once per table region** — it dominates wall time
  on mobile (a multi-table page can take minutes). An int8 encoder is *not* the
  easy win it looks like: it's a ResNet backbone (~20 Conv) feeding a 6-layer
  transformer, and the transformer Gemms — not the convs — are the cost.
  Measured on a native x86 build over 15 real table crops: conv-only static
  QDQ keeps fidelity (enc_out/cross cosine ≥ 0.995) but is *not* faster than
  ORT's fp32 conv kernels, and quantizing the attention MatMuls collapses
  cross-attention fidelity to ~0.85 (garbled structure) for no speed gain. So
  the encoder stays fp32; the real mobile levers are running heavy tables on a
  desktop and batching multi-table pages (below), not weight quantization.
- **Where to run heavy tables.** TableFormer's 380 MB of models and per-region
  encode make a desktop (more cores, more memory) far faster than a phone; the
  geometric table path (stage 2, no TableFormer) already captures all cell text
  and is the light option for mobile.

## Host-side tests

`cargo test -p docling-wasm` runs the conversion body natively (the
`JsError` boundary only exists on wasm), including a real corpus DOCX and
PDF.
