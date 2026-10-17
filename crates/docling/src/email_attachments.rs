//! Email attachments as input (#561): the payloads of an `.eml` / `.msg`.
//!
//! The email backend renders a message's headers and body, and with
//! `list_attachments` the attachments' *names*; the bytes themselves stayed
//! out of reach, so a consumer needing the PDF inside a message had to parse
//! it again with another library. [`EmailAttachments`] is the counterpart of
//! [`Archive`](crate::archive::Archive) for a message: [`EmailAttachments::open`]
//! lists every attachment with its name, media type, size and the
//! [`InputFormat`] it would convert as — or why it will not convert — and
//! [`EmailAttachments::read`] hands one over as a [`SourceDocument`];
//! [`EmailAttachments::data`] gives the raw bytes of any attachment that was
//! kept, convertible or not (a `.zip` goes on to
//! [`DocumentConverter::convert_archive`], an unknown blob to whatever the
//! caller has). [`DocumentConverter::convert_email_attachments`] converts them
//! one at a time, like an archive's entries.
//!
//! A forwarded message is an attachment too: a `message/rfc822` part of an
//! `.eml`, or an embedded message storage of a `.msg`, is listed as an `.eml`
//! entry whose payload is the nested message (one level — its own attachments
//! stay inside it). Attachments by reference (a path on the sender's disk)
//! and OLE objects carry no payload and are listed as skipped; TNEF, S/MIME
//! and PGP envelopes are opaque payloads like any other, not decoded here.
//!
//! The [`ArchiveLimits`] bound what is kept, as they do for an archive; the
//! compression ratio does not apply (MIME has no compression to bomb with).
//! File names are reduced to a safe base name — a path in a `filename=`
//! parameter never climbs anywhere, bidirectional controls that disguise an
//! extension are dropped — and made unique within the message (a second
//! `report.pdf` is `report-2.pdf`), so a caller can write every `name` to one
//! directory as it is. Payloads are the bytes as sent, with only the
//! transfer encoding (base64, quoted-printable) undone: a text attachment
//! keeps its own charset, which is what its `<meta charset>` or the caller's
//! decoder expects (#564).
//!
//! [`DocumentConverter::convert_archive`]: crate::DocumentConverter::convert_archive
//! [`DocumentConverter::convert_email_attachments`]: crate::DocumentConverter::convert_email_attachments

use std::borrow::Cow;

use mail_parser::{Encoding, MessageParser, MimeHeaders, PartType};

use crate::archive::{is_archive, ArchiveLimits, ArchiveOutcome};
use crate::error::ConversionError;
use crate::format::InputFormat;
use crate::source::SourceDocument;
use crate::DocumentConverter;

/// One attachment of a message as listed by [`EmailAttachments::open`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailAttachmentInfo {
    /// Position among the message's attachments (for
    /// [`EmailAttachments::read`] / [`EmailAttachments::data`]).
    pub index: usize,
    /// A safe file name, unique within the message: the declared name
    /// reduced to its base name with path separators, control and
    /// bidirectional-format characters removed, `attachment-N` (plus the
    /// extension its media type implies) when the message declares none,
    /// `<subject>.eml` for a forwarded message; a repeated name gets `-2`,
    /// `-3`, … before its extension.
    pub name: String,
    /// The declared media type (`type/subtype`, lower-case), when there is one.
    pub content_type: Option<String>,
    /// The format the attachment converts as — from its extension, else its
    /// media type, else the bytes' own signature; `None` when it is skipped.
    pub format: Option<InputFormat>,
    /// Payload size in bytes (0 for a payload-less attachment).
    pub size: u64,
    /// Whether it is an image the message shows inline — part of the HTML
    /// body (`Content-Disposition: inline` or a `Content-ID` on an image
    /// part, Outlook's hidden flag) rather than a file to open. Only images:
    /// a PDF some mailers dispose `inline` is still an attachment here.
    pub inline: bool,
    /// Why the attachment is not converted, when it is not: no payload
    /// (reference, OLE object), over a limit, an archive, or an unsupported
    /// type. The bytes of an unsupported type or an archive are still
    /// available through [`EmailAttachments::data`]; those over a limit are
    /// not kept.
    pub skipped: Option<String>,
}

/// A message's attachments, opened for conversion. See the [module docs](self).
pub struct EmailAttachments {
    entries: Vec<EmailAttachmentInfo>,
    payloads: Vec<Option<Vec<u8>>>,
}

/// An attachment as the message format stores it, before classification.
struct RawAttachment<'a> {
    name: Option<String>,
    content_type: Option<String>,
    /// `None` for an attachment without bytes (by reference, OLE). Borrowed
    /// from the message where it can be, so a payload the limits drop is
    /// never copied.
    payload: Option<Cow<'a, [u8]>>,
    inline: bool,
    /// A forwarded message (`message/rfc822`, an embedded `.msg`).
    message: bool,
    /// Why there is no payload, when there is none.
    missing: Option<&'static str>,
}

