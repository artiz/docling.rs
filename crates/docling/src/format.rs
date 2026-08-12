//! Input format enumeration and detection.
//!
//! Mirrors `docling.datamodel.base_models.InputFormat` and its
//! `FormatToExtensions` map.

/// A document format supported by docling.rs backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputFormat {
    Docx,
    Pptx,
    Html,
    Image,
    Pdf,
    Asciidoc,
    Md,
    Csv,
    Xlsx,
    /// Word 97–2004 binary (`.doc`) — parsed natively (CFB + MS-DOC), no
    /// external converter (docling shells out to LibreOffice for these).
    Doc,
    /// Excel 97–2004 binary (`.xls`, BIFF8) — parsed natively via calamine.
    Xls,
    /// PowerPoint 97–2003 binary (`.ppt`) — parsed natively (CFB + MS-PPT).
    Ppt,
    Odt,
    Ods,
    Odp,
    XmlUspto,
    XmlJats,
    XmlXbrl,
    XmlDoclang,
    /// Raw DocTags markup (`.doctags`/`.dt`) — the token stream docling's
    /// VLMs emit, parsed by `docling_core::doctags` (#152).
    DocTags,
    /// A DocLang OPC archive (`.dclx`, the format `--to dclx` writes).
    Dclx,
    MetsGbs,
    JsonDocling,
    Audio,
    /// Video containers (`.mp4`/`.avi`/`.mov`/`.mkv`/`.webm`) — Phase 1 of
    /// issue #138 transcribes the audio track through the ASR pipeline
    /// (mirrors docling's `InputFormat.VIDEO`, v2.114).
    Video,
    Vtt,
    Latex,
    Email,
    Epub,
    /// MIME HTML archive (`.mhtml`/`.mht`) — a docling.rs extension; docling
    /// has no MHTML backend.
    Mhtml,
    /// Rich Text Format (`.rtf`) — a docling.rs extension (#209); docling
    /// converts RTF only by shelling out to LibreOffice.
    Rtf,
    /// Microsoft Visio (`.vsdx`, `.vsdm`) — a docling.rs extension (#214);
    /// docling has no Visio reader. Pages become sections, shape text flows
    /// in reading order, connectors become a relations table.
    Visio,
    /// SVG (`.svg`) — a docling.rs extension (#212); docling does not accept
    /// SVG input. Mirrors the pdf / pdf-text split: the ML build rasterizes
    /// (resvg) and rides the image pipeline; without ML — or under `--no-ocr`
    /// — `<text>` elements are extracted directly into flat paragraphs.
    Svg,
    /// Apple Pages (`.pages`) — a docling.rs extension (#213); docling has
    /// no iWork reader. Modern (2013+) IWA packages, text-level extraction.
    Pages,
    /// Apple Numbers (`.numbers`), same IWA machinery as [`Self::Pages`].
    Numbers,
    /// Apple Keynote (`.key`), same IWA machinery as [`Self::Pages`].
    Keynote,
    /// StarOffice 5 binaries (`.sdw`/`.sda`/`.sdd`/`.vor`) — a docling.rs
    /// extension (#215); CFB containers parsed natively (text-level
    /// extraction). The document kind comes from the stream inside, so a
    /// `.vor` template of any application dispatches by content. StarCalc
    /// (`.sdc`) is a follow-up.
    StarOffice5,
}

