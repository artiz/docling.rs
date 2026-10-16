# llama-index-readers-docling-rs

`DoclingRsReader` turns PDF, DOCX, PPTX, XLSX, HTML, EPUB, e-mail, scans and
40-odd other formats into LlamaIndex `Document`s with
[docling.rs](https://github.com/docling-project/docling.rs), a Rust port of
IBM's [docling](https://github.com/docling-project/docling): layout analysis,
table structure and local OCR from a single native wheel, no PyTorch. It is
the API of [`llama-index-readers-docling`](https://github.com/run-llama/llama_index/tree/main/llama-index-integrations/readers/llama-index-readers-docling)
— export as Markdown (the default) or as docling's native JSON for a Docling
node parser.

## Installation

```sh
pip install llama-index-readers-docling-rs
```

PDFs and images use ONNX models (~720 MB), fetched once with
`python -c "import docling_rs; docling_rs.download_models()"`; Office, HTML and
e-mail need none.

## Usage

Basic usage looks like this:

```python
from llama_index.readers.docling_rs import DoclingRsReader

reader = DoclingRsReader()
docs = reader.load_data(file_path="https://arxiv.org/pdf/2408.09869")
print(f"{docs[0].text[:500]}...")
```

To export to docling's native JSON (parse it downstream with a Docling node
parser):

```python
reader = DoclingRsReader(export_type=DoclingRsReader.ExportType.JSON)
```

With `SimpleDirectoryReader`:

```python
from llama_index.core import SimpleDirectoryReader

dir_reader = SimpleDirectoryReader(
    input_dir="/path/to/docs",
    file_extractor={".pdf": reader, ".docx": reader},
)
docs = dir_reader.load_data()
```

The converter is docling.rs's `DocumentConverter`; configure it as usual and
pass it as `doc_converter` (`DoclingRsReader(doc_converter=DocumentConverter(do_ocr=False))`).
`DoclingReader` is the same class under the upstream name.

The package has no code of its own: it pins the `docling-rs` wheel (with its
`llamaindex` extra) and re-exports
[`docling_rs.llamaindex`](https://github.com/docling-project/docling.rs/blob/master/crates/docling-py/README.md#llamaindex),
where the parameters are documented; `pip install "docling-rs[llamaindex]"`
and `from docling_rs.llamaindex import DoclingReader` is the same thing under
the engine's own name. Published from the docling.rs repository, not from
LlamaIndex's monorepo (which no longer takes new integration packages).

## License

MIT. docling.rs is an independent project of the
[docling](https://github.com/docling-project) community (LF AI & Data); not a
LlamaIndex package.
