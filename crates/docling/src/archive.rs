//! ZIP archives as input (#557): every document inside converts on its own.
//!
//! Batches of documents travel as ZIP files — exports, datasets, mail
//! attachments, uploads. [`Archive`] reads one without unpacking it to disk:
//! [`Archive::open`] lists the entries from the central directory and decides,
//! before decompressing a byte, which ones convert and why the others are
//! skipped (unsupported type, nested archive, unsafe path, encryption, the
//! [`ArchiveLimits`]); [`Archive::read`] then extracts one entry as a
//! [`SourceDocument`], never more than the size its header declared.
//! [`DocumentConverter::convert_archive`] puts the two together as a lazy
//! iterator of per-entry outcomes, so one broken document never fails the
//! rest; [`DocumentConverter::convert_archive_merged`] folds them into one
//! document (#677) — MarkItDown's `_zip_converter` shape, each entry under a
//! level-2 heading naming its path.
//!
//! A ZIP inside the archive is read too when [`ArchiveLimits::max_depth`]
//! allows it (#677): its entries are listed as `inner.zip/<path>` beside the
//! outer ones and share the outer archive's entry count and size budget, so
//! a bomb cannot hide one level down. The default depth of 1 keeps nested
//! archives shut, as before. Other archive types (tar, 7z, rar, gzip) are
//! never opened.
//!
//! An archive is not one document, so it has no [`InputFormat`]:
//! `InputFormat::from_extension("zip")` stays `None` and
//! [`DocumentConverter::convert`] keeps rejecting it — archives go through
//! this module's API (and the CLI's / serve's archive handling built on it).
//! Office packages that are ZIP files underneath (DOCX, XLSX, ODT, EPUB, …)
//! are documents, not archives, and convert as before.
//!
//! [`DocumentConverter::convert_archive`]: crate::DocumentConverter::convert_archive
//! [`DocumentConverter::convert_archive_merged`]: crate::DocumentConverter::convert_archive_merged
//! [`DocumentConverter::convert`]: crate::DocumentConverter::convert

use std::io::{Cursor, Read, Seek};

use docling_core::{DoclingDocument, Node};

use crate::error::ConversionError;
use crate::format::InputFormat;
use crate::result::ConversionResult;
use crate::source::SourceDocument;
use crate::DocumentConverter;

/// Bounds on what an archive may make the converter decompress — the guard
/// against ZIP bombs and resource exhaustion on untrusted input. Checked
/// against the central directory before anything is inflated; extraction then
/// stops at the size an entry declared, so a header that understates its
/// entry cannot get past them either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveLimits {
    /// Entries considered at most (directories excluded); the rest are
    /// skipped. Default 10 000.
    pub max_entries: usize,
    /// Largest uncompressed entry converted. Default 256 MiB.
    pub max_entry_size: u64,
    /// Uncompressed bytes the converted entries may add up to. Default 1 GiB.
    pub max_total_size: u64,
    /// Largest uncompressed : compressed ratio accepted for an entry over
    /// 1 MiB (text compresses 5–20×; a bomb, 1000× and more). Default 200.
    pub max_compression_ratio: u64,
    /// How deep ZIP archives inside the archive are read (#677): 1 (the
    /// default) converts the archive's own entries and skips the archives in
    /// it, 2 reads the ZIPs it holds too, 3 the ZIPs inside those. Clamped
    /// to `1..=3`. Every level shares the one entry count and size budget,
    /// and a nested archive's own bytes count against it as well.
    pub max_depth: usize,
}

/// The deepest nesting [`ArchiveLimits::max_depth`] can ask for.
pub const MAX_ARCHIVE_DEPTH: usize = 3;

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            max_entries: 10_000,
            max_entry_size: 256 << 20,
            max_total_size: 1 << 30,
            max_compression_ratio: 200,
            max_depth: 1,
        }
    }
}

