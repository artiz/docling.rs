# WordPerfect fixtures (#216)

| File | Origin | What it pins |
| --- | --- | --- |
| `sources/tika_wp6.wpd` | Apache Tika `testWordPerfect.wpd` (Apache-2.0) | WP 6.x: `0xD0` end-of-line group soft/hard ends, extended characters, hard hyphen, an undo (deleted) region that must not surface |
| `sources/tika_wp50.wp` | Apache Tika `testWordPerfect_5_0.wp` (govdocs1 126546.wp) | WP 5.0: hard/soft returns, hard hyphens, Multinational 1 apostrophes |
| `sources/tika_wp51.wp` | Apache Tika `testWordPerfect_5_1.wp` (govdocs1 758750.wp) | WP 5.1: bold attribute pairs, hard hyphens, variable-length groups |
| `sources/libwpd_wp5.wp` | LibreOffice `writerperfect/qa/unit/data/writer/libwpd/pass/WP5.wp` (MPL-2.0) | WP 5.1 minimal document, hard page break |
| `sources/libwpd_wp6.wpd` | LibreOffice `…/libwpd/pass/WP6.wpd` | WP 6.x minimal document (header text lives in prefix packets and is not extracted) |
| `invalid/fuzzed_prefix.wpd` | LibreOffice `…/libwpd/pass/CVE-2007-1735-1.wpd` | garbage prefix → "not a WordPerfect document" |
| `invalid/wp_mac3.wpd` | LibreOffice `…/libwpd/pass/WP3.wpd` | WordPerfect for Macintosh 3.x (file type 44) → targeted refusal |

Python docling has no WordPerfect reader (it reaches the format only through
LibreOffice), so the `expected/` outputs are our own, checked against the
strings Apache Tika's `WordPerfectTest` asserts for the same files. Regenerate
after an intentional change with `DOCLING_RS_REGEN=1 cargo test -p docling
--test regression`.