impl EmailAttachments {
    /// Parse `bytes` — an `.eml`, or an Outlook `.msg` (by its CFB
    /// signature) — and classify every attachment. Fails only when the bytes
    /// are not a message.
    pub fn open(bytes: &[u8], limits: &ArchiveLimits) -> Result<Self, ConversionError> {
        let raw = if crate::backend::cfb::CompoundFile::detect(bytes) {
            msg_attachments(bytes)?
        } else {
            eml_attachments(bytes)?
        };
        let mut entries: Vec<EmailAttachmentInfo> = Vec::with_capacity(raw.len());
        let mut payloads = Vec::with_capacity(raw.len());
        let mut total: u64 = 0;
        for (index, att) in raw.into_iter().enumerate() {
            let size = att.payload.as_ref().map_or(0, |p| p.len() as u64);
            let content_type = att
                .content_type
                .as_deref()
                .map(|ct| ct.trim().to_ascii_lowercase())
                .filter(|ct| !ct.is_empty());
            let name = unique_name(
                safe_name(
                    att.name.as_deref(),
                    content_type.as_deref(),
                    index,
                    att.message,
                ),
                &entries,
            );
            let skip = |why: &str| Some(why.to_string());
            let mut format = None;
            let mut keep = att.payload.is_some();
            let skipped = if entries.len() >= limits.max_entries {
                keep = false;
                skip("over the entry limit")
            } else if att.payload.is_none() {
                skip(att.missing.unwrap_or("no payload"))
            } else if size > limits.max_entry_size {
                keep = false;
                skip("larger than the per-entry size limit")
            } else if total.saturating_add(size) > limits.max_total_size {
                keep = false;
                skip("over the total size limit")
            } else {
                total += size;
                if att.message {
                    format = Some(InputFormat::Email);
                    None
                } else if is_archive(&name) {
                    // Not descended into — the caller can hand the bytes to
                    // `convert_archive`, which applies its own limits.
                    skip("nested archive")
                } else {
                    format = detect_format(&name, content_type.as_deref(), att.payload.as_deref());
                    if format.is_some() {
                        None
                    } else {
                        skip("unsupported file type")
                    }
                }
            };
            // Inline means "an image of the body": Apple Mail and others
            // dispose a PDF `inline` to preview it, and a caller skipping
            // inline parts to drop logos must not lose it (#564).
            let is_image = content_type
                .as_deref()
                .is_some_and(|ct| ct.starts_with("image/"))
                || matches!(format, Some(InputFormat::Image | InputFormat::Svg));
            entries.push(EmailAttachmentInfo {
                index,
                name,
                content_type,
                format,
                size,
                inline: att.inline && is_image,
                skipped,
            });
            payloads.push(if keep {
                att.payload.map(Cow::into_owned)
            } else {
                None
            });
        }
        Ok(Self { entries, payloads })
    }

    /// The attachments in message order, convertible or skipped.
    pub fn entries(&self) -> &[EmailAttachmentInfo] {
        &self.entries
    }

    /// The raw bytes of attachment `index`, when they were kept: every
    /// attachment with a payload within the limits, convertible or not.
    pub fn data(&self, index: usize) -> Option<&[u8]> {
        self.payloads.get(index)?.as_deref()
    }

    /// Attachment `index` as a source document named after its file stem.
    /// Errors for an attachment [`open`](Self::open) skipped.
    pub fn read(&self, index: usize) -> Result<SourceDocument, ConversionError> {
        let info = self
            .entries
            .get(index)
            .ok_or_else(|| ConversionError::Parse(format!("email: no attachment #{index}")))?;
        let format = match (&info.format, &info.skipped) {
            (Some(f), None) => *f,
            (_, reason) => {
                return Err(ConversionError::Parse(format!(
                    "email: {} is skipped ({})",
                    info.name,
                    reason.as_deref().unwrap_or("not convertible")
                )))
            }
        };
        let bytes = self
            .data(index)
            .ok_or_else(|| ConversionError::Parse(format!("email: {} has no payload", info.name)))?
            .to_vec();
        let stem = info
            .name
            .rsplit_once('.')
            .map_or(info.name.as_str(), |(stem, _)| stem);
        let stem = if stem.is_empty() { "attachment" } else { stem };
        Ok(SourceDocument::from_bytes(stem, format, bytes))
    }
}

/// The attachments of an RFC 822 message: mail-parser's `attachments()` —
/// every leaf part that is not the text or HTML body — including the inline
/// images of `multipart/related` and nested `message/rfc822` parts.
fn eml_attachments<'a>(bytes: &'a [u8]) -> Result<Vec<RawAttachment<'a>>, ConversionError> {
    let msg = MessageParser::default()
        .parse(bytes)
        .ok_or_else(|| ConversionError::Parse("email: could not parse message".into()))?;
    Ok(msg
        .attachments()
        .map(|part| {
            // mail-parser decodes a text part's body into a UTF-8 string —
            // its own charset gone, undecodable bytes U+FFFD — which is the
            // wrong payload for an attachment (#564): an HTML file declaring
            // `windows-1252` would come back mojibake. Take the bytes as
            // sent off the message and undo only the transfer encoding (for
            // a binary part that is what `contents()` holds too, and an
            // unencoded one is then borrowed, not copied, until the limits
            // have kept it); `contents()` is the fallback for a span the
            // parser could not place.
            let payload: Cow<'a, [u8]> = transfer_decoded(bytes, part)
                .unwrap_or_else(|| Cow::Owned(part.contents().to_vec()));
            let content_type = part.content_type().map(|ct| match ct.subtype() {
                Some(sub) => format!("{}/{sub}", ct.ctype()),
                None => ct.ctype().to_string(),
            });
            let disposition = part.content_disposition();
            // A part disposed `attachment` is a file whatever else it says;
            // otherwise an `inline` disposition or a Content-ID (what the HTML
            // body's `<img src="cid:…">` points at) marks an inline part.
            let inline = !disposition.is_some_and(|d| d.is_attachment())
                && (disposition.is_some_and(|d| d.is_inline()) || part.content_id().is_some());
            let message = matches!(part.body, PartType::Message(_));
            // A forwarded message without a `filename=` takes its subject.
            let name = part.attachment_name().map(str::to_string).or_else(|| {
                part.message()
                    .and_then(|m| m.subject())
                    .map(|s| format!("{}.eml", subject_stem(s)))
            });
            RawAttachment {
                name,
                content_type: if message {
                    Some("message/rfc822".into())
                } else {
                    content_type
                },
                payload: Some(payload),
                inline,
                message,
                missing: None,
            }
        })
        .collect())
}