impl ArchiveLimits {
    /// The defaults with the `DOCLING_RS_ZIP_MAX_ENTRIES`,
    /// `DOCLING_RS_ZIP_MAX_ENTRY_MB`, `DOCLING_RS_ZIP_MAX_TOTAL_MB`,
    /// `DOCLING_RS_ZIP_MAX_RATIO` and `DOCLING_RS_ZIP_MAX_DEPTH` overrides —
    /// what the CLI and serve use.
    pub fn from_env() -> Self {
        let d = Self::default();
        let mb = |name: &str, default: u64| {
            docling_core::env::parse::<u64>(name).map_or(default, |v| v.saturating_mul(1 << 20))
        };
        Self {
            max_entries: docling_core::env::parse("DOCLING_RS_ZIP_MAX_ENTRIES")
                .unwrap_or(d.max_entries),
            max_entry_size: mb("DOCLING_RS_ZIP_MAX_ENTRY_MB", d.max_entry_size),
            max_total_size: mb("DOCLING_RS_ZIP_MAX_TOTAL_MB", d.max_total_size),
            max_compression_ratio: docling_core::env::parse("DOCLING_RS_ZIP_MAX_RATIO")
                .unwrap_or(d.max_compression_ratio),
            max_depth: docling_core::env::parse("DOCLING_RS_ZIP_MAX_DEPTH").unwrap_or(d.max_depth),
        }
    }

    /// `max_depth` as applied: at least 1, at most [`MAX_ARCHIVE_DEPTH`].
    pub fn depth(&self) -> usize {
        self.max_depth.clamp(1, MAX_ARCHIVE_DEPTH)
    }
}

/// Entries this small are never judged by their compression ratio: a page of
/// blanks compresses 1000× legitimately and cannot hurt anyone.
const RATIO_FLOOR: u64 = 1 << 20;

/// One file entry of an archive as listed by [`Archive::open`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveEntryInfo {
    /// The entry's position in [`Archive::entries`] — what
    /// [`Archive::read`] takes.
    pub index: usize,
    /// The entry's path inside the archive, `/`-separated; inside a nested
    /// archive, `inner.zip/<path>` (#677). For a skipped entry with an
    /// unsafe name, the raw name as stored.
    pub path: String,
    /// The format the entry converts as (from its extension); `None` when
    /// it is skipped.
    pub format: Option<InputFormat>,
    /// Why the entry is not converted, when it is not.
    pub skipped: Option<String>,
    /// Uncompressed size as the archive declares it.
    pub size: u64,
}

/// A ZIP archive opened for conversion. See the [module docs](self).
pub struct Archive<R> {
    zip: zip::ZipArchive<R>,
    /// The ZIP archives inside this one, opened under
    /// [`ArchiveLimits::max_depth`] (#677), in the order met.
    nested: Vec<Archive<Cursor<Vec<u8>>>>,
    entries: Vec<ArchiveEntryInfo>,
    /// Where each of `entries` lives, in step with it.
    locations: Vec<Location>,
}

/// Where an entry's bytes are: this archive's central directory, or an
/// entry of a nested archive.
#[derive(Debug, Clone, Copy)]
enum Location {
    Own(usize),
    Nested { archive: usize, index: usize },
}

/// What the whole archive — every nesting level — has used of the limits.
#[derive(Default)]
struct Budget {
    entries: usize,
    total: u64,
}

impl<R: Read + Seek> Archive<R> {
    /// Read the archive's directory and classify every file entry. Fails only
    /// when `reader` is not a readable ZIP archive.
    pub fn open(reader: R, limits: &ArchiveLimits) -> Result<Self, ConversionError> {
        Self::open_at(reader, limits, 1, &mut Budget::default())
    }

