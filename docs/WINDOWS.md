# docling.rs on Windows (native)

The pipeline runs natively on Windows — no WSL required, and usually faster
than under it: ONNX Runtime's prebuilt Windows binaries (pulled automatically
by the `ort` crate at build time) use the same AVX2/VNNI kernels, without the
VM/filesystem overhead.

## Prerequisites

- **Rust (MSVC toolchain)** — `rustup default stable-x86_64-pc-windows-msvc`
  plus the *Visual Studio Build Tools* with the "Desktop development with C++"
  workload (the linker).
- **Windows 10 1803+** — `curl.exe` and `tar.exe` ship with the OS; the
  download script needs nothing else.

## Build & fetch the ML dependencies

```bat
git clone https://github.com/docling-project/docling.rs
cd docling.rs

scripts\install\download_dependencies.bat   :: .models\
cargo build --release -p docling-cli
```

The script mirrors `download_dependencies.sh`: layout / OCR / TableFormer /
picture-classifier ONNX models (INT8 variants included — preferred
automatically; set `DOCLING_RS_FP32=1` to opt out), Whisper-tiny for audio
(`--no-asr` to skip) and the chunker tokenizer. PDF pages are parsed and
rendered in pure Rust — no `pdfium.dll` is needed.

## Run

Everything resolves relative to the current directory (or next to the
executable), so from the repo root no environment variables are needed:

```bat
target\release\docling-rs paper.pdf
target\release\docling-rs --to json report.docx
```

From another directory, point the models at the checkout — `cmd`:

```bat
set DOCLING_LAYOUT_ONNX=C:\src\docling.rs\models\layout_heron_int8.onnx
```

or PowerShell:

```powershell
$env:DOCLING_LAYOUT_ONNX = "C:\src\docling.rs\models\layout_heron_int8.onnx"
```

Every `DOCLING_*` / `DOCLING_RS_*` variable from the main README works the
same way. The HTTP server is the same crate as on Linux:

```bat
cargo run --release -p docling-serve            :: 127.0.0.1:5001, docs at /
```

## GPU (DirectML)

Every release carries `docling-rs-directml-<tag>-x86_64-pc-windows-msvc.zip`
next to the CPU zip: the CLI built with `--features directml`, and
`DirectML.dll`, which has to stay in the same directory as the exe. It runs
the layout, table, OCR and enrichment models on any DirectX 12 GPU —
NVIDIA, AMD or Intel integrated, no CUDA toolkit — and falls back to CPU
when none is usable (a 10-page paper: 2.5 s on an NVIDIA T600 or 2.7 s on an
Intel iGPU, against 5.5 s for the CPU build). On a machine without a GPU
use the CPU zip: the DirectML build is slower on CPU.

```bat
docling-rs paper.pdf                      :: the high-performance GPU
set DOCLING_RS_DML_DEVICE=1               :: or a DXGI adapter by index
set DOCLING_RS_EP=directml                :: fail instead of falling back to CPU
set DOCLING_RS_TIMING=1                   :: prints the providers registered
```

Build it yourself with `cargo build --release -p docling-cli --features
directml`; `DirectML.dll` lands in `target\release\`. The defaults it
changes (one page worker, per-page layout, fp32 models) and the
measurements are in [MODELS.md](MODELS.md#gpu-execution-providers-optional-off-by-default).
NVIDIA's CUDA provider builds on Windows too (`--features cuda`, CUDA 13 +
cuDNN on the PATH); no CUDA zip is prebuilt.

## Notes & limits

- **Model export / quantization / conformance scripts are bash** — run those
  under WSL or Git Bash; the prebuilt models from the release make this
  unnecessary for normal use.
- **Audio** decoding is pure Rust (symphonia), no ffmpeg needed on Windows
  either.
- The optional `--features web-browser` pre-render needs a Chromium install
  and is untested on Windows.
- Benchmark tip: `target\release\docling-rs --bench-warm 3 paper.pdf` prints
  warm seconds/conversion — handy for comparing against a WSL build of the
  same checkout.