/// A part's body bytes as sent, with the `Content-Transfer-Encoding`
/// undone and nothing else: mail-parser's own base64 / quoted-printable
/// decoders over the raw body span. `None` when the span or the decode is
/// off (the caller then falls back to mail-parser's decoded contents).
fn transfer_decoded<'a>(
    message: &'a [u8],
    part: &mail_parser::MessagePart<'_>,
) -> Option<Cow<'a, [u8]>> {
    let raw = message.get(part.offset_body as usize..part.offset_end as usize)?;
    match part.encoding {
        Encoding::None => Some(Cow::Borrowed(raw)),
        Encoding::Base64 => mail_parser::decoders::base64::base64_decode(raw).map(Cow::Owned),
        Encoding::QuotedPrintable => {
            mail_parser::decoders::quoted_printable::quoted_printable_decode(raw).map(Cow::Owned)
        }
    }
}

/// A forwarded message's subject as a file stem: `Re: Q3/Q4 plan` is one
/// name, not a path — the separators [`safe_name`] would split on become
/// dashes.
fn subject_stem(subject: &str) -> String {
    subject.replace(['/', '\\'], "-")
}

/// The attachments of an Outlook `.msg` (see [`crate::backend::msg`]).
fn msg_attachments(bytes: &[u8]) -> Result<Vec<RawAttachment<'static>>, ConversionError> {
    let atts = crate::backend::msg::attachments(bytes)
        .ok_or_else(|| ConversionError::Parse("email: could not parse .msg container".into()))?;
    Ok(atts
        .into_iter()
        .map(|a| {
            let missing = match a.method {
                5 => Some("embedded message could not be read"),
                6 => Some("OLE object (no payload)"),
                2..=4 | 7 => Some("attachment by reference (no payload)"),
                _ => Some("no payload"),
            };
            RawAttachment {
                name: a.name,
                content_type: a.mime,
                missing: a.payload.is_none().then_some(missing).flatten(),
                payload: a.payload.map(Cow::Owned),
                inline: a.inline,
                message: a.method == 5,
            }
        })
        .collect())
}

/// The format an attachment converts as: its name's extension first (what
/// the sender called it), then the declared media type (a `.bin` or nameless
/// part sent as `application/pdf`), then the bytes' own signature.
fn detect_format(
    name: &str,
    content_type: Option<&str>,
    payload: Option<&[u8]>,
) -> Option<InputFormat> {
    name.rsplit_once('.')
        .and_then(|(_, ext)| InputFormat::from_extension(ext))
        .or_else(|| content_type.and_then(InputFormat::from_mime))
        .or_else(|| payload.and_then(crate::sniff::detect))
}

/// Unicode format characters that change how a name *reads* without being
/// part of it: the bidi overrides and embeddings (`U+202E` turns
/// `invoice\u{202E}fdp.exe` into "invoice exe.pdf" on screen), the
/// zero-width joiners / spaces, the BOM. Dropped from file names (#564).
fn is_disguise(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
            | '\u{061C}'
            | '\u{180E}'
    )
}

/// A file name safe to write into a directory: the base name of whatever the
/// message declared (`/`, `\` and `..` segments dropped), control and
/// bidirectional-format characters removed, one line; `attachment-N` with
/// the media type's extension when nothing usable remains.
fn safe_name(
    declared: Option<&str>,
    content_type: Option<&str>,
    index: usize,
    message: bool,
) -> String {
    let base = declared
        .map(|n| {
            n.rsplit(['/', '\\'])
                .next()
                .unwrap_or("")
                .chars()
                .filter(|c| !c.is_control() && !is_disguise(*c))
                .collect::<String>()
        })
        .map(|n| n.trim().trim_matches('.').to_string())
        .filter(|n| !n.is_empty() && n != "..");
    match base {
        Some(n) => n,
        None if message => format!("message-{}.eml", index + 1),
        None => {
            let ext = content_type
                .and_then(InputFormat::from_mime)
                .and_then(default_extension)
                .map_or(String::new(), |e| format!(".{e}"));
            format!("attachment-{}{ext}", index + 1)
        }
    }
}

/// `name` made unique among `taken`: `report.pdf` already listed →
/// `report-2.pdf`, then `report-3.pdf`, … (the counter before the
/// extension), so writing every attachment to one directory overwrites
/// nothing (#564).
fn unique_name(name: String, taken: &[EmailAttachmentInfo]) -> String {
    let used = |n: &str| taken.iter().any(|e| e.name == n);
    if !used(&name) {
        return name;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), format!(".{ext}")),
        _ => (name.clone(), String::new()),
    };
    (2..)
        .map(|n| format!("{stem}-{n}{ext}"))
        .find(|candidate| !used(candidate))
        .expect("an unbounded counter finds a free name")
}

/// The extension a nameless attachment of this format gets, so its
/// `SourceDocument` name says what it is.
fn default_extension(format: InputFormat) -> Option<&'static str> {
    Some(match format {
        InputFormat::Pdf => "pdf",
        InputFormat::Docx => "docx",
        InputFormat::Pptx => "pptx",
        InputFormat::Xlsx => "xlsx",
        InputFormat::Doc => "doc",
        InputFormat::Xls => "xls",
        InputFormat::Ppt => "ppt",
        InputFormat::Html => "html",
        InputFormat::Md => "md",
        InputFormat::Csv => "csv",
        InputFormat::Rtf => "rtf",
        InputFormat::Odt => "odt",
        InputFormat::Ods => "ods",
        InputFormat::Odp => "odp",
        InputFormat::Epub => "epub",
        InputFormat::Email => "eml",
        InputFormat::Image => "png",
        InputFormat::Audio => "mp3",
        InputFormat::Feed => "rss",
        InputFormat::Plugin => "bin",
        InputFormat::Video => "mp4",
        InputFormat::Latex => "tex",
        InputFormat::JsonDocling => "json",
        InputFormat::Mhtml => "mhtml",
        _ => return None,
    })
}

/// One attachment's name and what became of it.
#[derive(Debug)]
pub struct EmailAttachmentItem {
    pub name: String,
    pub outcome: ArchiveOutcome,
}

/// The lazy per-attachment conversion
/// [`DocumentConverter::convert_email_attachments`] returns: each `next()`
/// converts one attachment.
///
/// [`DocumentConverter::convert_email_attachments`]: crate::DocumentConverter::convert_email_attachments
pub struct EmailAttachmentConversion<'c> {
    converter: &'c DocumentConverter,
    attachments: EmailAttachments,
    next: usize,
}