    /// [`open`](Self::open) at nesting level `depth` (1 = the archive itself),
    /// drawing on the budget the outer levels share.
    fn open_at(
        reader: R,
        limits: &ArchiveLimits,
        depth: usize,
        budget: &mut Budget,
    ) -> Result<Self, ConversionError> {
        let mut zip = zip::ZipArchive::new(reader)
            .map_err(|e| ConversionError::Parse(format!("zip: {e}")))?;
        let mut nested = Vec::new();
        let mut entries: Vec<ArchiveEntryInfo> = Vec::new();
        let mut locations = Vec::new();
        for index in 0..zip.len() {
            // Raw access reads the local header only — nothing is inflated.
            let Ok(file) = zip.by_index_raw(index) else {
                entries.push(ArchiveEntryInfo {
                    index: entries.len(),
                    path: format!("#{index}"),
                    format: None,
                    skipped: Some("unreadable entry header".into()),
                    size: 0,
                });
                locations.push(Location::Own(index));
                budget.entries += 1;
                continue;
            };
            if file.is_dir() {
                continue;
            }
            let size = file.size();
            let compressed = file.compressed_size();
            let safe = file
                .enclosed_name()
                .map(|p| p.to_string_lossy().replace('\\', "/"));
            let path = safe.clone().unwrap_or_else(|| match file.name() {
                Ok(name) => name.into_owned(),
                Err(_) => String::from_utf8_lossy(file.name_raw()).into_owned(),
            });
            let encrypted = file.encrypted();
            drop(file);
            let skip = |why: &str| Some(why.to_string());
            // The size checks every inflated entry passes — a document, or a
            // nested archive (#677) whose bytes are held while it is read.
            let over_size = |budget: &Budget| {
                if size > limits.max_entry_size {
                    skip("larger than the per-entry size limit")
                } else if size > RATIO_FLOOR
                    && size / compressed.max(1) > limits.max_compression_ratio
                {
                    skip("compression ratio over the limit")
                } else if budget.total.saturating_add(size) > limits.max_total_size {
                    skip("over the archive's total size limit")
                } else {
                    None
                }
            };
            let mut format = None;
            let skipped = if budget.entries >= limits.max_entries {
                skip("over the archive's entry limit")
            } else if safe.is_none() {
                // An absolute path or one climbing out with `..`: never
                // honoured, never written anywhere.
                skip("unsafe path")
            } else if encrypted {
                skip("encrypted")
            } else if is_macos_metadata(&path) {
                skip("macOS metadata")
            } else if is_zip_name(&path) && depth < limits.depth() {
                match over_size(budget) {
                    Some(why) => Some(why),
                    None => {
                        budget.total += size;
                        budget.entries += 1;
                        let inner = inflate(&mut zip, index, size, &path).and_then(|bytes| {
                            Archive::open_at(Cursor::new(bytes), limits, depth + 1, budget)
                        });
                        match inner {
                            Ok(inner) => {
                                let slot = nested.len();
                                for e in &inner.entries {
                                    entries.push(ArchiveEntryInfo {
                                        index: entries.len(),
                                        path: format!("{path}/{}", e.path),
                                        format: e.format,
                                        skipped: e.skipped.clone(),
                                        size: e.size,
                                    });
                                    locations.push(Location::Nested {
                                        archive: slot,
                                        index: e.index,
                                    });
                                }
                                nested.push(inner);
                                continue;
                            }
                            Err(e) => Some(format!("nested archive: {e}")),
                        }
                    }
                }
            } else if is_archive(&path) {
                skip("nested archive")
            } else if let Some(f) = extension(&path).and_then(InputFormat::from_extension) {
                match over_size(budget) {
                    Some(why) => Some(why),
                    None => {
                        budget.total += size;
                        format = Some(f);
                        None
                    }
                }
            } else {
                skip("unsupported file type")
            };
            entries.push(ArchiveEntryInfo {
                index: entries.len(),
                path,
                format,
                skipped,
                size,
            });
            locations.push(Location::Own(index));
            budget.entries += 1;
        }
        Ok(Self {
            zip,
            nested,
            entries,
            locations,
        })
    }

