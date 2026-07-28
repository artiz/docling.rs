# Contributing

Thanks for helping out. docling.rs is a Rust port of
[docling](https://github.com/docling-project/docling); the guiding constraint is
**conformance**: output is validated against upstream Python docling, byte-for-byte
where the format allows it.

## Setup

```bash
git clone https://github.com/docling-project/docling.rs
cd docling.rs
cargo build

# Optional — only for the ML pipeline (PDF layout/OCR/tables, ASR, enrichment):
scripts/install/download_dependencies.sh   # models/ + .pdfium/
```

Without models and pdfium the declarative converters (DOCX, HTML, XLSX, PPTX, EPUB, …)
and text-layer PDFs work fine, and the test suite stays green — tests that need runtime
assets skip themselves.

## Build & test

```bash
cargo test --lib --tests -p docling-core -p docling -p docling-asr -p docling-serve -p docling-pdf
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

Prefer `--lib --tests` over a bare `cargo test`: it skips the example binaries, each of
which statically links onnxruntime and adds gigabytes to `target/`.

CI additionally checks:

- MSRV: `docling-core` on 1.85, the workspace on 1.88;
- `wasm32-unknown-unknown`: `cargo check -p docling --no-default-features [--features pdf-text]`,
  a release build of `docling-wasm`, and its host-side tests;
- GPU feature combinations for `docling-cli` / `docling-rag` / `docling-pdf`;
- `cargo test --workspace --locked`.

If you touch the Python or Node bindings, check them from their own directories
(`cd crates/docling-py && cargo check`) — they are excluded from the workspace default
members.

## Conformance

The corpus lives in `tests/data/<format>/sources/` with `groundtruth/` produced by
published Python docling. Declarative formats must match **byte-for-byte**; the ML
pipeline is pinned by deterministic snapshots.

```bash
scripts/conformance/pdf_groundtruth.sh      # PDF vs the committed groundtruth (no docling install)
scripts/conformance/conformance.sh <format> # installs docling and diffs against it
DOCLING_RS_REGEN=1 cargo test -p docling --test regression   # regenerate intentional output changes
```

A change that moves conformance numbers must say so in the pull request and update
`docs/PDF_CONFORMANCE.md` / `docs/MIGRATION.md` with the real measured numbers.

## Conventions

- **Options plumb through every surface in one pull request**: library builder on
  `DocumentConverter` → CLI flag → serve option (multipart field + JSON body + query
  param) → Python kwarg → Node option. Grep an existing option (`page_range`,
  `video_frames`) for the full pattern.
- **Degradation over failure**: a missing optional tool or model warns and degrades; only
  "nothing convertible at all" is an error.
- **Comments explain *why*** — docling parity, a performance trade-off, a corpus fixture
  that forced the shape. What the code does should be readable from the code.
- **Docs are part of the change**: `README.md` for user-facing behavior,
  `docs/MIGRATION.md` for the parity table, `docs/PDF_CONFORMANCE.md` for pipeline/model
  changes.
- New behavior needs a test — ideally a corpus fixture, otherwise a unit test that pins
  the specific case.

## Commits and pull requests

- One feature per branch, branched off fresh `master`. Don't stack unrelated work.
- **Sign off every commit**: `Signed-off-by: Name <email>` (`git commit -s`).
- Reference issues in the message (`Refs #123`).
- Commit subjects drive releases: a merge whose commits are all `fix:` / `perf:` /
  `revert:` cuts a patch version; anything else bumps the minor. Prefix bug fixes
  accordingly and don't worry about the rest. Docs- or CI-only merges publish nothing.
- Describe in the pull request what you ran: which suites, which conformance numbers
  moved.

## Reporting bugs

Include the input format, the exact command or API call, the docling.rs version, and —
where the output is wrong rather than absent — what Python docling produces for the same
file. A minimal reproducing file is worth more than a description of one.

## License

MIT. By contributing you agree your work is licensed under it.
