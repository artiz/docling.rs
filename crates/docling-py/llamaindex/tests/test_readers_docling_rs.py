from llama_index.core.readers.base import BaseReader
from llama_index.readers.docling_rs import DoclingReader, DoclingRsReader


def test_class():
    names_of_base_classes = [b.__name__ for b in DoclingRsReader.__mro__]
    assert BaseReader.__name__ in names_of_base_classes
    assert DoclingRsReader is DoclingReader
    assert DoclingRsReader.class_name() == "DoclingReader"