    /// The archive's file entries in directory order, converted or skipped.
    pub fn entries(&self) -> &[ArchiveEntryInfo] {
        &self.entries
    }

    /// Extract the entry at position `index` of [`entries`](Self::entries) as
    /// a source document named after its file stem. Errors for an entry
    /// [`open`](Self::open) skipped, one that fails to inflate, or one that
    /// turns out larger than its header declared.
    pub fn read(&mut self, index: usize) -> Result<SourceDocument, ConversionError> {
        let info = self
            .entries
            .get(index)
            .ok_or_else(|| ConversionError::Parse(format!("zip: no entry #{index}")))?;
        let format = match (&info.format, &info.skipped) {
            (Some(f), None) => *f,
            (_, reason) => {
                return Err(ConversionError::Parse(format!(
                    "zip: {} is skipped ({})",
                    info.path,
                    reason.as_deref().unwrap_or("not convertible")
                )))
            }
        };
        let (path, declared) = (info.path.clone(), info.size);
        let bytes = match self.locations[index] {
            Location::Own(cd) => inflate(&mut self.zip, cd, declared, &path)?,
            Location::Nested { archive, index } => return self.nested[archive].read(index),
        };
        let name = path
            .rsplit('/')
            .next()
            .map(|f| f.rsplit_once('.').map_or(f, |(stem, _)| stem))
            .filter(|s| !s.is_empty())
            .unwrap_or("document")
            .to_string();
        Ok(SourceDocument::from_bytes(name, format, bytes))
    }
}

/// Inflate the entry at central-directory position `cd` — never more than
/// the `declared` size (which the limits vetted).
fn inflate<R: Read + Seek>(
    zip: &mut zip::ZipArchive<R>,
    cd: usize,
    declared: u64,
    path: &str,
) -> Result<Vec<u8>, ConversionError> {
    let file = zip
        .by_index(cd)
        .map_err(|e| ConversionError::Parse(format!("zip: {path}: {e}")))?;
    let mut bytes = Vec::with_capacity(declared.min(64 << 20) as usize);
    file.take(declared.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| ConversionError::Parse(format!("zip: {path}: {e}")))?;
    if bytes.len() as u64 > declared {
        return Err(ConversionError::Parse(format!(
            "zip: {path}: larger than its header declares"
        )));
    }
    Ok(bytes)
}

/// What became of one archive entry.
#[derive(Debug)]
pub enum ArchiveOutcome {
    /// Converted (possibly partially — see the result's status).
    Converted(Box<ConversionResult>),
    /// Not attempted, and why: unsupported type, nested archive, unsafe path,
    /// encryption, or a limit.
    Skipped(String),
    /// Attempted and failed; the other entries are unaffected.
    Failed(ConversionError),
}

/// One entry's path inside the archive and its outcome.
#[derive(Debug)]
pub struct ArchiveItem {
    pub path: String,
    pub outcome: ArchiveOutcome,
}

/// The lazy per-entry conversion [`DocumentConverter::convert_archive`]
/// returns: each `next()` extracts and converts one entry, so memory holds a
/// single document at a time.
///
/// [`DocumentConverter::convert_archive`]: crate::DocumentConverter::convert_archive
pub struct ArchiveConversion<'c, R> {
    converter: &'c DocumentConverter,
    archive: Archive<R>,
    next: usize,
}

impl<R: Read + Seek> Iterator for ArchiveConversion<'_, R> {
    type Item = ArchiveItem;

    fn next(&mut self) -> Option<ArchiveItem> {
        let info = self.archive.entries.get(self.next)?.clone();
        self.next += 1;
        let outcome = match info.skipped {
            Some(reason) => ArchiveOutcome::Skipped(reason),
            None => match self.archive.read(info.index) {
                Ok(source) => match self.converter.convert(source) {
                    Ok(result) => ArchiveOutcome::Converted(Box::new(result)),
                    Err(e) => ArchiveOutcome::Failed(e),
                },
                Err(e) => ArchiveOutcome::Failed(e),
            },
        };
        Some(ArchiveItem {
            path: info.path,
            outcome,
        })
    }
}

