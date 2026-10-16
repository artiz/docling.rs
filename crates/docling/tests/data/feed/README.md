# Feed fixtures (#678)

Real feeds, fetched 2026-10-15, trimmed only where noted; the articles
they link to are not part of the fixtures.

| File | Origin | Pins |
| --- | --- | --- |
| `sources/rust_blog.atom` | <https://blog.rust-lang.org/feed.xml> (the Rust blog, MIT / Apache-2.0), cut to its first four entries | Atom: `subtitle`, `published` and `updated`, `content type="html"`, entry links |
| `sources/go_blog.atom` | <https://go.dev/blog/feed.atom> (the Go blog, BSD / CC BY 4.0), cut to its first two entries | Atom: a `summary` and a `content` per entry, long HTML bodies |
| `sources/w3c_news.rss` | <https://www.w3.org/blog/news/feed/> (W3C News, W3C document licence) | RSS 2.0: `pubDate`, `description`, `content:encoded` in CDATA, an `atom:link` |
| `sources/debian_security.rdf` | <https://www.debian.org/security/dsa.rdf> (Debian Security Advisories) | RSS 1.0 (RDF): `rdf:RDF` root, items beside the channel, `dc:date` |

MarkItDown's `test_rss.xml` (the Microsoft blog, MIT, the ticket's
acceptance) lives with the other MarkItDown fixtures in
`crates/docling-py/tests/data/markitdown/`; `crates/docling/tests/feed.rs`
reads it from there.
