# Palm e-text fixtures (#216)

| File | Origin | What it pins |
| --- | --- | --- |
| `sources/libreoffice_palmdoc.pdb` | LibreOffice `writerperfect/qa/unit/data/writer/libe-book/pass/PalmDOC.pdb` (MPL-2.0) | PalmDOC (`TEXt`/`REAd`), LZ77-compressed, a one-line text |
| `sources/libreoffice_tealdoc.pdb` | LibreOffice `…/libe-book/pass/TealDoc.pdb` (MPL-2.0; the Universal Declaration of Human Rights) | TealDoc (`TEXt`/`TlDc`), three compressed text records, blank-line paragraphs |
| `sources/libreoffice_ztxt.pdb` | LibreOffice `…/libe-book/pass/zTXT.pdb` (MPL-2.0; the same text) | zTXT (`zTXT`/`GPlm`), one zlib stream over two records |

Python docling has no Palm e-text reader, so the `expected/` outputs are our
own. Regenerate after an intentional change with `DOCLING_RS_REGEN=1
DOCLING_RS_REGRESSION_FORMATS=palmdoc cargo test -p docling --test regression`.
