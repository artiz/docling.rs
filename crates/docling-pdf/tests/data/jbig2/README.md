# JBIG2 fixtures (#811)

Embedded JBIG2 streams (T.88 Annex D.3: segments without a file header,
the organization a PDF `JBIG2Decode` stream uses) and the reference
bitmaps they decode to, for the `render::jbig2` tests.

| File | What it is |
| --- | --- |
| `generic_mmr.jbig2` | page info + an immediate lossless generic region coded with MMR (the T.6 coding scanners share with CCITT G4) |
| `generic_mmr.g4` | that region's MMR data alone — the same bytes as a `CCITTFaxDecode /K -1` stream, the render tests' CCITT control |
| `generic_mmr.png` | its reference bitmap |
| `symbol_global.globals.jbig2` | a symbol dictionary segment, what a PDF puts in `/JBIG2Globals` |
| `symbol_global.jbig2` | page info + the text region drawing those symbols |
| `symbol_global.png` | its reference bitmap |

Source: the `hayro-jbig2` decoder's own test suite
(<https://github.com/LaurenzV/hayro>, `hayro-jbig2/tests/streams/`
`generic_mmr.jbig2` and `symbol_global.jbig2`, which decode to its
`references/truchet.png` and `references/honeycomb.png`), Apache-2.0 OR
MIT; the JBIG2 files there carry a file header, split
into the embedded form here with the segment headers kept byte for byte.
