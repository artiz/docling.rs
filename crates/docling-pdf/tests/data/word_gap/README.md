# Word-gap fixtures (#921)

One page each of public official documents whose headings or signature
blocks are letter-spaced: the producer sets every letter with one constant
cursor move and no space glyph, so the word-gap rule has to tell tracking
from word boundaries. Contributed on #921 by the issue's reporter, reduced to
the one page with lopdf (the page's object graph kept, the rest pruned; the
raw cells, glyphs and text export were checked equal before and after).
Official works — statutes, ordinances and court decisions — are free of
copyright (§ 7 öUrhG, § 5 UrhG).

| File | Document | Original | Page | Letter-spaced text |
| --- | --- | --- | --- | --- |
| `bgbl-at-2025-ii-6-p1.pdf` | BGBl. II Nr. 6/2025, Heimarbeitstarif (RIS, authentic edition) | <https://www.ris.bka.gv.at/Dokumente/BgblAuth/BGBLA_2025_II_6/BGBLA_2025_II_6.pdfsig>, SHA-256 `4aef661d…5766` | 1 | heading `Heimarbeitstarif` |
| `bgbl-de-2026-i-300-p4.pdf` | BGBl. 2026 I Nr. 300, 23. Verordnung zur Änderung der Außenwirtschaftsverordnung | <https://www.recht.bund.de/bgbl/1/2026/300/regelungstext.pdf?__blob=publicationFile&v=1>, SHA-256 `c129a542…b837` | 4 | signature block `Der Bundeskanzler` / `Merz` / `Die Bundesministerin für Wirtschaft und Energie` / `Katherina Reiche` |
| `ris-olg-wien-4r133-24v-p1.pdf` | OLG Wien 4 R 133/24v, 11 February 2025 (RIS Justiz) | <https://www.ris.bka.gv.at/Dokumente/Justiz/JJT_20250211_OLG0009_00400R00133_24V0000_000/JJT_20250211_OLG0009_00400R00133_24V0000_000.pdf>, SHA-256 `4fa14b5d…fdf1` | 1 | heading `Entscheidungsgründe:` |

`eurlex-c-2025-1373-p8.pdf` is the regression page for kerned `f|l` pairs
in one-word table cells on a page that draws its spaces as glyphs
(`Offenlegungspf lichten`): page 8 of Commission Notice C/2025/1373 on the
EU Taxonomy delegated acts (OJ C, 5.3.2025, German, ELI
<http://data.europa.eu/eli/C/2025/1373/oj>; file from the Publications
Office, <https://publications.europa.eu/resource/cellar/6763ce0a-f962-11ef-b7db-01aa75ed71a1.0004.01/DOC_1>,
SHA-256 `31c608b5…d9f0`), reduced the same way. © European Union,
<https://eur-lex.europa.eu>; reused under Commission Decision 2011/833/EU
(Articles 4 and 6(2): free reuse, source acknowledged, meaning not
distorted — the page's own objects, unmodified).

SHA-256 of the reduced files, in the table's order:

```
deea977eabb5aeb1c083ed1848b913a672bd367578454971194661e921e35712  bgbl-at-2025-ii-6-p1.pdf
141cb0eb2d8dac5e0d082550dc20f3da16707de594871f66c9e1e6d76df3c99c  bgbl-de-2026-i-300-p4.pdf
9f9b3982e772bb7cdb6917a40c8f0405fb9adcde1e7db26d99e0e37449e1f21a  ris-olg-wien-4r133-24v-p1.pdf
332c277c9b477d9e94c7d0721fecb445b11143a397a8981b33b8d61fdd503547  eurlex-c-2025-1373-p8.pdf
```
