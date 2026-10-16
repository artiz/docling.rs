# langchain-docling-rs

A [LangChain](https://python.langchain.com) document loader backed by
[docling.rs](https://github.com/docling-project/docling.rs), a Rust port of
IBM's [docling](https://github.com/docling-project/docling): PDF with layout
analysis, table structure and local OCR, DOCX, PPTX, XLSX, HTML, EPUB, e-mail,
scans and 40-odd other formats, into LangChain `Document`s — one per input as
Markdown, or one per chunk with docling's chunk metadata (items, page and
bounding-box provenance, headings). No PyTorch; the models run through ONNX
Runtime from a single native wheel.

It is `langchain-docling`'s API, so the import is the only change:

```bash
pip install langchain-docling-rs
```

```python
# was:  from langchain_docling import DoclingLoader, ExportType
from langchain_docling_rs import DoclingLoader, ExportType

docs = DoclingLoader(file_path=["https://arxiv.org/pdf/2408.09869", "notes.docx"]).load()
docs[0].page_content                      # heading path + chunk text, ready to embed
docs[0].metadata["dl_meta"]               # docling's chunk meta: items, page/bbox provenance, headings

DoclingLoader(file_path="report.pdf", export_type=ExportType.MARKDOWN).load()  # one Document per input
```

The package has no code of its own: it pins the `docling-rs` wheel (with its
`langchain` extra) and re-exports
[`docling_rs.langchain`](https://github.com/docling-project/docling.rs/blob/master/crates/docling-py/README.md#langchain)
— `DoclingLoader`, `ExportType`, `MetaExtractor`, `BaseMetaExtractor`,
`ConversionBackend` and `PictureDescriptionLangChainOptions` (describe
document pictures with any LangChain chat model during conversion). The
parameters, defaults and chunking are documented there; `pip install
"docling-rs[langchain]"` and `from docling_rs.langchain import …` is the same
thing under the engine's own name.

## PDF models

Office, HTML, Markdown and e-mail need no models. PDFs and images use ONNX
models for layout, tables and OCR (~720 MB), fetched once with
`python -c "import docling_rs; docling_rs.download_models()"`. Without them a
PDF converts from its embedded text layer.

## License

MIT. docling.rs is an independent project of the
[docling](https://github.com/docling-project) community (LF AI & Data); not a
LangChain partner package.