impl DocumentConverter {
    /// Convert every document inside a ZIP archive (#557), one at a time: a
    /// lazy iterator of each entry's [`ArchiveOutcome`] in directory order.
    /// Entries convert with this converter's settings; the archive limits come
    /// from [`DocumentConverter::archive_limits`]. Errors only when `reader`
    /// is not a ZIP archive.
    pub fn convert_archive<R: Read + Seek>(
        &self,
        reader: R,
    ) -> Result<ArchiveConversion<'_, R>, ConversionError> {
        Ok(ArchiveConversion {
            converter: self,
            archive: Archive::open(reader, &self.archive_limits_ref())?,
            next: 0,
        })
    }
}

/// One document for a whole archive (#677) — what
/// [`DocumentConverter::convert_archive_merged`] returns: per converted
/// entry, in directory order, a level-2 heading with the entry's path and
/// then the entry's content (MarkItDown's `_zip_converter` shape, `## File:`
/// prefix aside); the entries not in it, and why.
///
/// [`DocumentConverter::convert_archive_merged`]: crate::DocumentConverter::convert_archive_merged
#[derive(Debug)]
pub struct MergedArchive {
    /// The merged document, named after the archive. Page markers of the
    /// entries are dropped (several documents' pages do not make one page
    /// map); the rest of each entry's nodes and links follow its heading.
    pub document: DoclingDocument,
    /// Entries converted into the document.
    pub converted: usize,
    /// Entries not attempted — `(path, reason)`.
    pub skipped: Vec<(String, String)>,
    /// Entries that failed — `(path, error)`; the others are unaffected.
    pub failed: Vec<(String, ConversionError)>,
    /// Problems the converted entries survived (a spent timeout): docling's
    /// `ConversionResult.errors`, each message prefixed with the entry's path.
    pub errors: Vec<crate::result::ErrorItem>,
}

impl DocumentConverter {
    /// Convert every document inside a ZIP archive into one document
    /// (#677): `name` names it, each converted entry contributes a level-2
    /// heading with its path and its content, in directory order. The
    /// document carries this converter's serializer settings like any
    /// [`convert`](Self::convert) result. Errors only when `reader` is not a
    /// ZIP archive; an entry that fails or is skipped is reported in the
    /// result and leaves the others alone.
    pub fn convert_archive_merged<R: Read + Seek>(
        &self,
        name: &str,
        reader: R,
    ) -> Result<MergedArchive, ConversionError> {
        let mut merged = MergedArchive {
            document: DoclingDocument::new(name),
            converted: 0,
            skipped: Vec::new(),
            failed: Vec::new(),
            errors: Vec::new(),
        };
        for item in self.convert_archive(reader)? {
            match item.outcome {
                ArchiveOutcome::Converted(result) => {
                    merged.converted += 1;
                    merged.document.nodes.push(Node::Heading {
                        level: 2,
                        text: item.path.clone(),
                    });
                    merged.document.nodes.extend(
                        result
                            .document
                            .nodes
                            .into_iter()
                            .filter(|n| !matches!(n, Node::PageInfo { .. })),
                    );
                    merged.document.links.extend(result.document.links);
                    merged.errors.extend(result.errors.into_iter().map(|mut e| {
                        e.error_message = format!("{}: {}", item.path, e.error_message);
                        e
                    }));
                }
                ArchiveOutcome::Skipped(reason) => merged.skipped.push((item.path, reason)),
                ArchiveOutcome::Failed(e) => merged.failed.push((item.path, e)),
            }
        }
        self.finish_document(&mut merged.document);
        Ok(merged)
    }
}

