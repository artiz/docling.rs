"""docling-parity alias of ``docling.datamodel.base_models``::

# was: from docling.datamodel.base_models import InputFormat, DocumentStream
from docling_rs.datamodel.base_models import InputFormat, DocumentStream
"""

from .. import ConfidenceReport, ConversionStatus, PageConfidenceScores, QualityGrade
from ..options import DocumentStream, InputFormat

__all__ = [
    "InputFormat",
    "DocumentStream",
    "ConversionStatus",
    "ConfidenceReport",
    "PageConfidenceScores",
    "QualityGrade",
]
