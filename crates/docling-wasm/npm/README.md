# docling.rs-wasm

[docling.rs](https://github.com/docling-project/docling.rs) compiled to
WebAssembly: convert **DOCX, HTML, XLSX, PPTX, CSV, AsciiDoc, EPUB, ODF,
Markdown, WebVTT, Email, MHTML, JATS, USPTO, XBRL, LaTeX, JSON, DocLang — and
the embedded text layer of PDFs** — to Markdown / docling JSON / DocLang XML
**entirely in the browser**. No server; the file never leaves the page.

~1.9 MB gzipped, no models needed for any of the above. Scanned PDFs and
images additionally work through the in-browser ML pipeline (RT-DETR layout +
PP-OCRv3 + TableFormer via [ONNX Runtime Web](https://www.npmjs.com/package/onnxruntime-web))
once you provide the models — see the
[full docs](https://github.com/docling-project/docling.rs/tree/master/crates/docling-wasm#readme)
and the [live demo](https://docling-project.github.io/docling.rs/).

## Bundlers (Vite, webpack, …)

```js
import { convert, supported_extensions } from "docling.rs-wasm";

const file = input.files[0];
const bytes = new Uint8Array(await file.arrayBuffer());
const markdown = convert(bytes, file.name, "md");
const json     = convert(bytes, file.name, "json");
const withPics = convert(bytes, file.name, "md", "embedded"); // data: URIs
```

(The bundler target loads the wasm as a module import — Vite needs no config;
webpack 5 needs `experiments.asyncWebAssembly = true`.)

## No bundler (plain `<script type="module">`)

```js
import init, { convert } from "docling.rs-wasm/web";
await init(); // fetches docling_wasm_bg.wasm next to the JS

const markdown = convert(bytes, file.name, "md");
```

## API

```ts
convert(
  bytes: Uint8Array,
  filename: string,                        // extension drives format detection
  to?: "md" | "json" | "doclang",          // default "md"
  images?: "placeholder" | "embedded",     // default "placeholder", Markdown only
  max_pages?: number,                      // convert only the first N PDF pages
): string
supported_extensions(): string             // JSON array, e.g. for <input accept=…>
version(): string
```

Structured digital-PDF conversion (`DigitalConverter`: headings, lists,
tables, pictures via the layout model), scanned-document OCR
(`ScannedConverter`) and TableFormer table structure are exported too — they
need ONNX Runtime Web sessions on the JS side; the
[crate README](https://github.com/docling-project/docling.rs/tree/master/crates/docling-wasm#readme)
has the complete wiring, and the
[demo page source](https://github.com/docling-project/docling.rs/tree/master/crates/docling-wasm/www)
is a working reference.

## Tauri / Electron

The package runs in any webview as-is — but in a Tauri app you usually want
the **native** pipeline in the Rust backend instead (full OCR, TableFormer,
GPU, no wasm limits). See
[“Tauri and other desktop shells”](https://github.com/docling-project/docling.rs/tree/master/crates/docling-wasm#tauri-and-other-desktop-shells)
in the crate README.

## Related packages

- [`docling.rs`](https://www.npmjs.com/package/docling.rs) — native Node.js /
  Bun bindings with the full ML pipeline (use this on servers).
- [`docling.rs-cuda`](https://www.npmjs.com/package/docling.rs-cuda) — the
  same with CUDA execution providers.
- [`docling-rs`](https://pypi.org/project/docling-rs/) — Python wheel.