/// Whether an entry is a ZIP/tar/7z/rar archive by its extension. A ZIP is
/// descended into under [`ArchiveLimits::max_depth`] (#677); the others
/// never are.
pub fn is_archive(path: &str) -> bool {
    matches!(
        extension(path).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("zip" | "7z" | "rar" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst")
    )
}

/// A `.zip` entry — the one archive type read when the depth allows.
fn is_zip_name(path: &str) -> bool {
    extension(path).is_some_and(|e| e.eq_ignore_ascii_case("zip"))
}

/// `__MACOSX/…` resource forks and `._name` AppleDouble files: Finder's
/// metadata, never documents.
fn is_macos_metadata(path: &str) -> bool {
    path.starts_with("__MACOSX/") || path.rsplit('/').next().is_some_and(|f| f.starts_with("._"))
}

fn extension(path: &str) -> Option<&str> {
    let file = path.rsplit('/').next()?;
    file.rsplit_once('.')
        .map(|(_, ext)| ext)
        .filter(|e| !e.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        let mut w = zip::ZipWriter::new(&mut out);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            if name.ends_with('/') {
                w.add_directory(name.trim_end_matches('/'), opts).unwrap();
            } else {
                w.start_file(*name, opts).unwrap();
                w.write_all(data).unwrap();
            }
        }
        w.finish().unwrap();
        out.into_inner()
    }

    #[test]
    fn entries_are_classified_before_inflating() {
        let data = zip_of(&[
            ("docs/", b""),
            ("docs/a.md", b"# A\n\nalpha"),
            ("docs/b.html", b"<html><body><p>beta</p></body></html>"),
            ("tool.exe", b"MZ"),
            ("inner.zip", b"PK\x03\x04"),
            ("__MACOSX/docs/._a.md", b"junk"),
            ("noext", b"x"),
        ]);
        let archive = Archive::open(Cursor::new(data), &ArchiveLimits::default()).unwrap();
        let got: Vec<(&str, Option<&str>)> = archive
            .entries()
            .iter()
            .map(|e| (e.path.as_str(), e.skipped.as_deref()))
            .collect();
        assert_eq!(
            got,
            [
                ("docs/a.md", None),
                ("docs/b.html", None),
                ("tool.exe", Some("unsupported file type")),
                ("inner.zip", Some("nested archive")),
                ("__MACOSX/docs/._a.md", Some("macOS metadata")),
                ("noext", Some("unsupported file type")),
            ]
        );
    }

    #[test]
    fn convert_archive_reports_every_entry() {
        let data = zip_of(&[
            ("a.md", b"# A\n\nalpha"),
            ("broken.docx", b"not a zip at all"),
            ("c.exe", b"MZ"),
        ]);
        let conv = DocumentConverter::new();
        let items: Vec<ArchiveItem> = conv.convert_archive(Cursor::new(data)).unwrap().collect();
        assert_eq!(items.len(), 3);
        match &items[0].outcome {
            ArchiveOutcome::Converted(r) => {
                assert_eq!(r.input_name, "a");
                assert!(r.document.export_to_markdown().contains("alpha"));
            }
            other => panic!("a.md: {other:?}"),
        }
        assert!(
            matches!(items[1].outcome, ArchiveOutcome::Failed(_)),
            "{:?}",
            items[1]
        );
        assert!(matches!(items[2].outcome, ArchiveOutcome::Skipped(_)));
    }

    #[test]
    fn limits_stop_entries_before_decompression() {
        let big = vec![b'a'; 3 << 20]; // 3 MiB of one byte: ratio ≫ 200
        let data = zip_of(&[("a.md", b"alpha"), ("bomb.md", &big), ("c.md", b"gamma")]);
        let limits = ArchiveLimits::default();
        let archive = Archive::open(Cursor::new(data.clone()), &limits).unwrap();
        assert_eq!(
            archive.entries()[1].skipped.as_deref(),
            Some("compression ratio over the limit")
        );
        let tight = ArchiveLimits {
            max_entries: 2,
            max_entry_size: 4,
            ..limits
        };
        let archive = Archive::open(Cursor::new(data), &tight).unwrap();
        let skipped: Vec<_> = archive
            .entries()
            .iter()
            .map(|e| e.skipped.as_deref())
            .collect();
        assert_eq!(
            skipped,
            [
                Some("larger than the per-entry size limit"),
                Some("larger than the per-entry size limit"),
                Some("over the archive's entry limit"),
            ]
        );
    }

    #[test]
    fn unsafe_paths_are_never_extracted() {
        // `..` climbing out is refused; a leading `/` is read as relative
        // (the zip crate's `enclosed_name`), so it stays inside the target.
        let data = zip_of(&[("../escape.md", b"x"), ("/abs.md", b"y"), ("ok.md", b"z")]);
        let mut archive = Archive::open(Cursor::new(data), &ArchiveLimits::default()).unwrap();
        let listed: Vec<(&str, Option<&str>)> = archive
            .entries()
            .iter()
            .map(|e| (e.path.as_str(), e.skipped.as_deref()))
            .collect();
        assert_eq!(
            listed,
            [
                ("../escape.md", Some("unsafe path")),
                ("abs.md", None),
                ("ok.md", None)
            ]
        );
        assert!(archive.read(0).is_err());
        assert_eq!(archive.read(1).unwrap().bytes, b"y");
    }

    /// #677: a ZIP inside the archive opens under `max_depth` — its entries
    /// listed as `inner.zip/<path>` and readable — and stays a skipped
    /// "nested archive" at the default depth of 1; the depth clamps to 3;
    /// a `.zip` that is not one is skipped with the reason; tar and the
    /// like are never opened.
    #[test]
    fn nested_zips_open_under_max_depth() {
        let deepest = zip_of(&[("deep.md", b"# Deep")]);
        let inner = zip_of(&[
            ("doc.md", b"# Inner"),
            ("more/inner2.zip", &deepest),
            ("tool.exe", b"MZ"),
        ]);
        let data = zip_of(&[
            ("a.md", b"# A"),
            ("inner.zip", &inner),
            ("fake.zip", b"nope"),
            ("notes.tar", b"x"),
        ]);
        let listed = |archive: &Archive<Cursor<Vec<u8>>>| -> Vec<(String, Option<String>)> {
            archive
                .entries()
                .iter()
                .map(|e| (e.path.clone(), e.skipped.clone()))
                .collect()
        };
        let shut = Archive::open(Cursor::new(data.clone()), &ArchiveLimits::default()).unwrap();
        assert_eq!(
            listed(&shut),
            [
                ("a.md".into(), None),
                ("inner.zip".into(), Some("nested archive".into())),
                ("fake.zip".into(), Some("nested archive".into())),
                ("notes.tar".into(), Some("nested archive".into())),
            ]
        );

        let two = ArchiveLimits {
            max_depth: 2,
            ..ArchiveLimits::default()
        };
        let mut archive = Archive::open(Cursor::new(data.clone()), &two).unwrap();
        let got = listed(&archive);
        assert_eq!(
            got[..4],
            [
                ("a.md".into(), None),
                ("inner.zip/doc.md".into(), None),
                (
                    "inner.zip/more/inner2.zip".into(),
                    Some("nested archive".into())
                ),
                (
                    "inner.zip/tool.exe".into(),
                    Some("unsupported file type".into())
                ),
            ]
        );
        assert!(
            got[4].0 == "fake.zip" && got[4].1.as_deref().unwrap().starts_with("nested archive: "),
            "{:?}",
            got[4]
        );
        assert_eq!(got[5], ("notes.tar".into(), Some("nested archive".into())));
        assert_eq!(got.len(), 6);
        let doc = archive.read(1).unwrap();
        assert_eq!(
            (doc.name.as_str(), doc.bytes.as_slice()),
            ("doc", &b"# Inner"[..])
        );
        assert!(archive.read(2).is_err());

        let deep = ArchiveLimits {
            max_depth: 9,
            ..ArchiveLimits::default()
        };
        assert_eq!(deep.depth(), 3);
        let mut archive = Archive::open(Cursor::new(data), &deep).unwrap();
        let position = archive
            .entries()
            .iter()
            .position(|e| e.path == "inner.zip/more/inner2.zip/deep.md" && e.skipped.is_none())
            .expect("the third level is read");
        assert_eq!(archive.read(position).unwrap().bytes, b"# Deep");
    }

    /// #677: the levels share one budget — a bomb one level down trips the
    /// ratio, the entry count and the total size count every level, and the
    /// nested archive's own bytes count too.
    #[test]
    fn nested_archives_share_the_budget() {
        let big = vec![b'a'; 3 << 20];
        let inner = zip_of(&[("bomb.md", &big), ("ok.md", b"fine")]);
        let data = zip_of(&[("a.md", b"alpha"), ("inner.zip", &inner)]);
        let two = ArchiveLimits {
            max_depth: 2,
            ..ArchiveLimits::default()
        };
        let skipped = |limits: &ArchiveLimits| -> Vec<(String, Option<String>)> {
            Archive::open(Cursor::new(data.clone()), limits)
                .unwrap()
                .entries()
                .iter()
                .map(|e| (e.path.clone(), e.skipped.clone()))
                .collect()
        };
        assert_eq!(
            skipped(&two),
            [
                ("a.md".into(), None),
                (
                    "inner.zip/bomb.md".into(),
                    Some("compression ratio over the limit".into())
                ),
                ("inner.zip/ok.md".into(), None),
            ]
        );
        // Two entries in all: `a.md` and the nested archive itself.
        assert_eq!(
            skipped(&ArchiveLimits {
                max_entries: 2,
                ..two
            }),
            [
                ("a.md".into(), None),
                (
                    "inner.zip/bomb.md".into(),
                    Some("over the archive's entry limit".into())
                ),
                (
                    "inner.zip/ok.md".into(),
                    Some("over the archive's entry limit".into())
                ),
            ]
        );
        // The nested archive's bytes are inflated, so they are budgeted.
        assert_eq!(
            skipped(&ArchiveLimits {
                max_total_size: inner.len() as u64 + 2,
                ..two
            }),
            [
                ("a.md".into(), None),
                (
                    "inner.zip".into(),
                    Some("over the archive's total size limit".into())
                ),
            ]
        );
    }

    /// #677: the merged document — a level-2 heading per converted entry
    /// over its content, failures and skips reported beside it.
    #[test]
    fn convert_archive_merged_is_one_document() {
        let data = zip_of(&[
            ("a.md", b"# A\n\nalpha"),
            ("sub/b.md", b"- one\n- two"),
            ("broken.docx", b"not a zip at all"),
            ("tool.exe", b"MZ"),
        ]);
        let merged = DocumentConverter::new()
            .convert_archive_merged("bundle", Cursor::new(data))
            .unwrap();
        assert_eq!(merged.document.name, "bundle");
        assert_eq!(merged.converted, 2);
        assert_eq!(
            merged.skipped,
            [("tool.exe".to_string(), "unsupported file type".to_string())]
        );
        assert_eq!(merged.failed.len(), 1, "{:?}", merged.failed);
        assert_eq!(merged.failed[0].0, "broken.docx");
        assert!(merged.errors.is_empty());
        assert_eq!(
            merged.document.export_to_markdown(),
            "## a.md\n\n# A\n\nalpha\n\n## sub/b.md\n\n- one\n- two\n"
        );
    }

    #[test]
    fn not_a_zip_is_an_error() {
        assert!(Archive::open(Cursor::new(b"plain".to_vec()), &ArchiveLimits::default()).is_err());
        // An archive has no InputFormat: `convert` keeps rejecting `.zip`.
        assert_eq!(InputFormat::from_extension("zip"), None);
    }
}