impl InputFormat {
    /// Stable string identifier, matching the Python enum values.
    pub fn as_str(self) -> &'static str {
        match self {
            InputFormat::Docx => "docx",
            InputFormat::Pptx => "pptx",
            InputFormat::Html => "html",
            InputFormat::Image => "image",
            InputFormat::Pdf => "pdf",
            InputFormat::Asciidoc => "asciidoc",
            InputFormat::Md => "md",
            InputFormat::Csv => "csv",
            InputFormat::Xlsx => "xlsx",
            InputFormat::Doc => "doc",
            InputFormat::Xls => "xls",
            InputFormat::Ppt => "ppt",
            InputFormat::Odt => "odt",
            InputFormat::Ods => "ods",
            InputFormat::Odp => "odp",
            InputFormat::XmlUspto => "xml_uspto",
            InputFormat::XmlJats => "xml_jats",
            InputFormat::XmlXbrl => "xml_xbrl",
            InputFormat::XmlDoclang => "xml_doclang",
            InputFormat::DocTags => "doctags",
            InputFormat::Dclx => "dclx",
            InputFormat::MetsGbs => "mets_gbs",
            InputFormat::JsonDocling => "json_docling",
            InputFormat::Audio => "audio",
            InputFormat::Video => "video",
            InputFormat::Vtt => "vtt",
            InputFormat::Latex => "latex",
            InputFormat::Email => "email",
            InputFormat::Epub => "epub",
            InputFormat::Mhtml => "mhtml",
            InputFormat::Rtf => "rtf",
            InputFormat::Visio => "visio",
            InputFormat::Svg => "svg",
            InputFormat::Pages => "pages",
            InputFormat::Numbers => "numbers",
            InputFormat::Keynote => "key",
            InputFormat::StarOffice5 => "staroffice5",
        }
    }

    /// Best-effort format detection from a file extension (case-insensitive).
    ///
    /// Ambiguous extensions (notably bare `xml`) resolve to a single default
    /// here; the converter's content sniffing does the real disambiguation.
    pub fn from_extension(ext: &str) -> Option<Self> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "docx" | "dotx" | "docm" | "dotm" => InputFormat::Docx,
            "pptx" | "potx" | "ppsx" | "pptm" | "potm" | "ppsm" => InputFormat::Pptx,
            "pdf" => InputFormat::Pdf,
            "md" | "txt" | "text" | "qmd" | "rmd" => InputFormat::Md,
            "html" | "htm" | "xhtml" => InputFormat::Html,
            "xml" | "nxml" => InputFormat::XmlJats,
            "dclg" => InputFormat::XmlDoclang,
            "doctags" | "dt" => InputFormat::DocTags,
            "dclx" => InputFormat::Dclx,
            // `.gif` decodes through the same content-sniffing `image` path as
            // the rest (first frame of an animation), issue #208.
            "jpg" | "jpeg" | "png" | "tif" | "tiff" | "bmp" | "webp" | "gif" | "heic" | "heif" => {
                InputFormat::Image
            }
            "adoc" | "asciidoc" | "asc" => InputFormat::Asciidoc,
            // `.tsv` rides the CSV backend, whose delimiter sniffing already
            // prefers the tab when it dominates the first line (#208).
            "csv" | "tsv" => InputFormat::Csv,
            // `.xlsb` (binary Excel 2007+) parses through the same calamine
            // engine as xlsx — the backend detects the binary workbook part
            // and switches readers, issue #210.
            "xlsx" | "xlsm" | "xlsb" => InputFormat::Xlsx,
            // Legacy binary Office (Word/Excel/PowerPoint 97–2003), issue #127.
            // Extension sets mirror docling's FormatToExtensions.
            "doc" | "dot" => InputFormat::Doc,
            "xls" | "xlt" => InputFormat::Xls,
            "ppt" | "pot" | "pps" => InputFormat::Ppt,
            // StarOffice / OpenOffice 1.x XML and flat ODF (#215, docling.rs
            // extensions): the shared ODF backend parses the older namespace
            // vocabulary through a local-name mapping layer, and the flat
            // variants are the same XML uncompressed in a single file.
            // Templates (`.stw`/`.sti`/`.stc`) and the Writer master document
            // (`.sxg`) ride the same parsers as their document counterparts.
            "odt" | "ott" | "sxw" | "stw" | "sxg" | "fodt" => InputFormat::Odt,
            "ods" | "ots" | "sxc" | "stc" | "fods" => InputFormat::Ods,
            "odp" | "otp" | "sxi" | "sti" | "fodp" => InputFormat::Odp,
            "json" => InputFormat::JsonDocling,
            // `.mpga` *is* MPEG audio (an mp3 stream) — symphonia probes the
            // codec from the bytes, the extension is just the alias (#208).
            "wav" | "mp3" | "mpga" | "m4a" | "aac" | "ogg" | "flac" => InputFormat::Audio,
            // Upstream's FormatToExtensions[VIDEO] (docling v2.114, #3768):
            // the audio track transcribes through the same ASR path.
            // `.mpeg`/`.mpg` (#208): MPEG-PS has no symphonia demuxer, so both
            // the audio track and the sampled frames come from the ffmpeg
            // fallback; an audio-only `.mpeg` still decodes in-process (the
            // probe is content-based) and converts to its transcript.
            "mp4" | "avi" | "mov" | "mkv" | "webm" | "mpeg" | "mpg" => InputFormat::Video,
            "vtt" => InputFormat::Vtt,
            "tex" | "latex" => InputFormat::Latex,
            "eml" => InputFormat::Email,
            "epub" => InputFormat::Epub,
            "mhtml" | "mht" => InputFormat::Mhtml,
            "rtf" => InputFormat::Rtf,
            "vsdx" | "vsdm" => InputFormat::Visio,
            "svg" => InputFormat::Svg,
            // Apple iWork (#213). `.heic`/`.heif` route to Image above and
            // decode behind the opt-in `heif` cargo feature.
            "pages" => InputFormat::Pages,
            "numbers" => InputFormat::Numbers,
            "key" => InputFormat::Keynote,
            // StarOffice 5 binaries (#215): .vor templates dispatch by the
            // CFB stream inside (writer/draw/impress share the container).
            // .sdc routes here too so StarCalc gets its targeted
            // "save as .ods" error instead of an unknown-extension one.
            "sdw" | "sda" | "sdd" | "sdc" | "vor" => InputFormat::StarOffice5,
            // METS/Google Books scan packages ship as `*.tar.gz`.
            "gz" | "targz" => InputFormat::MetsGbs,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_and_video_extensions_split_like_upstream() {
        // docling v2.114 FormatToExtensions: AUDIO and VIDEO are disjoint
        // (docling.rs adds the MPEG aliases on top, #208).
        for ext in ["wav", "mp3", "mpga", "m4a", "aac", "ogg", "flac"] {
            assert_eq!(InputFormat::from_extension(ext), Some(InputFormat::Audio));
        }
        for ext in ["mp4", "avi", "mov", "mkv", "webm", "MKV", "mpeg", "mpg"] {
            assert_eq!(InputFormat::from_extension(ext), Some(InputFormat::Video));
        }
        assert_eq!(InputFormat::Video.as_str(), "video");
    }

    #[test]
    fn extension_aliases_route_to_existing_backends() {
        // #208/#210: aliases whose decoding machinery predated the mapping.
        assert_eq!(InputFormat::from_extension("tsv"), Some(InputFormat::Csv));
        assert_eq!(InputFormat::from_extension("gif"), Some(InputFormat::Image));
        assert_eq!(InputFormat::from_extension("xlsb"), Some(InputFormat::Xlsx));
    }
}