impl Iterator for EmailAttachmentConversion<'_> {
    type Item = EmailAttachmentItem;

    fn next(&mut self) -> Option<EmailAttachmentItem> {
        let info = self.attachments.entries.get(self.next)?.clone();
        self.next += 1;
        let outcome = match info.skipped {
            Some(reason) => ArchiveOutcome::Skipped(reason),
            None => match self.attachments.read(info.index) {
                Ok(source) => match self.converter.convert(source) {
                    Ok(result) => ArchiveOutcome::Converted(Box::new(result)),
                    Err(e) => ArchiveOutcome::Failed(e),
                },
                Err(e) => ArchiveOutcome::Failed(e),
            },
        };
        Some(EmailAttachmentItem {
            name: info.name,
            outcome,
        })
    }
}

impl DocumentConverter {
    /// Convert every attachment of an `.eml` / `.msg` (#561), one at a time: a
    /// lazy iterator of each attachment's [`ArchiveOutcome`] in message order.
    /// Attachments convert with this converter's settings and are bounded by
    /// its [`archive_limits`](DocumentConverter::archive_limits). Errors only
    /// when `bytes` is not a message.
    pub fn convert_email_attachments(
        &self,
        bytes: &[u8],
    ) -> Result<EmailAttachmentConversion<'_>, ConversionError> {
        Ok(EmailAttachmentConversion {
            converter: self,
            attachments: EmailAttachments::open(bytes, &self.archive_limits_ref())?,
            next: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/data/email/sources")
            .join(name);
        std::fs::read(path).expect("fixture")
    }

    /// A multipart/mixed message with a body and the given attachment parts
    /// (each: its headers, then its base64 payload).
    /// RFC 4648 base64 (the crate carries no encoder; the parser decodes).
    fn base64(bytes: &[u8]) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk.len();
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let v = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            for i in 0..4 {
                if i <= n {
                    out.push(T[((v >> (18 - 6 * i)) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    fn eml(parts: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = String::from(
            "From: A <a@x.com>\r\nTo: B <b@y.com>\r\nSubject: Outer\r\nMIME-Version: 1.0\r\n\
             Content-Type: multipart/mixed; boundary=\"bb\"\r\n\r\n\
             --bb\r\nContent-Type: text/plain\r\n\r\nBody.\r\n",
        );
        for (headers, payload) in parts {
            out.push_str("--bb\r\n");
            out.push_str(headers);
            out.push_str("Content-Transfer-Encoding: base64\r\n\r\n");
            out.push_str(&base64(payload));
            out.push_str("\r\n");
        }
        out.push_str("--bb--\r\n");
        out.into_bytes()
    }

    fn open(bytes: &[u8]) -> EmailAttachments {
        EmailAttachments::open(bytes, &ArchiveLimits::default()).unwrap()
    }

    /// `(name, content_type, format, inline, skipped)` of one entry.
    type Row<'a> = (
        &'a str,
        Option<&'a str>,
        Option<InputFormat>,
        bool,
        Option<&'a str>,
    );

    fn rows(atts: &EmailAttachments) -> Vec<Row<'_>> {
        atts.entries()
            .iter()
            .map(|e| {
                (
                    e.name.as_str(),
                    e.content_type.as_deref(),
                    e.format,
                    e.inline,
                    e.skipped.as_deref(),
                )
            })
            .collect()
    }

    #[test]
    fn eml_fixture_exposes_the_text_attachment() {
        let atts = open(&fixture("eml_with_attachment.eml"));
        assert_eq!(atts.entries().len(), 1);
        let e = &atts.entries()[0];
        assert_eq!(e.name, "test.txt");
        assert_eq!(e.content_type.as_deref(), Some("text/plain"));
        assert_eq!(e.format, Some(InputFormat::Md));
        assert!(!e.inline);
        assert_eq!(e.skipped, None);
        let data = atts.data(0).unwrap();
        assert_eq!(e.size, data.len() as u64);
        assert!(
            data.starts_with(b"This is a test attachment file."),
            "{data:?}"
        );
        let src = atts.read(0).unwrap();
        assert_eq!((src.name.as_str(), src.format), ("test", InputFormat::Md));
    }

    #[test]
    fn msg_fixture_exposes_both_attachments() {
        let atts = open(&fixture("msg_with_attachment.msg"));
        let got: Vec<(&str, Option<&str>, Option<InputFormat>, u64)> = atts
            .entries()
            .iter()
            .map(|e| (e.name.as_str(), e.content_type.as_deref(), e.format, e.size))
            .collect();
        assert_eq!(
            got,
            [
                ("test.txt", Some("text/plain"), Some(InputFormat::Md), 64),
                (
                    "report.pdf",
                    Some("application/pdf"),
                    Some(InputFormat::Pdf),
                    26
                ),
            ]
        );
        assert_eq!(atts.data(0).unwrap().len(), 64);
        assert!(atts
            .entries()
            .iter()
            .all(|e| e.skipped.is_none() && !e.inline));
    }

    /// Classification: the extension first, then the media type, then the
    /// bytes; inline parts flagged; a forwarded message is an `.eml` entry
    /// carrying the nested message; archives are kept but not converted.
    #[test]
    fn eml_parts_are_classified_and_named_safely() {
        let nested = "From: C <c@z.com>\r\nSubject: Inner note\r\n\r\nInner body.\r\n";
        let m = eml(&[
            (
                "Content-Type: application/octet-stream\r\n\
                 Content-Disposition: attachment; filename=\"report.pdf\"\r\n",
                b"%PDF-1.4 fake",
            ),
            (
                "Content-Type: application/pdf\r\n\
                 Content-Disposition: attachment; filename=\"scan.bin\"\r\n",
                b"%PDF-1.4 by type",
            ),
            (
                "Content-Type: image/png\r\nContent-ID: <logo@x>\r\n\
                 Content-Disposition: inline; filename=\"logo.png\"\r\n",
                b"\x89PNG\r\n\x1a\n",
            ),
            (
                "Content-Type: message/rfc822\r\nContent-Disposition: attachment\r\n",
                nested.as_bytes(),
            ),
            (
                "Content-Type: application/zip\r\n\
                 Content-Disposition: attachment; filename=\"../../etc/bundle.zip\"\r\n",
                b"PK\x03\x04",
            ),
            (
                "Content-Type: application/octet-stream\r\nContent-Disposition: attachment\r\n",
                b"<html><body><p>sniffed</p></body></html>",
            ),
            (
                "Content-Type: application/x-unknown\r\n\
                 Content-Disposition: attachment; filename=\"tool.exe\"\r\n",
                b"MZ",
            ),
        ]);
        let atts = open(&m);
        let got = rows(&atts);
        assert_eq!(
            got,
            [
                (
                    "report.pdf",
                    Some("application/octet-stream"),
                    Some(InputFormat::Pdf),
                    false,
                    None
                ),
                (
                    "scan.bin",
                    Some("application/pdf"),
                    Some(InputFormat::Pdf),
                    false,
                    None
                ),
                (
                    "logo.png",
                    Some("image/png"),
                    Some(InputFormat::Image),
                    true,
                    None
                ),
                (
                    "Inner note.eml",
                    Some("message/rfc822"),
                    Some(InputFormat::Email),
                    false,
                    None
                ),
                (
                    "bundle.zip",
                    Some("application/zip"),
                    None,
                    false,
                    Some("nested archive")
                ),
                (
                    "attachment-6",
                    Some("application/octet-stream"),
                    Some(InputFormat::Html),
                    false,
                    None
                ),
                (
                    "tool.exe",
                    Some("application/x-unknown"),
                    None,
                    false,
                    Some("unsupported file type")
                ),
            ]
        );
        // The nested message's payload is the message itself.
        let inner = atts.read(3).unwrap();
        assert_eq!(
            (inner.name.as_str(), inner.format),
            ("Inner note", InputFormat::Email)
        );
        assert_eq!(inner.bytes, nested.as_bytes());
        // Skipped for type, not for safety: the bytes are still there…
        assert_eq!(atts.data(4).unwrap(), b"PK\x03\x04");
        assert_eq!(atts.data(6).unwrap(), b"MZ");
        // …but `read` refuses what will not convert.
        assert!(atts.read(4).is_err());
    }

    /// #564: a text attachment's bytes are the bytes as sent, only the
    /// transfer encoding undone — a windows-1252 HTML file keeps its
    /// `\xe9`, in every transfer encoding, instead of the UTF-8
    /// re-encoding (or the U+FFFD) mail-parser's decoded body would give.
    #[test]
    fn text_payloads_keep_their_bytes() {
        let latin1_html =
            b"<html><head><meta charset=\"windows-1252\"></head><body>caf\xe9</body></html>";
        let mut m = String::from(
            "From: A <a@x.com>\r\nSubject: S\r\nMIME-Version: 1.0\r\n\
             Content-Type: multipart/mixed; boundary=\"bb\"\r\n\r\n\
             --bb\r\nContent-Type: text/plain\r\n\r\nBody.\r\n",
        );
        // 8bit: raw bytes on the wire.
        m.push_str(
            "--bb\r\nContent-Type: text/html; charset=windows-1252\r\n\
             Content-Disposition: attachment; filename=\"page.html\"\r\n\
             Content-Transfer-Encoding: 8bit\r\n\r\n",
        );
        let mut bytes = m.into_bytes();
        bytes.extend_from_slice(latin1_html);
        bytes.extend_from_slice(b"\r\n");
        // quoted-printable, no charset parameter at all.
        bytes.extend_from_slice(
            b"--bb\r\nContent-Type: text/plain\r\n\
              Content-Disposition: attachment; filename=\"note.txt\"\r\n\
              Content-Transfer-Encoding: quoted-printable\r\n\r\ncaf=E9 au lait=\r\n done\r\n",
        );
        // base64 of the same Latin-1 text.
        bytes.extend_from_slice(
            b"--bb\r\nContent-Type: text/plain; charset=iso-8859-1\r\n\
              Content-Disposition: attachment; filename=\"b64.txt\"\r\n\
              Content-Transfer-Encoding: base64\r\n\r\n",
        );
        bytes.extend_from_slice(base64(b"caf\xe9").as_bytes());
        bytes.extend_from_slice(b"\r\n--bb--\r\n");
        let atts = open(&bytes);
        assert_eq!(atts.data(0).unwrap(), latin1_html);
        assert_eq!(atts.data(1).unwrap(), b"caf\xe9 au lait done");
        assert_eq!(atts.data(2).unwrap(), b"caf\xe9");
        assert_eq!(atts.entries()[0].size, latin1_html.len() as u64);
    }

    /// #564: `inline` is an image of the body, not any part disposed
    /// `inline`; duplicate names get a counter; a forwarded subject with a
    /// slash is one name; bidi overrides are dropped from names.
    #[test]
    fn inline_images_only_unique_safe_names() {
        let nested = "From: C <c@z.com>\r\nSubject: Re: Q3/Q4 plan\r\n\r\nInner.\r\n";
        let m = eml(&[
            (
                "Content-Type: application/pdf\r\n\
                 Content-Disposition: inline; filename=\"report.pdf\"\r\n",
                b"%PDF-1.4 one",
            ),
            (
                "Content-Type: application/pdf\r\n\
                 Content-Disposition: attachment; filename=\"report.pdf\"\r\n",
                b"%PDF-1.4 two",
            ),
            (
                "Content-Type: application/pdf\r\n\
                 Content-Disposition: attachment; filename=\"report.pdf\"\r\n",
                b"%PDF-1.4 three",
            ),
            (
                "Content-Type: image/png\r\nContent-ID: <logo@x>\r\n\
                 Content-Disposition: inline; filename=\"logo.png\"\r\n",
                b"\x89PNG\r\n\x1a\n",
            ),
            (
                "Content-Type: message/rfc822\r\nContent-Disposition: attachment\r\n",
                nested.as_bytes(),
            ),
            (
                "Content-Type: application/pdf\r\n\
                 Content-Disposition: attachment; filename=\"invoice\u{202E}fdp.exe\"\r\n",
                b"%PDF-1.4 rlo",
            ),
        ]);
        let atts = open(&m);
        let names: Vec<(&str, bool)> = atts
            .entries()
            .iter()
            .map(|e| (e.name.as_str(), e.inline))
            .collect();
        assert_eq!(
            names,
            [
                ("report.pdf", false),
                ("report-2.pdf", false),
                ("report-3.pdf", false),
                ("logo.png", true),
                ("Re: Q3-Q4 plan.eml", false),
                ("invoicefdp.exe", false),
            ]
        );
        assert_eq!(atts.data(1).unwrap(), b"%PDF-1.4 two");
    }

    #[test]
    fn limits_drop_payloads() {
        let m = eml(&[
            (
                "Content-Type: text/markdown\r\nContent-Disposition: attachment; filename=\"a.md\"\r\n",
                b"# A\n\nalpha",
            ),
            (
                "Content-Type: text/markdown\r\nContent-Disposition: attachment; filename=\"b.md\"\r\n",
                b"# B\n\nbeta beta beta beta",
            ),
            (
                "Content-Type: text/markdown\r\nContent-Disposition: attachment; filename=\"c.md\"\r\n",
                b"# C",
            ),
        ]);
        let limits = ArchiveLimits {
            max_entry_size: 12,
            max_total_size: 12,
            max_entries: 3,
            ..ArchiveLimits::default()
        };
        let atts = EmailAttachments::open(&m, &limits).unwrap();
        let skipped: Vec<Option<&str>> = atts
            .entries()
            .iter()
            .map(|e| e.skipped.as_deref())
            .collect();
        assert_eq!(
            skipped,
            [
                None,
                Some("larger than the per-entry size limit"),
                Some("over the total size limit"),
            ]
        );
        assert!(atts.data(0).is_some() && atts.data(1).is_none() && atts.data(2).is_none());
        let two = EmailAttachments::open(
            &m,
            &ArchiveLimits {
                max_entries: 2,
                ..ArchiveLimits::default()
            },
        )
        .unwrap();
        assert_eq!(
            two.entries()[2].skipped.as_deref(),
            Some("over the entry limit")
        );
        assert!(two.data(2).is_none());
    }

    #[test]
    fn convert_email_attachments_reports_every_attachment() {
        let m = eml(&[
            (
                "Content-Type: text/markdown\r\nContent-Disposition: attachment; filename=\"a.md\"\r\n",
                b"# A\n\nalpha",
            ),
            (
                "Content-Type: application/vnd.openxmlformats-officedocument.wordprocessingml.document\r\n\
                 Content-Disposition: attachment; filename=\"broken.docx\"\r\n",
                b"not a zip",
            ),
            (
                "Content-Type: application/x-msdownload\r\n\
                 Content-Disposition: attachment; filename=\"tool.exe\"\r\n",
                b"MZ",
            ),
        ]);
        let conv = DocumentConverter::new();
        let items: Vec<EmailAttachmentItem> = conv.convert_email_attachments(&m).unwrap().collect();
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
        // A message without attachments converts to nothing, not an error.
        assert_eq!(
            conv.convert_email_attachments(b"Subject: bare\r\n\r\nJust a body.\r\n")
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn names_are_reduced_to_a_safe_base_name() {
        assert_eq!(
            safe_name(Some("../../etc/passwd.md"), None, 0, false),
            "passwd.md"
        );
        assert_eq!(
            safe_name(Some("C:\\Users\\x\\q.pdf"), None, 0, false),
            "q.pdf"
        );
        assert_eq!(safe_name(Some("a\r\nb.txt"), None, 0, false), "ab.txt");
        assert_eq!(safe_name(Some(".."), None, 2, false), "attachment-3");
        assert_eq!(
            safe_name(None, Some("application/pdf"), 0, false),
            "attachment-1.pdf"
        );
        assert_eq!(safe_name(None, Some("x/y"), 1, false), "attachment-2");
        assert_eq!(safe_name(None, None, 0, true), "message-1.eml");
    }

    /// Synthesized `.msg` (the fixtures hold by-value attachments only): an
    /// embedded message projects to an `.eml` entry, a reference and an OLE
    /// object are listed without payload, the hidden flag marks inline.
    #[test]
    fn msg_methods_are_handled() {
        let inner = cfb::MsgSpec {
            subject: "Inner subject",
            body: "Inner body.",
            attachments: vec![],
        };
        let outer = cfb::MsgSpec {
            subject: "Outer",
            body: "Outer body.",
            attachments: vec![
                cfb::AttachSpec::ByValue {
                    name: "notes.md",
                    mime: Some("text/markdown"),
                    data: b"# Notes\n\nhello".to_vec(),
                    hidden: false,
                },
                cfb::AttachSpec::ByValue {
                    name: "image001.png",
                    mime: Some("image/png"),
                    data: b"\x89PNG\r\n\x1a\n".to_vec(),
                    hidden: true,
                },
                cfb::AttachSpec::Embedded {
                    display_name: "Fwd: Inner subject",
                    message: Box::new(inner),
                },
                cfb::AttachSpec::Reference {
                    name: "shared.docx",
                },
                cfb::AttachSpec::Ole { name: "object.bin" },
            ],
        };
        let bytes = cfb::write_msg(&outer);
        assert!(crate::backend::cfb::CompoundFile::detect(&bytes));
        let atts = open(&bytes);
        let got = rows(&atts);
        assert_eq!(
            got,
            [
                (
                    "notes.md",
                    Some("text/markdown"),
                    Some(InputFormat::Md),
                    false,
                    None
                ),
                (
                    "image001.png",
                    Some("image/png"),
                    Some(InputFormat::Image),
                    true,
                    None
                ),
                (
                    "Fwd: Inner subject.eml",
                    Some("message/rfc822"),
                    Some(InputFormat::Email),
                    false,
                    None
                ),
                (
                    "shared.docx",
                    None,
                    None,
                    false,
                    Some("attachment by reference (no payload)")
                ),
                (
                    "object.bin",
                    None,
                    None,
                    false,
                    Some("OLE object (no payload)")
                ),
            ]
        );
        assert_eq!(atts.data(0).unwrap(), b"# Notes\n\nhello");
        // The embedded message's projection converts as an email of its own.
        let inner_src = atts.read(2).unwrap();
        let md = DocumentConverter::new()
            .convert(inner_src)
            .unwrap()
            .document
            .export_to_markdown();
        assert!(md.starts_with("# Inner subject\n"), "{md}");
        assert!(md.contains("Inner body."), "{md}");
        // The outer message itself still converts as before, labels included.
        let outer_md = DocumentConverter::new()
            .list_attachments(true)
            .convert(SourceDocument::from_bytes("m", InputFormat::Email, bytes))
            .unwrap()
            .document
            .export_to_markdown();
        assert!(
            outer_md.contains("- notes.md (text/markdown)"),
            "{outer_md}"
        );
        assert!(outer_md.contains("- Fwd: Inner subject"), "{outer_md}");
    }

    /// A minimal CFB (OLE2) writer for the `.msg` tests: version 3 (512-byte
    /// sectors), every stream in the mini stream (all are under 4 KiB), the
    /// directory as a right-linked sibling list. Only what the reader in
    /// `backend::cfb` needs — not a general-purpose writer.
    mod cfb {
        pub struct MsgSpec {
            pub subject: &'static str,
            pub body: &'static str,
            pub attachments: Vec<AttachSpec>,
        }

        pub enum AttachSpec {
            ByValue {
                name: &'static str,
                mime: Option<&'static str>,
                data: Vec<u8>,
                hidden: bool,
            },
            Embedded {
                display_name: &'static str,
                message: Box<MsgSpec>,
            },
            Reference {
                name: &'static str,
            },
            Ole {
                name: &'static str,
            },
        }

        /// A directory node under construction: a storage with children or a
        /// stream with bytes.
        enum Node {
            Storage(Vec<(String, Node)>),
            Stream(Vec<u8>),
        }

        fn utf16(s: &str) -> Vec<u8> {
            let mut out: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
            out.extend_from_slice(&[0, 0]);
            out
        }

        /// A `__properties_version1.0` stream: `header` zero bytes, then
        /// 16-byte records of (type, id, flags, value).
        fn properties(header: usize, fixed: &[(u16, u16, u64)]) -> Vec<u8> {
            let mut out = vec![0u8; header];
            for &(ty, id, value) in fixed {
                out.extend_from_slice(&ty.to_le_bytes());
                out.extend_from_slice(&id.to_le_bytes());
                out.extend_from_slice(&6u32.to_le_bytes()); // readable | writable
                out.extend_from_slice(&value.to_le_bytes());
            }
            out
        }

        fn string_stream(id: &str, value: &str) -> (String, Node) {
            (format!("__substg1.0_{id}001F"), Node::Stream(utf16(value)))
        }

        fn message_children(spec: &MsgSpec, header: usize) -> Vec<(String, Node)> {
            let mut kids = vec![
                string_stream("0037", spec.subject),
                string_stream("1000", spec.body),
                string_stream("0C1A", "Sender"),
                string_stream("0C1F", "sender@example.com"),
                (
                    "__properties_version1.0".into(),
                    Node::Stream(properties(header, &[])),
                ),
            ];
            for (i, att) in spec.attachments.iter().enumerate() {
                let mut a: Vec<(String, Node)> = Vec::new();
                let mut fixed: Vec<(u16, u16, u64)> = Vec::new();
                match att {
                    AttachSpec::ByValue {
                        name,
                        mime,
                        data,
                        hidden,
                    } => {
                        a.push(string_stream("3707", name));
                        a.push(string_stream("3704", name));
                        if let Some(m) = mime {
                            a.push(string_stream("370E", m));
                        }
                        a.push(("__substg1.0_37010102".into(), Node::Stream(data.clone())));
                        fixed.push((0x0003, 0x3705, 1));
                        if *hidden {
                            fixed.push((0x000B, 0x7FFE, 1));
                        }
                    }
                    AttachSpec::Embedded {
                        display_name,
                        message,
                    } => {
                        a.push(string_stream("3001", display_name));
                        a.push((
                            "__substg1.0_3701000D".into(),
                            Node::Storage(message_children(message, 24)),
                        ));
                        fixed.push((0x0003, 0x3705, 5));
                    }
                    AttachSpec::Reference { name } => {
                        a.push(string_stream("3707", name));
                        a.push(string_stream("3701", "\\\\server\\share\\shared.docx"));
                        fixed.push((0x0003, 0x3705, 2));
                    }
                    AttachSpec::Ole { name } => {
                        a.push(string_stream("3707", name));
                        a.push(("__substg1.0_37010102".into(), Node::Stream(vec![1, 2, 3])));
                        fixed.push((0x0003, 0x3705, 6));
                    }
                }
                a.push((
                    "__properties_version1.0".into(),
                    Node::Stream(properties(8, &fixed)),
                ));
                kids.push((format!("__attach_version1.0_#{i:08X}"), Node::Storage(a)));
            }
            kids
        }

        struct Dir {
            name: String,
            object_type: u8,
            left: u32,
            right: u32,
            child: u32,
            start: u32,
            size: u32,
        }

        /// Flatten the tree into directory entries (depth-first, children as
        /// a right-linked list) and the streams into the mini stream.
        fn flatten(
            name: &str,
            node: &Node,
            object_type: u8,
            dir: &mut Vec<Dir>,
            mini: &mut Vec<u8>,
            mini_fat: &mut Vec<u32>,
        ) -> u32 {
            let id = dir.len() as u32;
            dir.push(Dir {
                name: name.to_string(),
                object_type,
                left: 0xFFFF_FFFF,
                right: 0xFFFF_FFFF,
                child: 0xFFFF_FFFF,
                start: 0xFFFF_FFFE,
                size: 0,
            });
            match node {
                Node::Stream(bytes) => {
                    let first = (mini.len() / 64) as u32;
                    let sectors = bytes.len().div_ceil(64).max(1);
                    for s in 0..sectors {
                        mini_fat.push(if s + 1 == sectors {
                            0xFFFF_FFFE
                        } else {
                            first + s as u32 + 1
                        });
                    }
                    mini.extend_from_slice(bytes);
                    mini.resize(mini.len().div_ceil(64) * 64, 0);
                    dir[id as usize].start = first;
                    dir[id as usize].size = bytes.len() as u32;
                }
                Node::Storage(kids) => {
                    let mut prev: Option<u32> = None;
                    for (kname, kid) in kids {
                        let kid_id = flatten(
                            kname,
                            kid,
                            1 + u8::from(matches!(kid, Node::Stream(_))),
                            dir,
                            mini,
                            mini_fat,
                        );
                        match prev {
                            None => dir[id as usize].child = kid_id,
                            Some(p) => dir[p as usize].right = kid_id,
                        }
                        prev = Some(kid_id);
                    }
                }
            }
            id
        }

        pub fn write_msg(spec: &MsgSpec) -> Vec<u8> {
            let root = Node::Storage(message_children(spec, 32));
            let mut dir = Vec::new();
            let mut mini = Vec::new();
            let mut mini_fat = Vec::new();
            flatten("Root Entry", &root, 5, &mut dir, &mut mini, &mut mini_fat);
            // Sector plan: 0 = FAT, then the directory, the mini FAT, the mini
            // stream (the root entry's chain).
            let sectors = |bytes: usize| bytes.div_ceil(512).max(1);
            let mut mini_fat_bytes: Vec<u8> =
                mini_fat.iter().flat_map(|v| v.to_le_bytes()).collect();
            mini_fat_bytes.resize(sectors(mini_fat_bytes.len()) * 512, 0xFF);
            mini.resize(sectors(mini.len()) * 512, 0);
            let (dir_n, mfat_n, mini_n) = (
                sectors(dir.len() * 128),
                sectors(mini_fat_bytes.len()),
                sectors(mini.len()),
            );
            let dir_start = 1u32;
            let mfat_start = dir_start + dir_n as u32;
            let mini_start = mfat_start + mfat_n as u32;
            assert!(
                1 + dir_n + mfat_n + mini_n <= 128,
                "test file too large for one FAT sector"
            );
            // The root entry's stream is the mini stream.
            dir[0].start = mini_start;
            dir[0].size = mini.len() as u32;
            let mut dir_bytes = Vec::with_capacity(dir_n * 512);
            for d in &dir {
                let mut e = vec![0u8; 128];
                let name = utf16(&d.name);
                e[..name.len()].copy_from_slice(&name);
                e[64..66].copy_from_slice(&(name.len() as u16).to_le_bytes());
                e[66] = d.object_type;
                e[67] = 1; // black
                e[68..72].copy_from_slice(&d.left.to_le_bytes());
                e[72..76].copy_from_slice(&d.right.to_le_bytes());
                e[76..80].copy_from_slice(&d.child.to_le_bytes());
                e[116..120].copy_from_slice(&d.start.to_le_bytes());
                e[120..124].copy_from_slice(&d.size.to_le_bytes());
                dir_bytes.extend_from_slice(&e);
            }
            dir_bytes.resize(dir_n * 512, 0);
            let mut fat = vec![0xFFFF_FFFFu32; 128];
            fat[0] = 0xFFFF_FFFD; // FATSECT
            let chain = |fat: &mut Vec<u32>, start: u32, n: usize| {
                for i in 0..n {
                    fat[start as usize + i] = if i + 1 == n {
                        0xFFFF_FFFE
                    } else {
                        start + i as u32 + 1
                    };
                }
            };
            chain(&mut fat, dir_start, dir_n);
            chain(&mut fat, mfat_start, mfat_n);
            chain(&mut fat, mini_start, mini_n);
            let mut header = vec![0u8; 512];
            header[..8].copy_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
            header[24..26].copy_from_slice(&0x003Eu16.to_le_bytes()); // minor
            header[26..28].copy_from_slice(&3u16.to_le_bytes()); // major 3
            header[28..30].copy_from_slice(&0xFFFEu16.to_le_bytes()); // little-endian
            header[30..32].copy_from_slice(&9u16.to_le_bytes()); // 512-byte sectors
            header[32..34].copy_from_slice(&6u16.to_le_bytes()); // 64-byte mini sectors
            header[44..48].copy_from_slice(&1u32.to_le_bytes()); // FAT sectors
            header[48..52].copy_from_slice(&dir_start.to_le_bytes());
            header[56..60].copy_from_slice(&4096u32.to_le_bytes()); // mini cutoff
            header[60..64].copy_from_slice(&mfat_start.to_le_bytes());
            header[64..68].copy_from_slice(&(mfat_n as u32).to_le_bytes());
            header[68..72].copy_from_slice(&0xFFFF_FFFEu32.to_le_bytes()); // no DIFAT chain
            header[72..76].copy_from_slice(&0u32.to_le_bytes());
            for i in 0..109 {
                let v: u32 = if i == 0 { 0 } else { 0xFFFF_FFFF };
                header[76 + i * 4..80 + i * 4].copy_from_slice(&v.to_le_bytes());
            }
            let mut out = header;
            out.extend(fat.iter().flat_map(|v| v.to_le_bytes()));
            out.extend_from_slice(&dir_bytes);
            out.extend_from_slice(&mini_fat_bytes);
            out.extend_from_slice(&mini);
            out
        }
    }
}
