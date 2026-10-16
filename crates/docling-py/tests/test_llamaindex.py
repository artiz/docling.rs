"""LlamaIndex integration (docling_rs.llamaindex): the port of
``llama-index-readers-docling``'s ``DoclingReader``."""

import json
import re
from pathlib import Path

import pytest

pytest.importorskip("llama_index.core")

from docling_core.types.doc import DoclingDocument  # noqa: E402
from llama_index.core import SimpleDirectoryReader  # noqa: E402
from llama_index.core.schema import Document  # noqa: E402

from docling_rs import DocumentConverter  # noqa: E402
from docling_rs.llamaindex import DoclingReader  # noqa: E402

DATA = Path(__file__).parent / "data" / "markitdown"


def test_markdown_export_is_one_document_per_input():
    reader = DoclingReader()
    docs = reader.load_data(file_path=DATA / "test.docx")
    assert len(docs) == 1 and isinstance(docs[0], Document)
    assert "AutoGen: Enabling Next-Gen LLM Applications" in docs[0].text
    assert re.search(r"\|\s*29\s*\|", docs[0].text)  # the table rendered as Markdown
    assert docs[0].metadata == {}
    # Several inputs, in order; a str path works like a Path.
    docs = reader.load_data(file_path=[str(DATA / "test.docx"), DATA / "test.xlsx"])
    assert len(docs) == 2
    assert "AutoGen" in docs[0].text and "AutoGen" not in docs[1].text
    assert docs[0].doc_id != docs[1].doc_id


def test_json_export_is_the_docling_document():
    reader = DoclingReader(export_type=DoclingReader.ExportType.JSON)
    (doc,) = reader.load_data(file_path=DATA / "test.docx")
    payload = json.loads(doc.text)
    assert payload["schema_name"] == "DoclingDocument"
    dl_doc = DoclingDocument.model_validate(payload)
    assert "AutoGen: Enabling Next-Gen LLM Applications" in dl_doc.export_to_markdown()


def test_extra_info_id_func_and_md_kwargs():
    seen = []

    def ids(doc, file_path):
        seen.append((type(doc).__name__, Path(file_path).name))
        return f"id-{Path(file_path).name}"

    reader = DoclingReader(id_func=ids, md_export_kwargs={"image_placeholder": "<pic>"})
    extra = {"tenant": "t1"}
    docs = reader.load_data(file_path=[DATA / "test.docx", DATA / "test.pptx"], extra_info=extra)
    assert [d.doc_id for d in docs] == ["id-test.docx", "id-test.pptx"]
    assert seen == [("DoclingDocument", "test.docx"), ("DoclingDocument", "test.pptx")]
    # Each document gets its own copy of extra_info.
    assert docs[0].metadata == {"tenant": "t1"} and docs[0].metadata is not docs[1].metadata
    docs[0].metadata["x"] = 1
    assert "x" not in docs[1].metadata and "x" not in extra
    assert "<pic>" in docs[1].text  # the PPTX has pictures


def test_configured_converter_is_used():
    reader = DoclingReader(doc_converter=DocumentConverter(text_layer_only=True))
    (doc,) = reader.load_data(file_path=DATA / "test.pdf")
    assert doc.text.strip()
    assert reader.class_name() == "DoclingReader"


def test_simple_directory_reader_file_extractor(tmp_path):
    import shutil

    shutil.copy(DATA / "test.docx", tmp_path / "a.docx")
    shutil.copy(DATA / "test.xlsx", tmp_path / "b.xlsx")
    reader = DoclingReader()
    docs = SimpleDirectoryReader(
        input_dir=str(tmp_path), file_extractor={".docx": reader, ".xlsx": reader}
    ).load_data()
    assert len(docs) == 2
    by_name = {d.metadata["file_name"]: d for d in docs}
    assert "AutoGen" in by_name["a.docx"].text
    assert by_name["b.xlsx"].metadata["file_type"].startswith("application/")


def test_llama_index_readers_docling_rs_package_reexports_the_reader():
    """The `llama-index-readers-docling-rs` distribution (crates/docling-py/llamaindex)
    is a re-export of this module under LlamaIndex's reader namespace."""
    import importlib
    import sys

    src = Path(__file__).resolve().parents[1] / "llamaindex"
    sys.path.insert(0, str(src))
    try:
        shim = importlib.import_module("llama_index.readers.docling_rs")
    finally:
        sys.path.remove(str(src))
    assert shim.DoclingRsReader is DoclingReader
    assert shim.DoclingReader is DoclingReader
    assert set(shim.__all__) == {"DoclingRsReader", "DoclingReader"}
