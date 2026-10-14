# IBM Z (s390x) and other targets without a prebuilt ONNX Runtime — `ort-load-dynamic`

Back to the [project README](../README.md) · models: [`MODELS.md`](./MODELS.md).


docling.rs builds for `s390x-unknown-linux-gnu` (#504). The code is
endian-clean — s390x is big-endian, and the docling-core, HTML-export and
pure-Rust PDF renderer/raster suites pass as s390x binaries under
`qemu-user` — so every declarative format and the PDF text layer work out of
the box. What pyke's `ort` cannot provide there is a prebuilt ONNX Runtime
(it ships binaries for x86_64/aarch64 only; upstream ONNX Runtime builds and
runs on s390x — IBM contributes the port — but publishes no s390x release).
The `ort-load-dynamic` cargo feature (CLI, serve, lib, Python, Node, RAG)
therefore `dlopen`s the runtime at the first ML session instead of linking
it:

```bash
# on the mainframe, or cross-compiled with gcc-s390x-linux-gnu as the release workflow does
cargo build --release -p docling-cli --features ort-load-dynamic --target s390x-unknown-linux-gnu
```

The library is looked up as `ORT_DYLIB_PATH` (a path), then
`.models/onnxruntime/libonnxruntime.so` (through the models-dir resolution,
so `DOCLING_RS_MODELS_DIR` covers it), then the dynamic linker's search path
(`LD_LIBRARY_PATH`, a distro package); ONNX Runtime 1.17 or newer, any
build — a `pip install onnxruntime`'s `capi/libonnxruntime.so.1.x.y` works
on x86_64. Without it the ML stages degrade exactly like a missing model:
PDF conversion fails with a message naming the library and the lookup, OCR
and enrichment warn and skip, DOCX/HTML/XLSX/… and `docling-rs --to md` on a
DOCX never touch it.

**The s390x runtime itself** ships in the models release as
`onnxruntime-linux-s390x.tar.gz`: ONNX Runtime 1.29.0 as published (the
workflow's default tag is now v1.31.0, the version the linked builds carry
since #523 — re-run it to refresh the asset), cross-compiled by
`.github/workflows/onnxruntime-s390x.yml` with zig against glibc 2.28, so it
loads on RHEL 8/9-era mainframe Linux. `download_dependencies.sh` fetches it
into `.models/onnxruntime/` on an s390x host (`--with-onnxruntime` /
`--no-onnxruntime` elsewhere); the release attaches the s390x CLI tarball,
the FFI library, the Python wheel (`manylinux_2_28_s390x`) and the npm
platform package, `install.sh` picks them, and the container images publish
a `linux/s390x` variant. Every s390x artifact has the same glibc 2.28 floor.

Then either:

```bash
cargo run -p docling-cli -- document.pdf
```

or, in a Node.js/Bun app:

```bash
npm i docling.rs
```

```js
import { convertFileAsync } from 'docling.rs'
const { content } = await convertFileAsync('document.pdf', { to: 'markdown' })
console.log(content)
```
