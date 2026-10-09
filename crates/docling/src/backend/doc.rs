//! DOC (Word 97–2004 binary, [MS-DOC]) backend — issue #127.
//!
//! Native parsing, no external converter (docling proper shells out to
//! LibreOffice for these files — `docling` PR #3804). The format is a CFB
//! container ([`cfb`]) holding a `WordDocument` stream (FIB header + raw text)
//! and a `0Table`/`1Table` stream (the piece table and formatting):
//!
//! 1. The **FIB** locates the CLX in the table stream and says how long the
//!    main document text is (`ccpText`).
//! 2. The **piece table** (CLX → PlcPcd) maps character positions (CPs) to
//!    file offsets (FCs) — pieces are either 8-bit CP1252 or UTF-16LE.
//! 3. Text is split into paragraphs at the paragraph mark (`\r`) / cell mark
//!    (`0x07`); each paragraph's properties come from its **PAPX** (found via
//!    the PlcfBtePapx → FKP page for the mark's FC): the style index `istd`,
//!    `fInTable` (table cell content) and `fTtp` (table row terminator).
//! 4. The **stylesheet** (STSH) maps `istd` to the built-in style identifier
//!    `sti` — 1–9 are the Heading 1–9 styles, giving real headings.
//!
//! Scope: headings, paragraphs, list items (by list-format reference),
//! tables (cell/row marks), and embedded pictures — both inline (`0x01`
//! anchor → `sprmCPicLocation` → PICF in the Data stream) and floating
//! (`0x08` anchor → PlcfSpa → the drawing's shape → BLIP store, with
//! delay-stream data in `WordDocument`), decoded via [`officeart`].
//!
//! The other **stories** (#535) follow the main text in the same CP space —
//! footnotes (`ccpFtn`), headers/footers (`ccpHdd`), comments, endnotes,
//! text boxes (`ccpTxbx`) — and are read too: a text box's story
//! (`PlcftxbxTxt`, linked to its shape by `lid`) is placed at the shape's
//! anchor (a `textbox` section group, or extra paragraphs of the table cell
//! it is anchored in, as the DOCX backend reads the same document); each
//! section's header/footer stories (`PlcfHdd`, after the six separator
//! stories) become `page_header` / `page_footer` furniture; footnote and
//! endnote bodies (`PlcffndTxt` / `PlcfendTxt`) `footnote` furniture after
//! the body; a header's or footer's text boxes (the `ccpHdrTxbx` story,
//! `PlcftxbxHdrTxt` + `PlcSpaHdr`, #574) join that part's furniture ahead
//! of its own paragraphs. Comments are not read.
//!
//! The FIB is read through [`Fib`], which locates `fibRgLw` and
//! `fibRgFcLcb` from the counts the stream itself declares (`csw`, `cslw`,
//! `cbRgFcLcb`) instead of fixed offsets, so a short or truncated FIB is an
//! error naming what is missing and an `fc`/`lcb` pair beyond the writer's
//! `cbRgFcLcb` reads as "structure absent" (#512).
//!
//! **Word 6.0 / Word 95** (`nFib` 101–105, the pre-97 FIB — no table stream,
//! 8-bit text) goes through the same story walker (#640, [`DocFormat`]):
//! the FIB's `fcMin`/`fcMac` bound the text in `WordDocument` (its CLX for
//! a fast-saved file), the PAPX / CHPX bin tables (2-byte page numbers,
//! 7-byte `BX`es, single-byte sprms with their own numbering) give the
//! tables, headings (the stylesheet's built-in `sti`s; its names are 8-bit
//! Pascal strings) and bold / italic — a run's 0x80 / 0x81 operands are
//! relative to the style, resolved through the based-on chain — and the
//! section table's `grpfIhdt` says which header / footer stories `PlcfHdd`
//! holds; footnotes as in Word 97. Not read there: pictures (a Word 6 PICF
//! holds a WMF / DIB; the anchor is a placeholder picture), text boxes,
//! autonumber lists (`sprmPAnld`), endnotes. A bin table the FIB does not
//! place leaves default properties — the body-paragraph output this reader
//! gave before. Text decodes as Windows-1252, the code page of the Western
//! editions, or as UTF-16LE when the FIB's `fExtChar` is set (the Far East
//! editions).
//!
//! **Word for Windows 1.x / 2.0** (`wIdent` 0xA59B / 0xA59C / 0xA5DB,
//! `nFib` ≤ 63, #566) is not a compound file at all: the FIB sits at byte 0
//! of a flat file, the text at `fcMin`, and the formatting in 512-byte FKP
//! pages reached through bin tables the FIB locates with 6-byte
//! (`fc`, `cb`) pairs. [`convert_word2`] reads the main story as
//! paragraphs, the tables (cell marks `\r\x07`, the row-end paragraph's
//! PAPX), bold / italic from the CHPX, and the headings from the style code
//! each PAPX opens with, resolved through the Word 2.0 stylesheet
//! ([`word2_styles`], #573) — enough to match what Word's own re-save of
//! such a file converts to; pictures and fast-saved (`fComplex`) files are
//! not read.

use docling_core::{DoclingDocument, Node, PictureImage, Table};

use crate::backend::cfb::CompoundFile;
use crate::backend::markdown::escape_text;
use crate::backend::officeart;
use crate::backend::symbol_fonts::{self, SymbolFont};
use crate::backend::DeclarativeBackend;
use crate::error::ConversionError;
use crate::source::SourceDocument;

pub struct DocBackend;

impl DeclarativeBackend for DocBackend {
    fn convert(&self, source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
        // A Word for Windows 1.x / 2.0 file is flat — no container to open.
        if is_word2(&source.bytes) {
            return convert_word2(&source.name, &source.bytes);
        }
        let cfb = CompoundFile::open(&source.bytes).ok_or_else(|| {
            ConversionError::Parse(CompoundFile::open_error("doc", &source.bytes))
        })?;
        let word = cfb
            .stream("WordDocument")
            .ok_or_else(|| ConversionError::Parse(cfb.stream_error("doc", "WordDocument")))?;
        let w_ident = u16_at(&word, 0).ok_or_else(|| {
            ConversionError::Parse(format!(
                "doc: WordDocument stream too short for a FIB ({} bytes)",
                word.len()
            ))
        })?;
        let n_fib = u16_at(&word, 2).unwrap_or(0);
        let flags = u16_at(&word, 0x0A).unwrap_or(0);
        if flags & 0x0100 != 0 {
            return Err(crate::backend::offcrypto::encrypted("doc"));
        }
        if w_ident != 0xA5EC {
            if (101..=105).contains(&n_fib) {
                return convert_word6(&source.name, &word, flags);
            }
            return Err(ConversionError::Parse(format!(
                "doc: unsupported Word binary format (wIdent {w_ident:#06X}, nFib {n_fib}); \
                 Word 6.0/95 and Word 97 or later are read"
            )));
        }
        let fib = Fib::parse(&word)?;
        let table_name = if flags & 0x0200 != 0 {
            "1Table"
        } else {
            "0Table"
        };
        let table = cfb
            .stream(table_name)
            .ok_or_else(|| ConversionError::Parse(format!("doc: no {table_name} stream")))?;

        let ccp_text = fib.ccp_text as u64;
        let (fc_clx, lcb_clx) = fib.fc_lcb(Fib::CLX);
        // Two distinct failures, reported apart (#512): a CLX the FIB places
        // outside the table stream means the FIB and the table stream do not
        // belong together (or one is truncated); one inside it that does not
        // parse is a damaged piece table.
        let clx = fc_clx
            .checked_add(lcb_clx)
            .and_then(|end| table.get(fc_clx..end))
            .ok_or_else(|| {
                ConversionError::Parse(format!(
                    "doc: piece table (fcClx {fc_clx}, lcbClx {lcb_clx}) lies outside the \
                     {table_name} stream ({} bytes)",
                    table.len()
                ))
            })?;
        let pieces = parse_piece_table(clx, None).ok_or_else(|| {
            ConversionError::Parse(format!(
                "doc: bad piece table (fcClx {fc_clx}, lcbClx {lcb_clx} in {table_name})"
            ))
        })?;

        // Styles (istd → sti) and paragraph properties (FC → PAPX).
        let stis = parse_stsh(fib.part(&table, Fib::STSHF), DocFormat::Word97);
        let bte = fib.part(&table, Fib::PLCF_BTE_PAPX);
        // Character-run properties (bold/italic): PlcfBteChpx → CHPX FKPs.
        let btec = fib.part(&table, Fib::PLCF_BTE_CHPX);
        let mut chpx_cache = ChpxCache::default();

        // List tables: ilfo → numbering kind/start per level (ordered lists).
        let (fc_lst, lcb_lst) = fib.fc_lcb(Fib::PLF_LST);
        // `lcbPlfLst` covers only the LSTF array; the per-list LVL structures
        // follow it directly in the table stream, so hand `parse` the tail.
        let lists = ListTables::parse(
            table.get(fc_lst..).unwrap_or(&[]),
            lcb_lst,
            fib.part(&table, Fib::PLF_LFO),
        );

        // Pictures: the Data stream holds inline PICFs; the drawing tables
        // (fcDggInfo, in the table stream) + PlcfSpa anchor floating shapes.
        let data = cfb.stream("Data").unwrap_or_default();
        let spa = parse_plcf_spa(fib.part(&table, Fib::PLCSPA_MOM));
        let drawings = Drawings::parse(fib.part(&table, Fib::DGG_INFO), &word);
        let fonts = parse_sttbf_ffn(fib.part(&table, Fib::STTBF_FFN));

        // The stories follow each other in one CP space ([MS-DOC] 2.3.1):
        // main text, footnotes, headers, comments, endnotes, text boxes.
        let ftn_base = ccp_text;
        let hdd_base = ftn_base + fib.ccp_ftn as u64;
        let edn_base = hdd_base + fib.ccp_hdd as u64 + fib.ccp_atn as u64;
        let txbx_base = edn_base + fib.ccp_edn as u64;
        let story = Story {
            format: DocFormat::Word97,
            word: &word,
            pieces: &pieces,
            bte,
            btec,
            stis: &stis,
            data: &data,
            spa: &spa,
            drawings: &drawings,
            textboxes: parse_textboxes(
                fib.part(&table, Fib::PLCF_TXBX_TXT),
                txbx_base,
                fib.ccp_txbx as u64,
            ),
            placed: Default::default(),
            fonts: &fonts,
            lists: &lists,
        };

        // Walk the main-document text paragraph by paragraph, assembling nodes.
        let mut doc = DoclingDocument::new(&source.name);
        story.walk(&mut chpx_cache, 0, ccp_text, true, &mut doc);
        // A text box no anchor in the main text placed (its shape lives in a
        // header, or the drawing tables disagree) still reaches the document:
        // at the end of the body, in story order.
        let mut unplaced: Vec<(u64, u64)> = story
            .textboxes
            .iter()
            .filter(|(spid, _)| !story.placed.borrow().contains(spid))
            .map(|(_, &range)| range)
            .collect();
        unplaced.sort_unstable();
        for (a, b) in unplaced {
            let mut inner = DoclingDocument::new("");
            story.walk(&mut chpx_cache, a, b, false, &mut inner);
            if !inner.nodes.is_empty() {
                doc.push(Node::Group {
                    label: "section".into(),
                    name: Some("textbox".into()),
                    layer: None,
                    children: inner.nodes,
                });
            }
        }

        // The other stories (#535): headers/footers, then the note bodies —
        // furniture, after the body, where the DOCX backend puts them. A
        // header's text boxes (#574) are a story of their own after the body
        // ones, each anchored by PlcSpaHdr at a CP of the header story.
        let header_boxes = HeaderBoxes {
            anchors: parse_plcf_spa(fib.part(&table, Fib::PLCSPA_HDR)),
            stories: parse_textboxes(
                fib.part(&table, Fib::PLCF_HDR_TXBX_TXT),
                txbx_base + fib.ccp_txbx as u64,
                fib.ccp_hdr_txbx as u64,
            ),
        };
        for (footer, text) in header_footer_texts(
            &story,
            &mut chpx_cache,
            fib.part(&table, Fib::PLCF_HDD),
            hdd_base,
            fib.ccp_hdd as u64,
            &header_boxes,
        ) {
            doc.push(Node::FurnitureText {
                label: if footer { "page_footer" } else { "page_header" }.into(),
                text: escape_text(&text),
            });
        }
        for (refs, txt, base, len) in [
            (Fib::PLCF_FND_REF, Fib::PLCF_FND_TXT, ftn_base, fib.ccp_ftn),
            (Fib::PLCF_END_REF, Fib::PLCF_END_TXT, edn_base, fib.ccp_edn),
        ] {
            for text in note_texts(
                &story,
                &mut chpx_cache,
                fib.part(&table, refs),
                fib.part(&table, txt),
                base,
                len as u64,
            ) {
                doc.push(Node::FurnitureText {
                    label: "footnote".into(),
                    text: escape_text(&text),
                });
            }
        }
        Ok(doc)
    }
}

/// One Word 97 document's text, for walking any of its stories: the piece
/// table over the whole CP space and the lookups a paragraph needs.
struct Story<'a> {
    /// Which binary layout the FKPs and sprms use (#640).
    format: DocFormat,
    word: &'a [u8],
    pieces: &'a [Piece],
    bte: &'a [u8],
    btec: &'a [u8],
    stis: &'a [StyleDef],
    data: &'a [u8],
    spa: &'a [(u64, u32)],
    drawings: &'a Drawings,
    /// Shape id → its text box story, as absolute CPs (`[start, end)`).
    textboxes: std::collections::HashMap<u32, (u64, u64)>,
    /// The text boxes placed at an anchor so far (shape ids).
    placed: std::cell::RefCell<std::collections::HashSet<u32>>,
    /// The font table (`SttbfFfn`): name per `ftc` (#588).
    fonts: &'a [String],
    lists: &'a ListTables,
}

impl Story<'_> {
    /// The character at absolute `cp` and its FC, via the piece table.
    fn char_at(&self, cp: u64) -> Option<(char, u64)> {
        let k = self.pieces.partition_point(|p| p.cp_end <= cp);
        let piece = self.pieces.get(k).filter(|p| p.cp_start <= cp)?;
        let i = cp - piece.cp_start;
        Some((piece_char(self.word, piece, i), piece_fc(piece, i)))
    }

    /// A text character into the paragraph, as Word shows it (#588): an
    /// inserted symbol (`sprmCSymbol` on its `0x28` placeholder — the `(`
    /// the text used to print) and a character set in a symbol
    /// font (`Symbol`, `Wingdings`, … through the run's `sprmCRgFtc0`/`2`)
    /// become the glyph's Unicode character, like the DOCX backend's `w:sym`
    /// and symbol-font runs. A font the table does not name, or a code with
    /// no glyph, leaves the character as it was.
    fn push_char(&self, para: &mut ParaAccum, ch: char, fmt: CharFmt) {
        let font = |ftc: u16| {
            self.fonts
                .get(ftc as usize)
                .and_then(|name| SymbolFont::from_name(name))
        };
        let ch = match fmt.symbol {
            Some((ftc, code)) if ch == '(' => font(ftc)
                .and_then(|f| f.glyph(code as u32))
                .or_else(|| char::from_u32(code as u32).filter(|c| !c.is_control()))
                .unwrap_or(ch),
            _ if symbol_fonts::is_glyph_code(ch) => {
                let ftc = if (ch as u32) < 0x80 {
                    fmt.ftc_ascii.or(fmt.ftc_other)
                } else {
                    fmt.ftc_other.or(fmt.ftc_ascii)
                };
                ftc.and_then(font)
                    .and_then(|f| f.glyph(ch as u32))
                    .unwrap_or(ch)
            }
            _ => ch,
        };
        para.push(ch, fmt);
    }

    /// Walk CPs `from..to` paragraph by paragraph into `doc`. Only the main
    /// story places text boxes (`textboxes`): a text box's own story never
    /// anchors another.
    fn walk(
        &self,
        cache: &mut ChpxCache,
        from: u64,
        to: u64,
        textboxes: bool,
        doc: &mut DoclingDocument,
    ) {
        let mut builder = NodeBuilder::new(self.lists.clone());
        let mut para = ParaAccum::default();
        for cp in from..to {
            let Some((ch, fc)) = self.char_at(cp) else {
                break;
            };
            match ch {
                '\r' | '\u{0007}' | '\u{000C}' => {
                    // Paragraph / cell / page mark: property lookup is by
                    // the mark's own FC.
                    let props = paragraph_props(self.word, self.bte, fc, self.format);
                    para.finish(ch, props, self.stis, &mut builder, doc);
                }
                // Inline picture anchor: the run's CHPX locates the PICF.
                '\u{0001}' => {
                    if let Some(pic_fc) = cache.props(self.word, self.btec, fc, self.format).pic_fc
                    {
                        para.add_picture(inline_picture(self.data, pic_fc));
                    }
                }
                // Floating-shape anchor: PlcfSpa maps this CP to a shape — its
                // pictures, and its text box story (#535).
                '\u{0008}' => {
                    if let Some(spid) = spa_shape_at(self.spa, cp) {
                        for image in self.drawings.shape_pictures(spid, 0) {
                            para.add_picture(image);
                        }
                        if let Some(&(a, b)) = self.textboxes.get(&spid).filter(|_| textboxes) {
                            self.placed.borrow_mut().insert(spid);
                            let mut inner = DoclingDocument::new("");
                            self.walk(cache, a, b, false, &mut inner);
                            if !inner.nodes.is_empty() {
                                para.textboxes.push(inner.nodes);
                            }
                        }
                    }
                }
                _ => self.push_char(
                    &mut para,
                    ch,
                    cache.props(self.word, self.btec, fc, self.format),
                ),
            }
        }
        para.finish('\r', ParaProps::default(), self.stis, &mut builder, doc);
        builder.flush(doc);
    }

    /// The plain text of CPs `from..to`, one entry per non-empty paragraph
    /// (fields resolved to their results, note reference marks dropped) —
    /// header/footer and note bodies.
    fn paragraphs(&self, cache: &mut ChpxCache, from: u64, to: u64) -> Vec<String> {
        let mut out = Vec::new();
        let mut para = ParaAccum::default();
        for cp in from..to {
            let Some((ch, fc)) = self.char_at(cp) else {
                break;
            };
            match ch {
                '\r' | '\u{0007}' | '\u{000C}' => {
                    let text = para.plain().trim().to_string();
                    para.segments.clear();
                    para.field_stack.clear();
                    if !text.is_empty() {
                        out.push(text);
                    }
                }
                _ => self.push_char(
                    &mut para,
                    ch,
                    cache.props(self.word, self.btec, fc, self.format),
                ),
            }
        }
        let text = para.plain().trim().to_string();
        if !text.is_empty() {
            out.push(text);
        }
        out
    }
}

/// `PlcftxbxTxt` ([MS-DOC] 2.8.28): `n + 1` CPs into the text box story,
/// then `n` 22-byte FTXBXS whose `lid` (at byte 14) is the shape id of the
/// box showing that story — what PlcfSpa's anchors name. Word appends a
/// dummy last entry (`lid` 0 / -1), skipped. Ranges come back absolute
/// (`base` = the text box story's first CP), clamped to its `len`.
fn parse_textboxes(plc: &[u8], base: u64, len: u64) -> std::collections::HashMap<u32, (u64, u64)> {
    let mut out = std::collections::HashMap::new();
    if plc.len() < 4 {
        return out;
    }
    let n = (plc.len() - 4) / (4 + 22);
    for i in 0..n {
        let (Some(a), Some(b)) = (u32_at(plc, i * 4), u32_at(plc, i * 4 + 4)) else {
            break;
        };
        let Some(lid) = u32_at(plc, (n + 1) * 4 + i * 22 + 14) else {
            break;
        };
        let (a, b) = (a as u64, (b as u64).min(len));
        if lid != 0 && lid != u32::MAX && b > a {
            out.insert(lid, (base + a, base + b));
        }
    }
    out
}

/// The text boxes of the headers and footers (#574): `anchors` (PlcSpaHdr)
/// places each shape at a CP of the header story, `stories`
/// (PlcftxbxHdrTxt over the `ccpHdrTxbx` story) gives its text, by shape id.
struct HeaderBoxes {
    anchors: Vec<(u64, u32)>,
    stories: std::collections::HashMap<u32, (u64, u64)>,
}

/// Each section's header / footer text (#535): `PlcfHdd`'s CPs into the
/// header story — six separator stories first, then per section even /
/// odd (default) / first header and footer, as `(is_footer, paragraph)`
/// in reading order (default, first, even — headers before footers), each
/// distinct paragraph once. The text boxes anchored in a header or footer
/// story (#574) come first, as the DOCX backend's `textbox` group comes
/// before the part's own paragraphs; a box no story anchors follows as
/// header text, so it is never lost.
fn header_footer_texts(
    story: &Story,
    cache: &mut ChpxCache,
    plc: &[u8],
    base: u64,
    len: u64,
    boxes: &HeaderBoxes,
) -> Vec<(bool, String)> {
    let cps: Vec<u64> = plc
        .chunks_exact(4)
        .filter_map(|c| u32_at(c, 0).map(u64::from))
        .collect();
    let mut out: Vec<(bool, String)> = Vec::new();
    let push = |out: &mut Vec<(bool, String)>, footer: bool, text: String| {
        if !out.iter().any(|(f, t)| *f == footer && *t == text) {
            out.push((footer, text));
        }
    };
    let mut placed: std::collections::HashSet<u32> = Default::default();
    let mut section = 6;
    while section + 6 < cps.len() {
        // Story order within a section: even hdr, odd hdr, even ftr, odd
        // ftr, first hdr, first ftr.
        for (k, footer) in [
            (1, false),
            (4, false),
            (0, false),
            (3, true),
            (5, true),
            (2, true),
        ] {
            let (a, b) = (cps[section + k], cps[section + k + 1].min(len));
            if a >= b {
                continue;
            }
            for &(cp, spid) in &boxes.anchors {
                let Some(&(ta, tb)) = boxes.stories.get(&spid) else {
                    continue;
                };
                if (a..b).contains(&cp) && placed.insert(spid) {
                    for text in story.paragraphs(cache, ta, tb) {
                        push(&mut out, footer, text);
                    }
                }
            }
            for text in story.paragraphs(cache, base + a, base + b) {
                push(&mut out, footer, text);
            }
        }
        section += 6;
    }
    let mut rest: Vec<(u64, u64)> = boxes
        .stories
        .iter()
        .filter(|(spid, _)| !placed.contains(spid))
        .map(|(_, &range)| range)
        .collect();
    rest.sort_unstable();
    for (ta, tb) in rest {
        for text in story.paragraphs(cache, ta, tb) {
            push(&mut out, false, text);
        }
    }
    out
}

/// The footnote (or endnote) bodies (#535): `refs` (PlcffndRef: `n + 1`
/// CPs + `n` 2-byte FRDs) says how many there are; `txt` (PlcffndTxt) gives
/// each one's CPs in its story, starting at `base`. A note's paragraphs
/// are joined with a space, like the DOCX backend's.
fn note_texts(
    story: &Story,
    cache: &mut ChpxCache,
    refs: &[u8],
    txt: &[u8],
    base: u64,
    len: u64,
) -> Vec<String> {
    if refs.len() < 4 || len == 0 {
        return Vec::new();
    }
    let n = (refs.len() - 4) / 6;
    (0..n)
        .filter_map(|i| {
            let a = u32_at(txt, i * 4)? as u64;
            let b = (u32_at(txt, i * 4 + 4)? as u64).min(len);
            (a < b).then(|| story.paragraphs(cache, base + a, base + b).join(" "))
        })
        .filter(|t| !t.is_empty())
        .collect()
}

/// The Word 97+ FIB, located from the counts the stream declares
/// ([MS-DOC] 2.5.1): `FibBase` (32 bytes), `csw` + `fibRgW`, `cslw` +
/// `fibRgLw`, `cbRgFcLcb` + `fibRgFcLcbBlob`. Writers size these arrays by
/// `nFib`; reading them through the declared counts — rather than the
/// fixed offsets of the common `csw = 14`, `cslw = 22` layout — keeps a
/// non-standard or truncated FIB from silently reading garbage.
struct Fib<'a> {
    ccp_text: u32,
    /// The other stories' lengths in CPs, in their CP-space order after the
    /// main text ([MS-DOC] 2.5.4 `FibRgLw97`); 0 when the FIB is too short
    /// to say.
    ccp_ftn: u32,
    ccp_hdd: u32,
    ccp_atn: u32,
    ccp_edn: u32,
    ccp_txbx: u32,
    /// The header text box story (`ccpHdrTxbx`, #574), after `ccp_txbx`.
    ccp_hdr_txbx: u32,
    /// `fibRgFcLcbBlob`: `(fc, lcb)` u32 pairs.
    fc_lcb: &'a [u8],
}

impl<'a> Fib<'a> {
    /// Indices into `FibRgFcLcb97` (each an 8-byte `fc`/`lcb` pair).
    const STSHF: usize = 1;
    const PLCF_FND_REF: usize = 2;
    const PLCF_FND_TXT: usize = 3;
    const PLCF_HDD: usize = 11;
    const PLCF_BTE_CHPX: usize = 12;
    const PLCF_BTE_PAPX: usize = 13;
    const STTBF_FFN: usize = 15;
    const CLX: usize = 33;
    const PLCSPA_MOM: usize = 40;
    const PLCSPA_HDR: usize = 41;
    const PLCF_END_REF: usize = 46;
    const PLCF_END_TXT: usize = 47;
    const DGG_INFO: usize = 50;
    const PLCF_TXBX_TXT: usize = 56;
    const PLCF_HDR_TXBX_TXT: usize = 58;
    const PLF_LST: usize = 73;
    const PLF_LFO: usize = 74;

    fn parse(word: &'a [u8]) -> Result<Self, ConversionError> {
        let short = |what: &str, need: usize| {
            ConversionError::Parse(format!(
                "doc: FIB truncated — {what} needs {need} bytes, WordDocument has {}",
                word.len()
            ))
        };
        let csw = u16_at(word, 32).ok_or_else(|| short("csw", 34))? as usize;
        let cslw_at = 34 + csw * 2;
        let cslw = u16_at(word, cslw_at).ok_or_else(|| short("cslw", cslw_at + 2))? as usize;
        let lw_at = cslw_at + 2;
        if cslw < 4 {
            return Err(ConversionError::Parse(format!(
                "doc: FIB's fibRgLw has {cslw} entries, too few for ccpText"
            )));
        }
        let ccp_text = u32_at(word, lw_at + 12).ok_or_else(|| short("ccpText", lw_at + 16))?;
        // `fibRgLw` index → CP count, when the writer's `cslw` covers it.
        let lw = |i: usize| {
            if i < cslw {
                u32_at(word, lw_at + i * 4).unwrap_or(0)
            } else {
                0
            }
        };
        let cb_at = lw_at + cslw * 4;
        let cb = u16_at(word, cb_at).ok_or_else(|| short("cbRgFcLcb", cb_at + 2))? as usize;
        let blob_at = cb_at + 2;
        let fc_lcb = word
            .get(blob_at..blob_at + cb * 8)
            .ok_or_else(|| short("fibRgFcLcbBlob", blob_at + cb * 8))?;
        Ok(Self {
            ccp_text,
            ccp_ftn: lw(4),
            ccp_hdd: lw(5),
            ccp_atn: lw(7),
            ccp_edn: lw(8),
            ccp_txbx: lw(9),
            ccp_hdr_txbx: lw(10),
            fc_lcb,
        })
    }

    /// The `index`-th `(fc, lcb)` pair; `(0, 0)` — "absent" — past the
    /// writer's `cbRgFcLcb`.
    fn fc_lcb(&self, index: usize) -> (usize, usize) {
        let fc = u32_at(self.fc_lcb, index * 8).unwrap_or(0) as usize;
        let lcb = u32_at(self.fc_lcb, index * 8 + 4).unwrap_or(0) as usize;
        (fc, lcb)
    }

    /// The bytes of structure `index` in `stream`; empty when absent or when
    /// the range overruns the stream (optional structures degrade).
    fn part<'s>(&self, stream: &'s [u8], index: usize) -> &'s [u8] {
        let (fc, lcb) = self.fc_lcb(index);
        fc.checked_add(lcb)
            .and_then(|end| stream.get(fc..end))
            .unwrap_or(&[])
    }
}

/// Word 6.0 / Word 95 (`nFib` 101–105, #640): the same story walk as Word
/// 97, over the pre-97 layout ([`DocFormat::Word6`]). The FIB is fixed:
/// `fcMin` 0x18 / `fcMac` 0x1C bound the text, the story lengths sit at
/// 0x34 (`ccpText`, `ccpFtn`, `ccpHdd`, `ccpMcr`, `ccpAtn`, `ccpEdn`,
/// `ccpTxbx`, `ccpHdrTxbx`), and the structures live in `WordDocument`
/// itself (no table stream): the stylesheet at 0x60, the footnote PLCs at
/// 0x68 / 0x70, the section table at 0x88, `PlcfHdd` at 0xB0, the CHPX and
/// PAPX bin tables at 0xB8 / 0xC0, the CLX at 0x160 — which only matters
/// for a fast-saved file (`fComplex`); otherwise the text is one piece at
/// `fcMin`. `fExtChar` (flag 0x1000, the Far East editions) makes the text
/// UTF-16LE instead of 8-bit. A bin table the FIB does not place (or an
/// older writer's layout the walk cannot follow) leaves every paragraph
/// with default properties — the body-paragraph output this reader gave
/// before, never an error.
///
/// Read: tables (cell / row marks + `sprmPFInTable` / `sprmPTtp`),
/// headings (the built-in Heading `sti`s of the stylesheet), bold / italic,
/// fields (result text), headers and footers (furniture; which stories a
/// section has comes from its SEPX's `sprmSGprfIhdt`), footnotes. Not
/// read: pictures (a Word 6 PICF holds a WMF / DIB the Word 97 decoder does
/// not take — the anchor is a placeholder picture), text boxes, autonumber
/// lists (`sprmPAnld`), endnotes.
fn convert_word6(name: &str, word: &[u8], flags: u16) -> Result<DoclingDocument, ConversionError> {
    let field = |o: usize, what: &str| {
        u32_at(word, o).map(|v| v as usize).ok_or_else(|| {
            ConversionError::Parse(format!(
                "doc: Word 6/95 FIB truncated — no {what} at {o:#x} ({} bytes)",
                word.len()
            ))
        })
    };
    let fc_min = field(0x18, "fcMin")?;
    let fc_mac = field(0x1C, "fcMac")?;
    let ccp_text = field(0x34, "ccpText")? as u64;
    // The other stories, in CP order after the main text (absent → 0).
    let ccp = |o: usize| u32_at(word, o).unwrap_or(0) as u64;
    let (ccp_ftn, ccp_hdd) = (ccp(0x38), ccp(0x3C));
    let total_cp: u64 = (0x34..=0x50).step_by(4).map(ccp).sum();
    // `fExtChar`: the Far East editions store the text as UTF-16LE.
    let unicode = flags & 0x1000 != 0;
    let pieces = if flags & 0x0004 != 0 {
        let fc_clx = field(0x160, "fcClx")?;
        let lcb_clx = field(0x164, "lcbClx")?;
        let clx = fc_clx
            .checked_add(lcb_clx)
            .and_then(|end| word.get(fc_clx..end))
            .ok_or_else(|| {
                ConversionError::Parse(format!(
                    "doc: piece table (fcClx {fc_clx}, lcbClx {lcb_clx}) lies outside the \
                     WordDocument stream ({} bytes)",
                    word.len()
                ))
            })?;
        parse_piece_table(clx, Some(unicode)).ok_or_else(|| {
            ConversionError::Parse(format!(
                "doc: bad piece table (fcClx {fc_clx}, lcbClx {lcb_clx} in WordDocument)"
            ))
        })?
    } else {
        // One piece over every story: `fcMin..fcMac` holds them all.
        let chars = (fc_mac.saturating_sub(fc_min) / if unicode { 2 } else { 1 }) as u64;
        vec![Piece {
            cp_start: 0,
            cp_end: total_cp.max(ccp_text).min(chars.max(ccp_text)),
            fc: fc_min as u64,
            compressed: !unicode,
        }]
    };
    // An `(fc, lcb)` structure of the FIB, empty when absent or out of range.
    let part = |o: usize| -> &[u8] {
        let (Some(fc), Some(lcb)) = (u32_at(word, o), u32_at(word, o + 4)) else {
            return &[];
        };
        (fc as usize)
            .checked_add(lcb as usize)
            .and_then(|end| word.get(fc as usize..end))
            .unwrap_or(&[])
    };
    let stis = parse_stsh(part(0x60), DocFormat::Word6);
    let lists = ListTables::default();
    let drawings = Drawings::default();
    let story = Story {
        format: DocFormat::Word6,
        word,
        pieces: &pieces,
        bte: part(0xC0),
        btec: part(0xB8),
        stis: &stis,
        data: &[],
        spa: &[],
        drawings: &drawings,
        textboxes: Default::default(),
        placed: Default::default(),
        fonts: &[],
        lists: &lists,
    };
    let mut cache = ChpxCache::default();
    let mut doc = DoclingDocument::new(name);
    story.walk(&mut cache, 0, ccp_text, false, &mut doc);

    // Headers / footers (furniture, after the body): a section's stories in
    // `PlcfHdd` are the ones its `grpfIhdt` names, in bit order.
    let hdd_base = ccp_text + ccp_ftn;
    for (footer, text) in word6_header_footer_texts(
        &story,
        &mut cache,
        part(0xB0),
        word6_section_ihdt(word, part(0x88)),
        hdd_base,
        ccp_hdd,
    ) {
        doc.push(Node::FurnitureText {
            label: if footer { "page_footer" } else { "page_header" }.into(),
            text: escape_text(&text),
        });
    }
    for text in note_texts(
        &story,
        &mut cache,
        part(0x68),
        part(0x70),
        ccp_text,
        ccp_ftn,
    ) {
        doc.push(Node::FurnitureText {
            label: "footnote".into(),
            text: escape_text(&text),
        });
    }
    Ok(doc)
}

/// Each section's `grpfIhdt` (which header / footer stories it has: bit 0
/// even header, 1 odd header, 2 even footer, 3 odd footer, 4 first header,
/// 5 first footer) from the Word 6/95 section table: `PlcfSed` is `n + 1`
/// CPs then `n` 12-byte SEDs whose `fcSepx` locates the SEPX (a word count,
/// then the section's sprms) in `WordDocument`; `sprmSGprfIhdt` is opcode
/// 153. A section whose SEPX cannot be read has no stories.
fn word6_section_ihdt(word: &[u8], plcfsed: &[u8]) -> Vec<u8> {
    let Some(n) = plcfsed.len().checked_sub(4).map(|l| l / 16) else {
        return Vec::new();
    };
    (0..n)
        .map(|i| {
            let sed = (n + 1) * 4 + i * 12;
            let Some(fc_sepx) = u32_at(plcfsed, sed + 2) else {
                return 0;
            };
            let at = fc_sepx as usize;
            let Some(cb) = u16_at(word, at) else {
                return 0;
            };
            let grpprl = word.get(at + 2..at + 2 + cb as usize).unwrap_or(&[]);
            let mut ihdt = 0;
            walk_word6_sprms(grpprl, |op, operand| {
                if op == 153 {
                    ihdt = operand[0];
                }
            });
            ihdt
        })
        .collect()
}

/// The Word 6/95 twin of [`header_footer_texts`]: `PlcfHdd` lists, per
/// section, only the stories its `grpfIhdt` names (no separator stories,
/// no fixed six), so the bits say which CP range is which. Emitted in the
/// Word 97 reader's reading order — default, first, even; headers before
/// footers — each distinct paragraph once.
fn word6_header_footer_texts(
    story: &Story,
    cache: &mut ChpxCache,
    plc: &[u8],
    ihdts: Vec<u8>,
    base: u64,
    len: u64,
) -> Vec<(bool, String)> {
    let cps: Vec<u64> = plc
        .chunks_exact(4)
        .filter_map(|c| u32_at(c, 0).map(u64::from))
        .collect();
    let mut out: Vec<(bool, String)> = Vec::new();
    let mut next = 0usize;
    for ihdt in ihdts {
        // Story index per kind, in file order (bit order).
        let mut kinds: [Option<usize>; 6] = [None; 6];
        for (bit, slot) in kinds.iter_mut().enumerate() {
            if ihdt & (1 << bit) != 0 {
                *slot = Some(next);
                next += 1;
            }
        }
        for (k, footer) in [
            (1, false),
            (4, false),
            (0, false),
            (3, true),
            (5, true),
            (2, true),
        ] {
            let Some(i) = kinds[k] else {
                continue;
            };
            let (Some(&a), Some(&b)) = (cps.get(i), cps.get(i + 1)) else {
                continue;
            };
            let b = b.min(len);
            if a >= b {
                continue;
            }
            for text in story.paragraphs(cache, base + a, base + b) {
                if !out.iter().any(|(f, t)| *f == footer && *t == text) {
                    out.push((footer, text));
                }
            }
        }
    }
    out
}

/// Whether `data` is a flat Word for Windows 1.x / 2.0 file (#566): the
/// pre-Word-6 `wIdent` at byte 0, an `nFib` of that era, and a text start
/// (`fcMin`) inside the file.
pub(crate) fn is_word2(data: &[u8]) -> bool {
    matches!(u16_at(data, 0), Some(0xA59B | 0xA59C | 0xA5DB))
        && u16_at(data, 2).is_some_and(|n| (1..=63).contains(&n))
        && u32_at(data, 0x18).is_some_and(|fc_min| (fc_min as usize) < data.len())
}

/// Word for Windows 1.x / 2.0 (#566; module docs). The flat file's FIB:
/// `fcMin` 0x18, `fcMac` 0x1C, `ccpText` 0x34, and from 0x52 on 6-byte
/// (`fc` u32, `cb` u16) pairs — the CHPX bin table at 0xA0, the PAPX one at
/// 0xA6. Each bin table is a PLC of FCs and 16-bit page numbers; a page is a
/// 512-byte FKP: `crun` in its last byte, `crun + 1` FCs, then one byte per
/// run — the word offset of its PAPX / CHPX in the page (0: none), which
/// starts with its byte count. A PAPX is the style code, six fixed bytes,
/// then single-byte sprms; a CHPX starts with the character flags (bit 0
/// bold, bit 1 italic). Paragraph marks are `\r\n`, cell marks `\r\x07`,
/// the row end a `\r\x07` paragraph whose PAPX carries the table sprms.
fn convert_word2(name: &str, data: &[u8]) -> Result<DoclingDocument, ConversionError> {
    let flags = u16_at(data, 0x0A).unwrap_or(0);
    if flags & 0x0100 != 0 {
        return Err(crate::backend::offcrypto::encrypted("doc"));
    }
    if flags & 0x0004 != 0 {
        return Err(ConversionError::Parse(
            "doc: fast-saved (complex) Word 1.x/2.0 file — its text is in pieces this reader \
             does not follow; open it in Word and save without Fast Save"
                .into(),
        ));
    }
    let field = |o: usize, what: &str| {
        u32_at(data, o).map(|v| v as usize).ok_or_else(|| {
            ConversionError::Parse(format!(
                "doc: Word 2.0 FIB truncated — no {what} at {o:#x} ({} bytes)",
                data.len()
            ))
        })
    };
    let fc_min = field(0x18, "fcMin")?;
    let fc_mac = field(0x1C, "fcMac")?;
    let ccp_text = field(0x34, "ccpText")?;
    let end = fc_min.saturating_add(ccp_text).min(fc_mac).min(data.len());
    let text = data.get(fc_min..end).unwrap_or(&[]);
    // The bin tables; a FIB too short for them means no formatting at all.
    let plc = |o: usize| -> &[u8] {
        let (Some(fc), Some(cb)) = (u32_at(data, o), u16_at(data, o + 4)) else {
            return &[];
        };
        let (fc, cb) = (fc as usize, cb as usize);
        fc.checked_add(cb)
            .and_then(|e| data.get(fc..e))
            .unwrap_or(&[])
    };
    let chpx_plc = plc(0xA0);
    let papx_plc = plc(0xA6);
    let styles = word2_styles(data);

    let mut doc = DoclingDocument::new(name);
    let mut builder = NodeBuilder::new(ListTables::default());
    let mut para = ParaAccum::default();
    let mut i = 0usize;
    while i < text.len() {
        let fc = fc_min + i;
        let b = text[i];
        if b == 0x0D {
            // `\r\n`: a paragraph; `\r\x07`: a cell (or the row end — the
            // PAPX says which); a bare `\r`: a paragraph too.
            let next = text.get(i + 1).copied();
            i += if matches!(next, Some(0x0A | 0x07)) {
                2
            } else {
                1
            };
            let mut props = ParaProps::default();
            let papx = word2_fkp(data, papx_plc, fc as u64);
            // The PAPX opens with the paragraph's style code (`stc`); no PAPX
            // means the default style, `stc` 0 — Word 2.0's "Normal".
            props.istd = papx.and_then(|p| p.first()).copied().unwrap_or(0) as u16;
            if next == Some(0x07) {
                let (in_table, ttp) = papx.map(word2_table_flags).unwrap_or((true, false));
                props.in_table = in_table || !ttp;
                props.ttp = ttp;
            }
            let mark = if next == Some(0x07) { '\u{0007}' } else { '\r' };
            para.finish(mark, props, &styles, &mut builder, &mut doc);
            continue;
        }
        let fmt = word2_fkp(data, chpx_plc, fc as u64)
            .and_then(|chpx| chpx.first())
            .map_or(CharFmt::default(), |&flags| CharFmt {
                bold: flags & 0x01 != 0,
                italic: flags & 0x02 != 0,
                ..CharFmt::default()
            });
        para.push(cp1252(b), fmt);
        i += 1;
    }
    para.finish('\r', ParaProps::default(), &styles, &mut builder, &mut doc);
    builder.flush(&mut doc);
    Ok(doc)
}

/// Word 2.0's "Normal" is style code 0 — the default of a paragraph without
/// a PAPX — and the standard styles occupy 222..=255 (the name slots Word
/// fills in itself): 222 null, 242 footer, 243 header, 244/245 footnote
/// reference/text, **254 heading 1 down to 246 heading 9**, 255 normal
/// indent. (LibreOffice's `ww1` filter, `Ww1Style::ReadName`.)
const WORD2_STC_HEADING1: u8 = 254;
const WORD2_STC_HEADING9: u8 = 246;

/// The Word 2.0 stylesheet (`fcStshf` 0x5E / `cbStshf` 0x62 in the FIB) as
/// `stc → StyleDef`, 256 slots (#573). The STSH is `cstcStd`, then four
/// blocks each opening with its byte count: the names, the CHPXes, the
/// PAPXes (per style a count byte — 0xFF: slot unused, 0: defaults — then
/// the bytes), and the ESTCPs (`iMac`, then per style `stcNext`, `stcBase`).
/// Slot `stcp` of a block is style code `(stcp - cstcStd) & 255`, so the
/// standard styles come first. A standard heading is `sti` 1..=9 as in the
/// Word 97 STSH ([`parse_stsh`]), so [`NodeBuilder::paragraph`] renders it
/// at the same Markdown level the DOCX backend gives "heading N"; a user
/// style based on one (one hop, as the DOCX backend reads `basedOn`) is that
/// heading too. Everything else, and a missing or unreadable stylesheet, is
/// no style.
fn word2_styles(data: &[u8]) -> Vec<StyleDef> {
    let none = StyleDef::NONE;
    let heading_sti = |stc: u8| {
        (WORD2_STC_HEADING9..=WORD2_STC_HEADING1)
            .contains(&stc)
            .then(|| (255 - stc) as u16)
    };
    let mut styles = vec![none; 256];
    for (stc, def) in styles.iter_mut().enumerate() {
        if let Some(sti) = heading_sti(stc as u8) {
            def.sti = sti;
        }
    }
    // The based-on chain needs the ESTCP block, which follows three
    // variable blocks: walk them.
    let (Some(fc), Some(cb)) = (u32_at(data, 0x5E), u16_at(data, 0x62)) else {
        return styles;
    };
    let Some(stsh) = data.get(fc as usize..(fc as usize).saturating_add(cb as usize)) else {
        return styles;
    };
    let Some(cstc_std) = u16_at(stsh, 0) else {
        return styles;
    };
    let mut pos = 2usize;
    for _ in 0..3 {
        let Some(cb_block) = u16_at(stsh, pos) else {
            return styles;
        };
        pos = pos.saturating_add(cb_block.max(2) as usize);
    }
    let Some(imac) = u16_at(stsh, pos) else {
        return styles;
    };
    pos += 2;
    for stcp in 0..imac as usize {
        let stc = stcp.wrapping_sub(cstc_std as usize) & 255;
        let (Some(&_next), Some(&base)) = (stsh.get(pos), stsh.get(pos + 1)) else {
            break;
        };
        pos += 2;
        if styles[stc].sti == 0x0FFF && base as usize != stc {
            if let Some(sti) = heading_sti(base) {
                styles[stc].sti = sti;
            }
        }
    }
    styles
}

/// The PAPX / CHPX bytes (after their count byte) of the run holding `fc`,
/// through a Word 2.0 bin table and its FKP page; `None` when the run has no
/// property bytes (the defaults apply) or the structures are out of range.
fn word2_fkp<'a>(data: &'a [u8], plc: &[u8], fc: u64) -> Option<&'a [u8]> {
    let n = plc.len().checked_sub(4)? / 6;
    let pn = (0..n).find_map(|k| {
        let start = u32_at(plc, k * 4)? as u64;
        let end = u32_at(plc, (k + 1) * 4)? as u64;
        (start <= fc && fc < end).then(|| u16_at(plc, (n + 1) * 4 + k * 2))?
    })?;
    let page = data.get(pn as usize * 512..pn as usize * 512 + 512)?;
    let crun = *page.last()? as usize;
    let fcs_end = (crun + 1) * 4;
    let run = (0..crun).find(|&r| {
        let start = u32_at(page, r * 4).unwrap_or(u32::MAX) as u64;
        let end = u32_at(page, (r + 1) * 4).unwrap_or(0) as u64;
        start <= fc && fc < end
    })?;
    let bx = *page.get(fcs_end + run)? as usize;
    if bx == 0 {
        return None;
    }
    let cb = *page.get(bx * 2)? as usize;
    page.get(bx * 2 + 1..bx * 2 + 1 + cb)
}

/// `(in_table, row_end)` from a Word 2.0 PAPX: the style code, six fixed
/// bytes, then sprms. The cell paragraphs of the sample file carry the
/// operand-less `0x11`; the row-end paragraph `0x18 01 0x19 01` followed by
/// the table sprms (`0x94` = half the cell gap, 108 twips). Read as: `0x11`
/// and `0x18 n` put the paragraph in a table, `0x19 n` ends the row. The
/// scan stops at the first other sprm, whose operand size is not known
/// here — what matters comes first.
fn word2_table_flags(papx: &[u8]) -> (bool, bool) {
    let (mut in_table, mut ttp) = (false, false);
    let mut i = 7usize;
    while i < papx.len() {
        match papx[i] {
            0x11 => {
                in_table = true;
                i += 1;
            }
            0x18 => {
                in_table |= papx.get(i + 1).is_some_and(|&v| v != 0);
                i += 2;
            }
            0x19 => {
                ttp |= papx.get(i + 1).is_some_and(|&v| v != 0);
                i += 2;
            }
            _ => break,
        }
    }
    (in_table, ttp)
}

/// One piece-table entry: characters `cp_start..cp_end` live at byte offset
/// `fc` (already unmasked) — CP1252 bytes when `compressed`, else UTF-16LE.
struct Piece {
    cp_start: u64,
    cp_end: u64,
    fc: u64,
    compressed: bool,
}

/// The `i`-th character of a piece (bounds-safe; U+FFFD off the end).
fn piece_char(word: &[u8], piece: &Piece, i: u64) -> char {
    if piece.compressed {
        let b = word.get((piece.fc + i) as usize).copied().unwrap_or(0);
        cp1252(b)
    } else {
        let o = (piece.fc + 2 * i) as usize;
        let u = u16_at(word, o).unwrap_or(0xFFFD);
        char::from_u32(u as u32).unwrap_or('\u{FFFD}')
    }
}

/// The byte offset (FC) of the `i`-th character of a piece.
fn piece_fc(piece: &Piece, i: u64) -> u64 {
    if piece.compressed {
        piece.fc + i
    } else {
        piece.fc + 2 * i
    }
}

/// Parse the CLX into pieces. The CLX is a run of `Prc` blocks (0x01, skipped)
/// followed by the `Pcdt` (0x02) holding the PlcPcd.
///
/// `word6`: a Word 6.0/95 CLX — `Some(unicode)` — whose PCD `fc` is a plain
/// byte offset (bit 30 carries no meaning there) to text that is 8-bit, or
/// UTF-16LE throughout when the FIB's `fExtChar` says so.
fn parse_piece_table(clx: &[u8], word6: Option<bool>) -> Option<Vec<Piece>> {
    let mut pos = 0usize;
    loop {
        match clx.get(pos)? {
            0x01 => {
                let cb = u16_at(clx, pos + 1)? as usize;
                pos += 3 + cb;
            }
            0x02 => {
                let lcb = u32_at(clx, pos + 1)? as usize;
                let plc = clx.get(pos + 5..(pos + 5).checked_add(lcb)?)?;
                // n pieces: (n+1) CPs (4 bytes) + n PCDs (8 bytes).
                let n = (lcb.checked_sub(4)?) / 12;
                let mut pieces = Vec::with_capacity(n);
                for i in 0..n {
                    let cp_start = u32_at(plc, i * 4)? as u64;
                    let cp_end = u32_at(plc, (i + 1) * 4)? as u64;
                    let pcd = (n + 1) * 4 + i * 8;
                    let fc_raw = u32_at(plc, pcd + 2)?;
                    let compressed = match word6 {
                        Some(unicode) => !unicode,
                        None => fc_raw & 0x4000_0000 != 0,
                    };
                    let fc = if word6.is_some() {
                        fc_raw as u64
                    } else if compressed {
                        ((fc_raw & 0x3FFF_FFFF) / 2) as u64
                    } else {
                        fc_raw as u64
                    };
                    pieces.push(Piece {
                        cp_start,
                        cp_end,
                        fc,
                        compressed,
                    });
                }
                return Some(pieces);
            }
            _ => return None,
        }
    }
}

/// Properties of one paragraph, read from its PAPX.
#[derive(Default, Clone, Copy)]
struct ParaProps {
    istd: u16,
    in_table: bool,
    ttp: bool,
    /// List-format reference (`sprmPIlfo`) set in this PAPX: non-zero → a
    /// numbered/bulleted list item, 0 → explicitly none. `None` = not set
    /// here; a paragraph then inherits its style's (#641), since a PAPX is
    /// the delta over the style's properties ([MS-DOC] 2.4.6.1).
    ilfo: Option<u16>,
    /// List nesting level (`sprmPIlvl`) set in this PAPX, 0-based; `None`
    /// inherits like `ilfo` and defaults to 0.
    ilvl: Option<u8>,
    /// Outline level (`sprmPOutLvl`, 0–8 = heading levels 1–9; 9 = body
    /// text). Read off *style* PAPX UPXes only — mirroring docling, which
    /// consults the style definition, never the paragraph's direct formatting.
    outline: Option<u8>,
}

/// The two binary layouts the FKP walks read (#640): Word 97+ ([MS-DOC])
/// and Word 6.0/95 (the pre-97 "Word 6.0 Binary File Format"), which
/// differ in three places only — a bin table's page numbers are 2 bytes
/// instead of 4, a PAPX FKP's `BX` is 7 bytes (offset + a 6-byte PHE)
/// instead of 13, and the sprms are single-byte opcodes with their own
/// numbering and operand sizes ([`word6_sprm_len`]). Everything else —
/// the 512-byte pages, `crun` in the last byte, the FC array, the
/// `istd`-first PAPX, the cell / row / paragraph marks, fields — is the
/// same, so one walker serves both.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DocFormat {
    Word97,
    Word6,
}

impl DocFormat {
    /// Bytes per page number in a `PlcfBte`.
    fn pn_size(self) -> usize {
        match self {
            DocFormat::Word97 => 4,
            DocFormat::Word6 => 2,
        }
    }
    /// Bytes per `BX` entry of a PAPX FKP.
    fn bx_size(self) -> usize {
        match self {
            DocFormat::Word97 => 13,
            DocFormat::Word6 => 7,
        }
    }
}

/// The FKP page number for `fc` from a bin table (`PlcfBtePapx` /
/// `PlcfBteChpx`: `n + 1` FCs, then `n` page numbers of the format's size).
fn bin_table_page(bte: &[u8], fc: u64, format: DocFormat) -> Option<u32> {
    let n = bte.len().checked_sub(4)? / (4 + format.pn_size());
    (0..n).find_map(|i| {
        let lo = u32_at(bte, i * 4).unwrap_or(u32::MAX) as u64;
        let hi = u32_at(bte, (i + 1) * 4).unwrap_or(0) as u64;
        if fc < lo || fc >= hi {
            return None;
        }
        let at = (n + 1) * 4 + i * format.pn_size();
        match format {
            DocFormat::Word97 => u32_at(bte, at),
            DocFormat::Word6 => u16_at(bte, at).map(u32::from),
        }
    })
}

/// Look up the PAPX for the paragraph containing byte offset `fc`:
/// PlcfBtePapx → FKP page (512 bytes in the WordDocument stream) → PapxInFkp.
fn paragraph_props(word: &[u8], bte: &[u8], fc: u64, format: DocFormat) -> ParaProps {
    let mut props = ParaProps::default();
    let Some(pn) = bin_table_page(bte, fc, format) else {
        return props;
    };
    let page_off = (pn & 0x003F_FFFF) as usize * 512;
    let Some(page) = word.get(page_off..page_off + 512) else {
        return props;
    };
    let crun = page[511] as usize;
    let bx = format.bx_size();
    if crun == 0 || (crun + 1) * 4 + crun * bx > 511 {
        return props;
    }
    // rgfc[j] <= fc < rgfc[j+1] selects BxPap j.
    let mut run = None;
    for j in 0..crun {
        let lo = u32_at(page, j * 4).unwrap_or(u32::MAX) as u64;
        let hi = u32_at(page, (j + 1) * 4).unwrap_or(0) as u64;
        if fc >= lo && fc < hi {
            run = Some(j);
            break;
        }
    }
    let Some(j) = run else { return props };
    let b_offset = page[(crun + 1) * 4 + j * bx] as usize;
    if b_offset == 0 {
        return props; // default PAP
    }
    // PapxInFkp at word offset b_offset: cb, or 0 + cb'. Word 97's `cb`
    // counts itself (the grpprl is `2 * cb - 1` bytes); Word 6's counts the
    // grpprl's words alone (`2 * cb` bytes) — an `istd` + `sprmPFInTable`
    // PAPX is `cb` 2 there, and the shorter reading cut its last byte off.
    let mut o = b_offset * 2;
    let Some(&cb) = page.get(o) else { return props };
    let grpprl_len = if cb == 0 {
        o += 2;
        page.get(b_offset * 2 + 1).map(|&c| c as usize * 2)
    } else {
        o += 1;
        Some(match format {
            DocFormat::Word97 => cb as usize * 2 - 1,
            DocFormat::Word6 => cb as usize * 2,
        })
    };
    let Some(len) = grpprl_len else { return props };
    let Some(grpprl) = page.get(o..(o + len).min(512)) else {
        return props;
    };
    if grpprl.len() < 2 {
        return props;
    }
    props.istd = u16::from_le_bytes([grpprl[0], grpprl[1]]);
    match format {
        DocFormat::Word97 => apply_pap_sprms(&grpprl[2..], &mut props),
        DocFormat::Word6 => apply_word6_pap_sprms(&grpprl[2..], &mut props),
    }
    props
}

/// The operand size of a Word 6.0/95 sprm (the pre-97 single-byte opcodes,
/// "Word 6.0 Binary File Format" sprm table): `Some(n)` fixed, `None` for
/// an opcode this table does not know — the walk stops there, keeping what
/// it read, rather than desynchronising on a guessed width. Variable-length
/// sprms carry their byte count in the operand's first byte (`cb`), except
/// `sprmTDefTable10` (188) and `sprmTDefTable` (190), whose count is a
/// word. Verified against the FKPs of the #640 files: every PAPX / CHPX
/// grpprl walks to its end exactly (the regression fixtures assert it).
fn word6_sprm_len(op: u8, operand: &[u8]) -> Option<usize> {
    let var_byte = || operand.first().map(|&cb| 1 + cb as usize);
    let var_word = || u16_at(operand, 0).map(|cb| 2 + cb as usize);
    Some(match op {
        // PAP.
        2 => 2,
        3 => return var_byte(),
        4..=11 => 1,
        12 => return var_byte(),
        13 | 14 => 1,
        15 => return var_byte(),
        16..=19 => 2,
        20 => 4,
        21 | 22 => 2,
        23 => return var_byte(),
        24 | 25 => 1,
        26..=28 => 2,
        29 => 1,
        30..=36 => 2,
        37 => 1,
        38..=43 => 2,
        44 => 1,
        45..=49 => 2,
        50 | 51 => 1,
        52 => return var_byte(),
        53..=55 => 1,
        // CHP.
        65..=67 => 1,
        68 => 4,
        69 => 2,
        70 => 4,
        71 => 1,
        72 => 2,
        73 => 3,
        74 => return var_byte(),
        75 => 1,
        80 => 2,
        81 | 82 => return var_byte(),
        83 => 0,
        85..=92 => 1,
        93 => 2,
        94 | 95 => 1,
        96 | 97 => 2,
        98 => 1,
        99 => 2,
        100..=102 => 1,
        103 => return var_byte(),
        104 => 1,
        105 | 106 => return var_byte(),
        107 => 2,
        108 => return var_byte(),
        109 | 110 => 2,
        111 => 1,
        117 | 118 => 1,
        // PIC.
        119 => 1,
        120 => return var_byte(),
        121..=124 => 2,
        // SEP.
        131 | 132 => 1,
        133 => return var_byte(),
        136 | 137 => 3,
        138 | 139 => 1,
        140 | 141 => 2,
        142 | 143 => 1,
        144 | 145 => 2,
        146 | 147 => 1,
        148 | 149 => 2,
        150..=153 => 1,
        154..=157 => 2,
        158 | 159 => 1,
        160 | 161 => 2,
        162 | 163 => 1,
        164..=171 => 2,
        // TAP.
        182..=184 => 2,
        185 | 186 => 1,
        187 => 12,
        188 => return var_word(),
        189 => 2,
        190 => return var_word(),
        191 => return var_byte(),
        192 => 4,
        193 => 5,
        194 => 4,
        195 => 2,
        196 => 4,
        197 | 198 => 2,
        199 => 5,
        200 => 4,
        _ => return None,
    })
}

/// Walk a Word 6/95 grpprl, handing each `(opcode, operand)` to `f` (the
/// operand includes a variable sprm's count bytes). Ends at the first
/// unknown opcode or truncated operand.
fn walk_word6_sprms(mut sprms: &[u8], mut f: impl FnMut(u8, &[u8])) {
    while let Some((&op, rest)) = sprms.split_first() {
        let Some(len) = word6_sprm_len(op, rest) else {
            return;
        };
        if rest.len() < len {
            return;
        }
        f(op, &rest[..len]);
        sprms = &rest[len..];
    }
}

/// Scan a Word 6/95 PAP grpprl for the sprms this backend reads: the style
/// (`sprmPIstd` 2, when a UPX repeats it), `sprmPFInTable` (24) and
/// `sprmPTtp` (25) — the table structure the flat reader lacked (#640).
fn apply_word6_pap_sprms(sprms: &[u8], props: &mut ParaProps) {
    walk_word6_sprms(sprms, |op, operand| match op {
        2 => props.istd = u16::from_le_bytes([operand[0], operand[1]]),
        24 => props.in_table = operand[0] != 0,
        25 => props.ttp = operand[0] != 0,
        _ => {}
    });
}

/// Scan a Word 6/95 CHP grpprl: bold (85) / italic (86) — operand 0 / 1
/// absolute, 0x80 / 0x81 relative to the paragraph style (resolved at the
/// paragraph's end, see [`CharFmt::bold_from_style`]) — the run's font
/// (`sprmCFtc` 93) and a picture anchor's PICF offset
/// (`sprmCPicLocation` 68; the Word 6 PICF itself is not decoded, so the
/// anchor stays a placeholder picture).
fn apply_word6_chp_sprms(sprms: &[u8], fmt: &mut CharFmt) {
    walk_word6_sprms(sprms, |op, operand| match op {
        85 => match operand[0] {
            0x80 => fmt.bold_from_style = Some(false),
            0x81 => fmt.bold_from_style = Some(true),
            v => fmt.bold = v == 1,
        },
        86 => match operand[0] {
            0x80 => fmt.italic_from_style = Some(false),
            0x81 => fmt.italic_from_style = Some(true),
            v => fmt.italic = v == 1,
        },
        93 => fmt.ftc_ascii = Some(u16::from_le_bytes([operand[0], operand[1]])),
        68 => {
            fmt.pic_fc = Some(u32::from_le_bytes([
                operand[0], operand[1], operand[2], operand[3],
            ]))
        }
        _ => {}
    });
}

/// Scan a PAP grpprl for the sprms this backend cares about.
fn apply_pap_sprms(mut sprms: &[u8], props: &mut ParaProps) {
    while sprms.len() >= 2 {
        let sprm = u16::from_le_bytes([sprms[0], sprms[1]]);
        sprms = &sprms[2..];
        let spra = sprm >> 13;
        let operand_len = match spra {
            0 | 1 => 1,
            2 | 4 | 5 => 2,
            3 => 4,
            7 => 3,
            _ => {
                // Variable: first byte is the operand size (sprmTDefTable uses
                // a 16-bit size; it never appears in a PAP grpprl we scan).
                match sprms.first() {
                    Some(&cb) => 1 + cb as usize,
                    None => return,
                }
            }
        };
        if sprms.len() < operand_len {
            return;
        }
        match sprm {
            0x2416 => props.in_table = sprms[0] != 0, // sprmPFInTable
            0x2417 => props.ttp = sprms[0] != 0,      // sprmPFTtp
            0x460B => props.ilfo = Some(u16::from_le_bytes([sprms[0], sprms[1]])), // sprmPIlfo
            0x260A => props.ilvl = Some(sprms[0]),    // sprmPIlvl
            0x2640 => props.outline = Some(sprms[0]), // sprmPOutLvl
            _ => {}
        }
        sprms = &sprms[operand_len..];
    }
}

/// One paragraph style's identity from the STSH: the built-in style id (`sti`
/// 1–9 are the Heading 1–9 styles, 62 is Title, 0x0FFE/0x0FFF user/none) and
/// the style's own outline level (`sprmPOutLvl` in its PAPX UPX, 0–8 for
/// heading levels 1–9) — OOXML/.doc's language-independent heading marker.
/// A LibreOffice-converted legacy document names its heading styles in the
/// document language with `sti` "user", so the outline level is the only
/// signal left (docling#3961 / #270).
///
/// The same UPX carries the style's *list* (#641): Word attaches a numbered
/// heading's list to the Heading style (`sprmPIlfo` + `sprmPIlvl` there,
/// the paragraphs holding only `istd`), as `w:numPr` in a `.docx` style
/// does; `base` is the STD's `istdBase`, through which both inherit.
#[derive(Clone, Copy)]
struct StyleDef {
    sti: u16,
    outline: Option<u8>,
    ilfo: Option<u16>,
    ilvl: Option<u8>,
    /// `istdBase`: the style this one is based on; 0x0FFF = none.
    base: u16,
    /// The style's own `sprmCFBold` / `sprmCFItalic` operand (Word 6/95,
    /// #640): 0 off, 1 on, 0x80 the base style's, 0x81 its opposite;
    /// `None` = not set (the base style's). A run's 0x81 toggles *this*.
    bold: Option<u8>,
    italic: Option<u8>,
}

impl StyleDef {
    const NONE: StyleDef = StyleDef {
        sti: 0x0FFF,
        outline: None,
        ilfo: None,
        ilvl: None,
        base: 0x0FFF,
        bold: None,
        italic: None,
    };
}

/// Whether style `istd` is bold (or italic) once its based-on chain is
/// resolved — the value a run's "opposite of the style" operand (0x81)
/// flips against. The Word 97 reader never needed this: Word 97 writes
/// an absolute 1 for direct bold; Word 6/95 writes 0x81 on a run that
/// differs from its style, so a bold run in a plain style and a plain
/// run in a bold heading style carry the same byte (#640).
fn style_flag(stis: &[StyleDef], istd: u16, italic: bool) -> bool {
    let mut cur = Some(istd);
    let mut flip = false;
    for _ in 0..MAX_STYLE_CHAIN {
        let Some(style) = cur.and_then(|i| stis.get(i as usize)) else {
            break;
        };
        let operand = if italic { style.italic } else { style.bold };
        match operand {
            Some(0) => return flip,
            Some(1) => return !flip,
            Some(0x81) => flip = !flip,
            _ => {}
        }
        cur = (style.base != 0x0FFF && Some(style.base) != cur).then_some(style.base);
    }
    flip
}

/// Ancestors a based-on walk follows before giving up on a malformed or
/// cyclic chain — the DOCX backend's `MAX_STYLE_INHERITANCE_DEPTH`.
const MAX_STYLE_CHAIN: usize = 10;

/// The effective list reference of a paragraph, `(ilfo, ilvl)` — `None` when
/// it is in no list. The paragraph's own sprms win (an explicit `sprmPIlfo`
/// of 0 is "no list", like a `.docx` paragraph's `numId` 0); what the PAPX
/// leaves unset comes from the style, then its based-on chain, with `ilfo`
/// and `ilvl` inherited *independently* — the DOCX backend's
/// `style_numbering` (docling#3917), and what Word's own stock styles
/// need: "heading 2" may carry only its level and take the list from
/// "heading 1" (#641).
fn resolve_numbering(props: &ParaProps, stis: &[StyleDef]) -> Option<(u16, u8)> {
    let (mut ilfo, mut ilvl) = (props.ilfo, props.ilvl);
    let mut cur = Some(props.istd);
    let mut depth = 0;
    while let Some(istd) = cur {
        if (ilfo.is_some() && ilvl.is_some()) || depth >= MAX_STYLE_CHAIN {
            break;
        }
        let Some(style) = stis.get(istd as usize) else {
            break;
        };
        if ilfo.is_none() {
            ilfo = style.ilfo;
        }
        if ilvl.is_none() {
            ilvl = style.ilvl;
        }
        cur = (style.base != 0x0FFF && style.base != istd).then_some(style.base);
        depth += 1;
    }
    match ilfo? {
        0 => None,
        ilfo => Some((ilfo, ilvl.unwrap_or(0))),
    }
}

/// Whether a list level's number format (`nfc`, [MS-OSHARED] 2.2.1.3
/// `MSONFC`) prints a number a heading should carry — docling's
/// `_VISIBLE_NUMBERING_FORMATS` by its OOXML names (`_is_numbered_heading`,
/// docling#3760): decimal and roman/letter forms, `decimalZero`, and the
/// East Asian counting formats; a bullet (0x17), "none" (0xFF — Word's
/// invisible numbering that keeps a heading in the outline) and the spelled
/// out ordinal/cardinal words leave the heading text as is.
fn nfc_numbers_heading(nfc: u8) -> bool {
    matches!(
        nfc,
        0x00..=0x04 // arabic, upper/lower roman, upper/lower letter
            | 0x16 // decimalZero
            | 0x0A // ideographDigital
            | 0x0B // japaneseCounting
            | 0x0E // decimalFullWidth
            | 0x12 // decimalEnclosedCircle
            | 0x1E // ideographZodiac
            | 0x25 // chineseCounting
            | 0x26 // chineseLegalSimplified
            | 0x27 // chineseCountingThousand
    )
}

/// Parse the STSH into `istd → StyleDef`. A Word 6/95 stylesheet (#640)
/// has the same STSHI / STD framing; its style names are 8-bit Pascal
/// strings (a count byte, the characters, a null) instead of the
/// 2-byte-counted UTF-16 ones, and its UPX sprms are the pre-97 opcodes —
/// which carry no outline level or list reference, so only the built-in
/// `sti` identifies a heading there.
fn parse_stsh(stsh: &[u8], format: DocFormat) -> Vec<StyleDef> {
    let none = StyleDef::NONE;
    let Some(cb_stshi) = u16_at(stsh, 0) else {
        return Vec::new();
    };
    let Some(cstd) = u16_at(stsh, 2) else {
        return Vec::new();
    };
    // STSHI: cstd, then cbSTDBaseInFile — the size of each STD's fixed part,
    // which is where the style name (and after it, the UPX array) starts.
    let cb_std_base = u16_at(stsh, 4).unwrap_or(10) as usize;
    let mut styles = Vec::with_capacity(cstd as usize);
    let mut pos = 2 + cb_stshi as usize;
    for _ in 0..cstd {
        let Some(cb_std) = u16_at(stsh, pos) else {
            break;
        };
        pos += 2;
        // Empty slot: cbStd == 0.
        let mut def = none;
        if cb_std >= 2 {
            let std = stsh.get(pos..pos + cb_std as usize).unwrap_or(&[]);
            def.sti = u16_at(std, 0).map(|w| w & 0x0FFF).unwrap_or(0x0FFF);
            // STDF word 2: `sgc` in the low nibble, `istdBase` above it.
            let sgc = u16_at(std, 2).map(|w| w & 0x0F).unwrap_or(0);
            def.base = u16_at(std, 2).map(|w| w >> 4).unwrap_or(0x0FFF);
            // Paragraph styles (sgc 1) carry a PAPX UPX first: after the
            // fixed STDF part comes the 2-byte-counted UTF-16 name (plus its
            // null), then the 2-byte-aligned UPX array — UPX[0] is `istd +
            // grpprl`, the same sprm stream the paragraph walker reads.
            if sgc == 1 {
                match format {
                    DocFormat::Word97 => {
                        if let Some(cch) = u16_at(std, cb_std_base) {
                            let mut upx = cb_std_base + 2 + (cch as usize + 1) * 2;
                            upx += upx & 1;
                            if let Some(cb_upx) = u16_at(std, upx) {
                                let g = std.get(upx + 2..upx + 2 + cb_upx as usize).unwrap_or(&[]);
                                if g.len() >= 2 {
                                    let mut props = ParaProps::default();
                                    apply_pap_sprms(&g[2..], &mut props);
                                    def.outline = props.outline;
                                    def.ilfo = props.ilfo;
                                    def.ilvl = props.ilvl;
                                }
                            }
                        }
                    }
                    // Word 6: an 8-bit Pascal name, then UPX[0] (PAPX) and
                    // UPX[1] (CHPX) — the style's bold / italic operands.
                    DocFormat::Word6 => {
                        if let Some(&cch) = std.get(cb_std_base) {
                            let mut upx = cb_std_base + 1 + cch as usize + 1;
                            upx += upx & 1;
                            if let Some(cb_upx0) = u16_at(std, upx) {
                                let mut chpx = upx + 2 + cb_upx0 as usize;
                                chpx += chpx & 1;
                                if let Some(cb_upx1) = u16_at(std, chpx) {
                                    let g = std
                                        .get(chpx + 2..chpx + 2 + cb_upx1 as usize)
                                        .unwrap_or(&[]);
                                    walk_word6_sprms(g, |op, operand| match op {
                                        85 => def.bold = Some(operand[0]),
                                        86 => def.italic = Some(operand[0]),
                                        _ => {}
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
        styles.push(def);
        pos += cb_std as usize;
        // LPStd entries are 2-byte aligned.
        pos += pos & 1;
    }
    styles
}

/// Character formatting of one run (the subset the Markdown output shows),
/// plus the picture-data offset when the run is a picture anchor.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
struct CharFmt {
    bold: bool,
    italic: bool,
    /// Word 6/95 (#640): the run's bold / italic is the *opposite of its
    /// paragraph style's* (operand 0x81) or the style's own (0x80) —
    /// resolved against the style when the paragraph ends
    /// ([`ParaAccum::finish`]); `bold` / `italic` hold nothing until then.
    bold_from_style: Option<bool>,
    italic_from_style: Option<bool>,
    /// `sprmCPicLocation`: offset of the run's PICF in the Data stream (the
    /// run's `0x01` character is an inline-picture anchor).
    pic_fc: Option<u32>,
    /// `sprmCRgFtc0` / `sprmCRgFtc2`: the run's font (an index into the
    /// font table) for ASCII characters and for the "other" ones — the
    /// 0x80–0xFF byte codes and their PUA images (#588). `None` = the style's.
    ftc_ascii: Option<u16>,
    ftc_other: Option<u16>,
    /// `sprmCSymbol`: `(ftc, xchar)` — the font and glyph code the run's
    /// `0x28` placeholder stands for (an inserted symbol, Insert ▸ Symbol).
    /// Word also sets `sprmCFSpec` on such a run, but not in every CHPX of
    /// a document that shares them, so the placeholder alone is the cue.
    symbol: Option<(u16, u16)>,
}

/// FC → [`CharFmt`] through the PlcfBteChpx and CHPX FKPs, memoizing the last
/// run's FC range — consecutive characters nearly always share a run, so the
/// per-character lookup is amortized to a range check.
#[derive(Default)]
struct ChpxCache {
    lo: u64,
    hi: u64,
    fmt: CharFmt,
}

impl ChpxCache {
    fn props(&mut self, word: &[u8], btec: &[u8], fc: u64, format: DocFormat) -> CharFmt {
        if fc >= self.lo && fc < self.hi {
            return self.fmt;
        }
        let (fmt, lo, hi) = char_props(word, btec, fc, format);
        self.lo = lo;
        self.hi = hi;
        self.fmt = fmt;
        fmt
    }
}

/// Look up the CHPX for the character at `fc`: PlcfBteChpx → CHPX FKP page →
/// grpprl scan for `sprmCFBold`/`sprmCFItalic`. Returns the format and the FC
/// range it covers (for the cache).
fn char_props(word: &[u8], btec: &[u8], fc: u64, format: DocFormat) -> (CharFmt, u64, u64) {
    let fmt = CharFmt::default();
    let Some(pn) = bin_table_page(btec, fc, format) else {
        return (fmt, fc, fc + 1);
    };
    let page_off = (pn & 0x003F_FFFF) as usize * 512;
    let Some(page) = word.get(page_off..page_off + 512) else {
        return (fmt, fc, fc + 1);
    };
    let crun = page[511] as usize;
    if crun == 0 || (crun + 1) * 4 + crun > 511 {
        return (fmt, fc, fc + 1);
    }
    for j in 0..crun {
        let lo = u32_at(page, j * 4).unwrap_or(u32::MAX) as u64;
        let hi = u32_at(page, (j + 1) * 4).unwrap_or(0) as u64;
        if fc < lo || fc >= hi {
            continue;
        }
        // rgb: crun 1-byte word offsets after the FC array; 0 → no CHPX.
        let b = page[(crun + 1) * 4 + j] as usize;
        let mut out = CharFmt::default();
        if b != 0 {
            if let Some(&cb) = page.get(b * 2) {
                if let Some(grpprl) = page.get(b * 2 + 1..(b * 2 + 1 + cb as usize).min(512)) {
                    match format {
                        DocFormat::Word97 => apply_chp_sprms(grpprl, &mut out),
                        DocFormat::Word6 => apply_word6_chp_sprms(grpprl, &mut out),
                    }
                }
            }
        }
        return (out, lo, hi);
    }
    (fmt, fc, fc + 1)
}

/// Scan a CHP grpprl for bold/italic. Operand 1 = on, 0x81 = "opposite of the
/// style" — treated as on (the styles the Markdown output cares about are not
/// themselves bold/italic).
fn apply_chp_sprms(mut sprms: &[u8], fmt: &mut CharFmt) {
    while sprms.len() >= 2 {
        let sprm = u16::from_le_bytes([sprms[0], sprms[1]]);
        sprms = &sprms[2..];
        let operand_len = match sprm >> 13 {
            0 | 1 => 1,
            2 | 4 | 5 => 2,
            3 => 4,
            7 => 3,
            _ => match sprms.first() {
                Some(&cb) => 1 + cb as usize,
                None => return,
            },
        };
        if sprms.len() < operand_len {
            return;
        }
        match sprm {
            0x0835 => fmt.bold = sprms[0] == 1 || sprms[0] == 0x81, // sprmCFBold
            0x0836 => fmt.italic = sprms[0] == 1 || sprms[0] == 0x81, // sprmCFItalic
            0x4A4F => fmt.ftc_ascii = Some(u16::from_le_bytes([sprms[0], sprms[1]])), // sprmCRgFtc0
            0x4A51 => fmt.ftc_other = Some(u16::from_le_bytes([sprms[0], sprms[1]])), // sprmCRgFtc2
            // sprmCSymbol: the font and the glyph code of an inserted symbol.
            0x6A09 => {
                fmt.symbol = Some((
                    u16::from_le_bytes([sprms[0], sprms[1]]),
                    u16::from_le_bytes([sprms[2], sprms[3]]),
                ))
            }
            // sprmCPicLocation: PICF offset in the Data stream.
            0x6A03 => {
                fmt.pic_fc = Some(u32::from_le_bytes([sprms[0], sprms[1], sprms[2], sprms[3]]))
            }
            _ => {}
        }
        sprms = &sprms[operand_len..];
    }
}

/// Decode an inline picture: PICF at `pic_fc` in the Data stream — a header
/// of `cbHeader` bytes (with the total size in `lcb`), then the OfficeArt
/// record tree holding the BLIP. `MM_SHAPEFILE` (0x66) interposes a
/// length-prefixed picture name.
fn inline_picture(data: &[u8], pic_fc: u32) -> Option<PictureImage> {
    let base = pic_fc as usize;
    let lcb = u32::from_le_bytes(data.get(base..base + 4)?.try_into().ok()?) as usize;
    let cb_header = u16::from_le_bytes(data.get(base + 4..base + 6)?.try_into().ok()?) as usize;
    let mm = u16::from_le_bytes(data.get(base + 6..base + 8)?.try_into().ok()?);
    let mut start = base + cb_header;
    if mm == 0x0066 {
        // MM_SHAPEFILE: cchPicName + name precede the OfficeArt data.
        let cch = *data.get(start)? as usize;
        start += 1 + cch;
    }
    let body = data.get(start..base + lcb.max(cb_header))?;
    officeart::first_blip(body, 0)
}

/// The font table ([MS-DOC] 2.9.272 `SttbfFfn`, #588): an STTB whose
/// strings are `FFN` records — `cData` entries, each led by its byte count,
/// which is the FFN's own `cbFfnM1`; the 39 bytes of metrics that follow
/// (flags, `wWeight`, `chs`, `ixchSzAlt`, `panose`, `fs`) precede the
/// null-terminated UTF-16 face name (`xszFfn`). Names by `ftc`, the index
/// the character sprms use; an unreadable table is empty (the text then
/// prints as before).
fn parse_sttbf_ffn(sttb: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    let Some(&c_data) = u16_at(sttb, 0).as_ref() else {
        return names;
    };
    // An extended STTB (`0xFFFF` marker) is not how Word writes this table,
    // but read its header the same way rather than mis-parse it.
    let (c_data, mut pos) = if c_data == 0xFFFF {
        (u16_at(sttb, 2).unwrap_or(0) as usize, 6)
    } else {
        (c_data as usize, 4)
    };
    let cb_extra = u16_at(sttb, pos - 2).unwrap_or(0) as usize;
    for _ in 0..c_data {
        let Some(&cch) = sttb.get(pos) else { break };
        let Some(ffn) = sttb.get(pos + 1..pos + 1 + cch as usize) else {
            break;
        };
        let name: String = ffn
            .get(39..)
            .unwrap_or(&[])
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&u| u != 0)
            .map(|u| char::from_u32(u as u32).unwrap_or('\u{FFFD}'))
            .collect();
        names.push(name);
        pos += 1 + cch as usize + cb_extra;
    }
    names
}

/// Parse the PlcfSpa: floating-shape anchors as `(anchor CP, spid)`.
fn parse_plcf_spa(plc: &[u8]) -> Vec<(u64, u32)> {
    // PLC with 26-byte SPA data: n = (lcb - 4) / 30.
    let Some(n) = plc.len().checked_sub(4).map(|l| l / 30) else {
        return Vec::new();
    };
    (0..n)
        .filter_map(|i| {
            let cp = u32_at(plc, i * 4)? as u64;
            let spid = u32_at(plc, (n + 1) * 4 + i * 26)?;
            Some((cp, spid))
        })
        .collect()
}

/// The spid anchored exactly at `cp`, if any.
fn spa_shape_at(spa: &[(u64, u32)], cp: u64) -> Option<u32> {
    spa.iter()
        .find(|(acp, _)| *acp == cp)
        .map(|(_, spid)| *spid)
}

/// The document's OfficeArt drawing tables: shape id → BLIP index (`pib`),
/// and the BLIP store — each entry either embeds its BLIP record or points
/// at one in the WordDocument (delay) stream via `foDelay`.
#[derive(Default)]
struct Drawings {
    /// spid → 1-based pib.
    shape_pib: std::collections::HashMap<u32, u32>,
    /// Group-frame spid → member spids (an anchored group shows every
    /// member's picture).
    groups: std::collections::HashMap<u32, Vec<u32>>,
    /// BStore order: decoded pictures (delay-stream ones resolved eagerly).
    blips: Vec<Option<PictureImage>>,
}

impl Drawings {
    fn parse(dgg: &[u8], delay_stream: &[u8]) -> Self {
        let mut out = Self {
            shape_pib: std::collections::HashMap::new(),
            groups: std::collections::HashMap::new(),
            blips: Vec::new(),
        };
        // The OfficeArtContent in a Word file is NOT a plain record stream:
        // each per-drawing DgContainer is preceded by a raw byte
        // (OfficeArtWordDrawing.dgglbl), which would desynchronize a straight
        // record walk. Parse top-level records with a resync: a position that
        // doesn't hold a well-formed OfficeArt header (0xF0xx type, in-bounds
        // length) skips one byte.
        let mut pos = 0usize;
        while pos + 8 <= dgg.len() {
            let rec_type = u16::from_le_bytes([dgg[pos + 2], dgg[pos + 3]]);
            let len = u32::from_le_bytes([dgg[pos + 4], dgg[pos + 5], dgg[pos + 6], dgg[pos + 7]])
                as usize;
            if (rec_type & 0xFF00) == 0xF000 && pos + 8 + len <= dgg.len() {
                out.walk(&dgg[pos..pos + 8 + len], delay_stream, 0);
                pos += 8 + len;
            } else {
                pos += 1;
            }
        }
        out
    }

    /// spid of an SpContainer body (its FSP record).
    fn sp_spid(sp_body: &[u8]) -> Option<u32> {
        officeart::Records::new(sp_body)
            .find(|(h, _)| h.rec_type == 0xF00A)
            .and_then(|(_, b)| {
                b.get(..4)
                    .map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]))
            })
    }

    fn walk(&mut self, body: &[u8], delay: &[u8], depth: usize) {
        if depth > 16 {
            return;
        }
        for (h, b) in officeart::Records::new(body) {
            match h.rec_type {
                // OfficeArtFBSE: BLIP store entry — embedded record or foDelay.
                0xF007 => {
                    let embedded = b.get(36..).and_then(|tail| officeart::first_blip(tail, 0));
                    let img = embedded.or_else(|| {
                        let fo = u32::from_le_bytes(b.get(28..32)?.try_into().ok()?) as usize;
                        let rec = delay.get(fo..)?;
                        officeart::Records::new(rec)
                            .next()
                            .filter(|(rh, _)| officeart::is_blip(rh.rec_type))
                            .and_then(|(rh, rb)| officeart::decode_blip(&rh, rb))
                    });
                    self.blips.push(img);
                }
                // A group: the frame shape's spid maps to the member spids.
                0xF003 => {
                    let mut frame = None;
                    let mut members = Vec::new();
                    for (h2, b2) in officeart::Records::new(b) {
                        match h2.rec_type {
                            0xF004 => {
                                let spid = Self::sp_spid(b2);
                                let is_frame = officeart::Records::new(b2)
                                    .any(|(h3, _)| h3.rec_type == 0xF009);
                                match (is_frame, frame) {
                                    (true, None) => frame = spid,
                                    _ => members.extend(spid),
                                }
                            }
                            0xF003 => {
                                // Nested group: its frame spid joins as a member.
                                let nested_frame = officeart::Records::new(b2)
                                    .find(|(h3, _)| h3.rec_type == 0xF004)
                                    .and_then(|(_, b3)| Self::sp_spid(b3));
                                members.extend(nested_frame);
                            }
                            _ => {}
                        }
                    }
                    if let Some(frame) = frame {
                        self.groups.insert(frame, members);
                    }
                    // Fall through to the generic recursion below for pib/blip
                    // collection inside the group.
                    self.walk(b, delay, depth + 1);
                }
                // OfficeArtSpContainer: read the FSP spid + FOPT pib property.
                0xF004 => {
                    let mut spid = None;
                    let mut pib = None;
                    for (h2, b2) in officeart::Records::new(b) {
                        match h2.rec_type {
                            0xF00A => {
                                spid = b2
                                    .get(..4)
                                    .map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]));
                            }
                            0xF00B => {
                                // Fixed 6-byte property entries; id bits 0–13.
                                for e in 0..h2.instance as usize {
                                    let o = e * 6;
                                    let Some(entry) = b2.get(o..o + 6) else { break };
                                    let id = u16::from_le_bytes([entry[0], entry[1]]) & 0x3FFF;
                                    if id == 260 {
                                        pib = Some(u32::from_le_bytes([
                                            entry[2], entry[3], entry[4], entry[5],
                                        ]));
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    if let (Some(spid), Some(pib)) = (spid, pib) {
                        self.shape_pib.insert(spid, pib);
                    }
                }
                _ if h.version == 0xF => self.walk(b, delay, depth + 1),
                _ => {}
            }
        }
    }

    /// The pictures behind a shape id: a picture shape yields one; a group
    /// yields each member's, in order. `None` entries are undecodable images
    /// (still emitted as placeholders).
    fn shape_pictures(&self, spid: u32, depth: usize) -> Vec<Option<PictureImage>> {
        if depth > 8 {
            return Vec::new();
        }
        if let Some(pib) = self.shape_pib.get(&spid) {
            let img = pib
                .checked_sub(1)
                .and_then(|ix| self.blips.get(ix as usize))
                .cloned()
                .flatten();
            return vec![img];
        }
        match self.groups.get(&spid) {
            Some(members) => members
                .iter()
                .flat_map(|&m| self.shape_pictures(m, depth + 1))
                .collect(),
            None => Vec::new(),
        }
    }
}

/// One piece of a level's number template (`xst`): a literal, or a
/// placeholder for another level's counter (placeholder chars are the level
/// index 0–8 in the raw text).
#[derive(Clone)]
enum LvlPart {
    Level(u8),
    Text(String),
}

/// One list level's numbering: kind, start value, and number template.
#[derive(Clone, Default)]
struct LvlInfo {
    /// Number format code (`nfc`): 0x17 = bullet, 0xFF = none, else numbered.
    nfc: u8,
    start: u32,
    /// The `xst` template (e.g. `1.1.` as [Level(0), ".", Level(1), "."]).
    template: Vec<LvlPart>,
}

/// The document's list tables: `ilfo` (1-based, from `sprmPIlfo`) resolves
/// through the LFO array to a list (`lsid`) and its per-level numbering.
#[derive(Default, Clone)]
struct ListTables {
    /// LFO index (0-based) → lsid.
    lfo_lsids: Vec<u32>,
    /// lsid → per-level info (1 entry for simple lists, 9 otherwise).
    lists: std::collections::HashMap<u32, Vec<LvlInfo>>,
}

impl ListTables {
    /// Parse the PlfLst (`lst_tail` starts at `fcPlfLst`; `lcb_lst` covers the
    /// LSTF array, and the per-list LVLs follow it in the stream) and the
    /// PlfLfo (LFO array). Any malformed structure yields what was parsed so
    /// far — unresolvable items degrade to bullets, never to an error.
    fn parse(lst_tail: &[u8], lcb_lst: usize, plflfo: &[u8]) -> Self {
        let plflst = lst_tail;
        let mut out = Self::default();
        // PlfLst: cLst u16, then cLst LSTFs of 28 bytes, then each list's LVLs.
        let c_lst = u16_at(plflst, 0).unwrap_or(0) as usize;
        let mut lstfs = Vec::with_capacity(c_lst);
        for i in 0..c_lst {
            let base = 2 + i * 28;
            let Some(lsid) = u32_at(plflst, base) else {
                break;
            };
            let simple = plflst.get(base + 26).is_some_and(|&f| f & 0x01 != 0);
            lstfs.push((lsid, if simple { 1usize } else { 9usize }));
        }
        let mut pos = (2 + c_lst * 28).max(lcb_lst);
        'lists: for (lsid, nlvl) in lstfs {
            let mut lvls = Vec::with_capacity(nlvl);
            for _ in 0..nlvl {
                // LVL = LVLF (28 bytes) + grpprlPapx + grpprlChpx + xst.
                let Some(start) = u32_at(plflst, pos) else {
                    break 'lists;
                };
                let Some(&nfc) = plflst.get(pos + 4) else {
                    break 'lists;
                };
                let cb_chpx = plflst.get(pos + 24).copied().unwrap_or(0) as usize;
                let cb_papx = plflst.get(pos + 25).copied().unwrap_or(0) as usize;
                pos += 28 + cb_papx + cb_chpx;
                let cch = u16_at(plflst, pos).unwrap_or(0) as usize;
                pos += 2;
                // xst: UTF-16 template where a code ≤ 8 is a placeholder for
                // that level's counter (`1.1.` = [0, '.', 1, '.']).
                let mut template: Vec<LvlPart> = Vec::new();
                for i in 0..cch {
                    let Some(u) = u16_at(plflst, pos + i * 2) else {
                        break;
                    };
                    if u <= 8 {
                        template.push(LvlPart::Level(u as u8));
                    } else if let Some(c) = char::from_u32(u as u32) {
                        match template.last_mut() {
                            Some(LvlPart::Text(t)) => t.push(c),
                            _ => template.push(LvlPart::Text(c.to_string())),
                        }
                    }
                }
                pos += cch * 2;
                lvls.push(LvlInfo {
                    nfc,
                    start,
                    template,
                });
            }
            out.lists.insert(lsid, lvls);
        }
        // PlfLfo: lfoMac u32, then lfoMac LFOs of 16 bytes (lsid first).
        let lfo_mac = u32_at(plflfo, 0).unwrap_or(0) as usize;
        for i in 0..lfo_mac {
            match u32_at(plflfo, 4 + i * 16) {
                Some(lsid) => out.lfo_lsids.push(lsid),
                None => break,
            }
        }
        out
    }

    /// Numbering info for `(ilfo, ilvl)`, when the tables resolve it.
    fn level(&self, ilfo: u16, ilvl: u8) -> Option<LvlInfo> {
        let lsid = *self.lfo_lsids.get(ilfo.checked_sub(1)? as usize)?;
        let lvls = self.lists.get(&lsid)?;
        lvls.get(ilvl as usize).or_else(|| lvls.first()).cloned()
    }

    /// A level's start value, for counters not yet touched in a run.
    fn level_start(&self, ilfo: u16, ilvl: u8) -> u64 {
        self.level(ilfo, ilvl).map(|l| l.start as u64).unwrap_or(1)
    }
}

/// Accumulates one paragraph's characters as `(text, format)` segments, then
/// classifies the paragraph on its mark.
#[derive(Default)]
struct ParaAccum {
    /// Consecutive same-format runs of the paragraph.
    segments: Vec<(String, CharFmt)>,
    /// Pictures anchored in this paragraph: `(before any text, image)`.
    pictures: Vec<(bool, Option<PictureImage>)>,
    /// Result-text state of any field (`0x13 code 0x14 result 0x15`) stack:
    /// characters inside the *code* part are dropped.
    field_stack: Vec<bool>, // true = in result part
    /// Text boxes anchored in this paragraph, each its story's nodes (#535).
    textboxes: Vec<Vec<Node>>,
}

impl ParaAccum {
    /// Queue a picture anchored at the current position (`None` = a picture
    /// whose bytes couldn't be decoded — still a placeholder node).
    fn add_picture(&mut self, image: Option<PictureImage>) {
        let before_text = self.segments.iter().all(|(t, _)| t.trim().is_empty());
        self.pictures.push((before_text, image));
    }

    fn push(&mut self, ch: char, fmt: CharFmt) {
        match ch {
            '\u{0013}' => self.field_stack.push(false),
            '\u{0014}' => {
                if let Some(top) = self.field_stack.last_mut() {
                    *top = true;
                }
            }
            '\u{0015}' => {
                self.field_stack.pop();
            }
            // Object/drawing/note anchors and other control marks: dropped.
            '\u{0001}' | '\u{0002}' | '\u{0005}' | '\u{0008}' => {}
            '\u{000B}' => self.keep('\n', fmt), // hard line break
            '\u{001E}' => self.keep('-', fmt),  // non-breaking hyphen
            '\u{001F}' => {}                    // soft hyphen
            _ => self.keep(ch, fmt),
        }
    }

    fn keep(&mut self, ch: char, fmt: CharFmt) {
        if !self.field_stack.iter().all(|&r| r) {
            return;
        }
        match self.segments.last_mut() {
            Some((text, last)) if *last == fmt => text.push(ch),
            _ => self.segments.push((ch.to_string(), fmt)),
        }
    }

    /// Flat text, formatting ignored (headings, table-structure decisions).
    fn plain(&self) -> String {
        self.segments.iter().map(|(t, _)| t.as_str()).collect()
    }

    /// Markdown text with `**bold**` / `*italic*` markers per run, whitespace
    /// kept outside the markers (matching the DOCX backend's rendering). Not
    /// yet escaped: [`NodeBuilder::paragraph`] escapes prose and keeps table
    /// cells raw.
    fn markdown(&self) -> String {
        let mut out = String::new();
        for (text, fmt) in &self.segments {
            if !fmt.bold && !fmt.italic {
                out.push_str(text);
                continue;
            }
            let core = text.trim();
            if core.is_empty() {
                out.push_str(text);
                continue;
            }
            let lead = &text[..text.len() - text.trim_start().len()];
            let trail = &text[text.trim_end().len()..];
            let marker = match (fmt.bold, fmt.italic) {
                (true, true) => "***",
                (true, false) => "**",
                (false, true) => "*",
                _ => unreachable!(),
            };
            out.push_str(lead);
            out.push_str(marker);
            out.push_str(core);
            out.push_str(marker);
            out.push_str(trail);
        }
        out
    }

    fn finish(
        &mut self,
        mark: char,
        props: ParaProps,
        stis: &[StyleDef],
        builder: &mut NodeBuilder,
        doc: &mut DoclingDocument,
    ) {
        // Word 6/95 runs formatted relative to the style (#640): now that
        // the paragraph's style is known, settle them.
        for (_, fmt) in self.segments.iter_mut() {
            if let Some(flip) = fmt.bold_from_style.take() {
                fmt.bold = style_flag(stis, props.istd, false) != flip;
            }
            if let Some(flip) = fmt.italic_from_style.take() {
                fmt.italic = style_flag(stis, props.istd, true) != flip;
            }
        }
        let plain = self.plain();
        let markdown = self.markdown();
        let pictures = std::mem::take(&mut self.pictures);
        let textboxes = std::mem::take(&mut self.textboxes);
        self.segments.clear();
        self.field_stack.clear();
        builder.paragraph(plain, markdown, pictures, textboxes, mark, props, stis, doc);
    }
}

/// Turns the classified paragraph stream into nodes, assembling table runs.
#[derive(Default)]
struct NodeBuilder {
    /// In-progress table rows, current row's cells, and the current cell's
    /// accumulated paragraphs (a cell may span several).
    rows: Vec<Vec<String>>,
    cells: Vec<String>,
    cell_text: String,
    /// The `ilfo` of the most recent list item — a new item starts a new list
    /// (`first_in_list`) when its `ilfo` differs. Empty paragraphs between
    /// items do NOT break a list (docling's DOCX behavior: numbering and
    /// grouping continue across gaps); any other node kind does.
    last_ilfo: Option<u16>,
    /// Base `ilvl` of the current contiguous list run (its first item's),
    /// mirroring the DOCX backend's `list_run_base`.
    run_base: Option<u8>,
    lists: ListTables,
    /// Running list counters, keyed by `(ilfo, ilvl)` — docling semantics:
    /// the value is the *current* number (post-increment), deeper levels
    /// zero on a shallower item.
    counters: std::collections::HashMap<(u16, u8), u64>,
    /// Heading level → running number of the numbered headings (#641), the
    /// DOCX backend's `numbered_headers`.
    numbered_headers: std::collections::HashMap<u8, u64>,
}

impl NodeBuilder {
    fn new(lists: ListTables) -> Self {
        Self {
            lists,
            ..Self::default()
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn paragraph(
        &mut self,
        plain: String,
        markdown: String,
        pictures: Vec<(bool, Option<PictureImage>)>,
        textboxes: Vec<Vec<Node>>,
        mark: char,
        props: ParaProps,
        stis: &[StyleDef],
        doc: &mut DoclingDocument,
    ) {
        if props.ttp {
            // Row terminator: close the row.
            if !self.cell_text.is_empty() {
                self.cells.push(std::mem::take(&mut self.cell_text));
            }
            if !self.cells.is_empty() {
                self.rows.push(std::mem::take(&mut self.cells));
            }
            return;
        }
        if props.in_table {
            let text = markdown.trim_end_matches('\u{0007}');
            // Multi-paragraph cells join with a blank line, matching the
            // DOCX backend's rich cells (the Markdown table serializer
            // then folds each newline into a space); an empty paragraph
            // adds nothing, as the DOCX backend's cell text skips it (#640
            // — a Word 6 cell with blank lines between its paragraphs).
            if !self.cell_text.is_empty() && !text.trim().is_empty() {
                self.cell_text.push_str("\n\n");
            }
            self.cell_text.push_str(text);
            // A text box anchored in the cell: its paragraphs are more of the
            // cell's (#535), as the DOCX backend reads the same document.
            for text in textboxes.iter().flatten().filter_map(node_text) {
                if !self.cell_text.is_empty() {
                    self.cell_text.push_str("\n\n");
                }
                self.cell_text.push_str(&text);
            }
            if mark == '\u{0007}' {
                self.cells.push(std::mem::take(&mut self.cell_text));
            }
            return;
        }
        self.flush(doc);
        // Text boxes anchored here (#535): a `textbox` section group each, in
        // front of the anchoring paragraph — the DOCX backend's shape.
        for children in textboxes {
            doc.push(Node::Group {
                label: "section".into(),
                name: Some("textbox".into()),
                layer: None,
                children,
            });
            self.last_ilfo = None;
            self.run_base = None;
        }

        let picture_node = |image: Option<PictureImage>| Node::Picture {
            caption: None,
            caption_href: None,
            image,
            classification: None,
            description: None,
            caption_parent: Default::default(),
            caption_location: None,
        };
        let plain = plain.trim().to_string();
        // Prose is Markdown-escaped like every other backend's text nodes
        // (docling-core's `serialize_run`: `_` → `\_`, then `& < >`), #567 —
        // `OBJ_DIR` otherwise reads as the start of an emphasis span and the
        // JSON export's `unescape_text` restores the raw text. Table cells
        // stay raw: docling's table serializer prints the cell text as is
        // (the DOCX backend's cells are unescaped too, so the mirrored
        // `docx_rich_tables_01` fixtures keep matching).
        let text = escape_text(markdown.trim());
        if plain.is_empty() {
            // Pictures anchored in an otherwise-empty paragraph are blocks of
            // their own. A blank paragraph does not break a list run
            // (docling's DOCX behavior: items continue across gaps).
            for (_, image) in pictures {
                doc.push(picture_node(image));
            }
            return;
        }
        // Anchored pictures surround the paragraph text by anchor position.
        for (_, image) in pictures.iter().filter(|(before, _)| *before) {
            doc.push(picture_node(image.clone()));
        }
        let after: Vec<_> = pictures
            .into_iter()
            .filter(|(before, _)| !before)
            .map(|(_, image)| image)
            .collect();
        let style = stis
            .get(props.istd as usize)
            .copied()
            .unwrap_or(StyleDef::NONE);
        // The paragraph's list, its own or its style's (#641).
        let numbering = resolve_numbering(&props, stis);
        let sti = style.sti;
        // A style that is not one of the built-in Heading/Title styles can
        // still be a heading through its outline level (docling#3961 / #270):
        // LibreOffice-converted legacy documents carry localized user styles
        // ("Επικεφαλίδα 2") whose only heading marker is `sprmPOutLvl`. 0-8
        // are heading levels 1-9; 9 is the "body text" sentinel.
        let outline_heading = (!(1..=9).contains(&sti) && sti != 62)
            .then_some(style.outline)
            .flatten()
            .filter(|l| *l <= 8);
        if (1..=9).contains(&sti) || sti == 62 || outline_heading.is_some() {
            // Mirrors the DOCX backend: docling renders "heading N" at
            // Markdown level N+1 and Title (sti 62) at level 1.
            let level = if sti == 62 {
                1
            } else if let Some(l) = outline_heading {
                l + 2
            } else {
                sti as u8 + 1
            };
            // A heading style wins over a list: a numbered heading gets a
            // computed `1.2` prefix rather than becoming a list item — the
            // DOCX backend's rule, and docling's `_is_numbered_heading`
            // (docling#3760): only a level whose number format is *visible*
            // numbers it, so Word's "none"-format outline numbering leaves
            // the text alone. The number is docling's own per-level heading
            // counter, not the list template, as the `.docx` twin prints it
            // (`## 1 Introduction`, `### 1.1 Context`), #641.
            let visibly_numbered = numbering
                .and_then(|(ilfo, ilvl)| self.lists.level(ilfo, ilvl))
                .is_some_and(|l| nfc_numbers_heading(l.nfc));
            let plain = if visibly_numbered && !plain.is_empty() {
                let docling_level = level.saturating_sub(1).max(1);
                super::docx::numbered_heading_text(
                    &mut self.numbered_headers,
                    docling_level,
                    &plain,
                )
            } else {
                plain
            };
            // Headings render without run markers (the style carries the look).
            doc.push(Node::Heading {
                level,
                text: escape_text(&plain),
            });
            self.last_ilfo = None;
            self.run_base = None;
        } else if let Some((ilfo, ilvl)) = numbering {
            let lvl = self.lists.level(ilfo, ilvl);
            // Bullet (0x17) / no-number (0xFF) levels — and unresolvable
            // references — are unordered; everything else numbers.
            let numbered = lvl.as_ref().is_some_and(|l| l.nfc != 0x17 && l.nfc != 0xFF);
            let first_in_list = self.last_ilfo != Some(ilfo);
            // The run's base indent (its first item's ilvl) — nested levels
            // indent relative to it, matching the DOCX backend's
            // `list_run_base` (the same list restarted after body text can
            // legitimately sit at a different Markdown depth).
            let base = *self.run_base.get_or_insert(ilvl);
            let level = ilvl.saturating_sub(base);
            if numbered {
                // docling's counter semantics: bump this level (from the
                // level's own start), zero deeper levels of the same list.
                let start = self.lists.level_start(ilfo, ilvl);
                let c = self
                    .counters
                    .entry((ilfo, ilvl))
                    .or_insert(start.saturating_sub(1));
                *c += 1;
                for ((f, l), v) in self.counters.iter_mut() {
                    if *f == ilfo && *l > ilvl {
                        *v = 0;
                    }
                }
                let marker = self.build_marker(ilfo, ilvl, lvl.as_ref());
                let number = marker
                    .trim_end_matches(['.', ')'])
                    .rsplit(['.', ')'])
                    .next()
                    .and_then(|s| s.trim().parse::<u64>().ok())
                    .unwrap_or(1);
                if cached_regex!(r"^\d+[.)]$").is_match(&marker) {
                    // A plain `N.` marker: ordered in Markdown and DocLang.
                    doc.push(Node::ListItem {
                        ordered: true,
                        number,
                        first_in_list,
                        text,
                        level,
                        marker: Some(marker),
                        location: None,
                        dclx: None,
                        href: None,
                        layer: None,
                    });
                } else {
                    // A multilevel marker (`1.1.`): Markdown bullet with the
                    // marker as a text prefix, ordered DocLang item with a
                    // clean-text `<marker>` — same as the DOCX backend.
                    let dclx = Some(docling_core::ListItemDclx {
                        ordered: true,
                        marker: Some(marker.clone()),
                        text: text.clone(),
                        runs: Vec::new(),
                    });
                    doc.push(Node::ListItem {
                        ordered: false,
                        number,
                        first_in_list,
                        text: format!("{marker} {text}"),
                        level,
                        marker: None,
                        location: None,
                        dclx,
                        href: None,
                        layer: None,
                    });
                }
            } else {
                doc.push(Node::ListItem {
                    ordered: false,
                    number: 0,
                    first_in_list,
                    text,
                    level,
                    marker: None,
                    location: None,
                    dclx: None,
                    href: None,
                    layer: None,
                });
            }
            self.last_ilfo = Some(ilfo);
        } else {
            doc.push(Node::Paragraph { text });
            self.last_ilfo = None;
            self.run_base = None;
        }
        for image in after {
            doc.push(picture_node(image));
        }
    }

    /// Emit any table under construction.
    fn flush(&mut self, doc: &mut DoclingDocument) {
        if !self.cell_text.is_empty() {
            self.cells.push(std::mem::take(&mut self.cell_text));
        }
        if !self.cells.is_empty() {
            self.rows.push(std::mem::take(&mut self.cells));
        }
        if self.rows.is_empty() {
            return;
        }
        let rows = std::mem::take(&mut self.rows);
        // Rectangularize (rows may have ragged cell counts).
        let width = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        let rows: Vec<Vec<String>> = rows
            .into_iter()
            .map(|mut r| {
                r.resize(width, String::new());
                r
            })
            .collect();
        doc.push(Node::Table(Table {
            rows,
            location: None,
            structure: None,
            cell_blocks: None,
            cells: None,
            caption: None,
            caption_parent: Default::default(),
            caption_location: None,
        }));
        self.last_ilfo = None;
        self.run_base = None;
    }

    /// Build the item's enumeration marker from the level's `xst` template —
    /// the DOC twin of the DOCX backend's `build_enum_marker`: a template
    /// with literal text beyond separators substitutes its placeholders;
    /// a bare numeric template falls back to the hierarchical `1.2.` form
    /// joining the counters of levels `0..=ilvl`.
    fn build_marker(&self, ilfo: u16, ilvl: u8, lvl: Option<&LvlInfo>) -> String {
        let counter_at = |l: u8| -> u64 {
            self.counters
                .get(&(ilfo, l))
                .copied()
                .unwrap_or_else(|| self.lists.level_start(ilfo, l))
        };
        if let Some(lvl) = lvl {
            let literal: String = lvl
                .template
                .iter()
                .filter_map(|p| match p {
                    LvlPart::Text(t) => Some(t.as_str()),
                    LvlPart::Level(_) => None,
                })
                .collect();
            let has_levels = lvl.template.iter().any(|p| matches!(p, LvlPart::Level(_)));
            if has_levels
                && !literal
                    .trim_matches(|c: char| " .)(:[]".contains(c))
                    .is_empty()
            {
                return lvl
                    .template
                    .iter()
                    .map(|p| match p {
                        LvlPart::Level(l) => counter_at(*l).to_string(),
                        LvlPart::Text(t) => t.clone(),
                    })
                    .collect();
            }
        }
        let parts: Vec<String> = (0..=ilvl).map(|l| counter_at(l).to_string()).collect();
        parts.join(".") + "."
    }
}

/// A text box node's text, for placing it in a table cell: a paragraph,
/// heading or list item's text, a table's cells.
fn node_text(node: &Node) -> Option<String> {
    let text = match node {
        Node::Paragraph { text } | Node::Heading { text, .. } | Node::ListItem { text, .. } => {
            text.clone()
        }
        Node::Table(t) => t
            .rows
            .iter()
            .flatten()
            .filter(|c| !c.trim().is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" "),
        Node::Group { children, .. } => children
            .iter()
            .filter_map(node_text)
            .collect::<Vec<_>>()
            .join(" "),
        _ => return None,
    };
    (!text.trim().is_empty()).then_some(text)
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?))
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

/// Windows-1252 to Unicode (the 0x80–0x9F block differs from Latin-1).
pub(crate) fn cp1252(b: u8) -> char {
    match b {
        0x80 => '€',
        0x82 => '‚',
        0x83 => 'ƒ',
        0x84 => '„',
        0x85 => '…',
        0x86 => '†',
        0x87 => '‡',
        0x88 => 'ˆ',
        0x89 => '‰',
        0x8A => 'Š',
        0x8B => '‹',
        0x8C => 'Œ',
        0x8E => 'Ž',
        0x91 => '\u{2018}',
        0x92 => '\u{2019}',
        0x93 => '\u{201C}',
        0x94 => '\u{201D}',
        0x95 => '•',
        0x96 => '–',
        0x97 => '—',
        0x98 => '˜',
        0x99 => '™',
        0x9A => 'š',
        0x9B => '›',
        0x9C => 'œ',
        0x9E => 'ž',
        0x9F => 'Ÿ',
        other => other as char,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Word 97 FIB of the usual shape (`csw` 14, `cslw` 22) with
    /// `cbRgFcLcb` pairs, `ccpText` = 1234 and pair 33 (the CLX) = (7, 9).
    fn fib_bytes(cb: u16) -> Vec<u8> {
        let mut w = vec![0u8; 32];
        w[0..2].copy_from_slice(&0xA5ECu16.to_le_bytes());
        w.extend_from_slice(&14u16.to_le_bytes());
        w.extend(std::iter::repeat_n(0, 28));
        w.extend_from_slice(&22u16.to_le_bytes());
        let mut lw = vec![0u8; 88];
        lw[12..16].copy_from_slice(&1234u32.to_le_bytes());
        w.extend(lw);
        w.extend_from_slice(&cb.to_le_bytes());
        let mut blob = vec![0u8; cb as usize * 8];
        if cb > 33 {
            blob[264..268].copy_from_slice(&7u32.to_le_bytes());
            blob[268..272].copy_from_slice(&9u32.to_le_bytes());
        }
        w.extend(blob);
        w
    }

    #[test]
    fn fib_reads_through_the_declared_counts() {
        let w = fib_bytes(0x5D);
        let fib = Fib::parse(&w).expect("well-formed FIB");
        assert_eq!(fib.ccp_text, 1234);
        assert_eq!(fib.fc_lcb(Fib::CLX), (7, 9));
        // The CLX pair sits where the old fixed offsets (418/422) put it.
        assert_eq!(u32_at(&w, 418), Some(7));
        assert_eq!(u32_at(&w, 422), Some(9));
        // A pair past the writer's cbRgFcLcb reads as absent.
        let short = fib_bytes(20);
        let fib = Fib::parse(&short).unwrap();
        assert_eq!(fib.fc_lcb(Fib::CLX), (0, 0));
        assert!(fib.part(&[1, 2, 3], Fib::CLX).is_empty());
    }

    #[test]
    fn truncated_fib_is_an_error_naming_the_field() {
        let w = fib_bytes(0x5D);
        for (cut, field) in [
            (20, "csw"),
            (40, "cslw"),
            (70, "ccpText"),
            (153, "cbRgFcLcb"),
            (300, "fibRgFcLcbBlob"),
        ] {
            let err = Fib::parse(&w[..cut])
                .err()
                .expect("truncated FIB must fail");
            assert!(err.to_string().contains(field), "cut {cut}: {err}");
        }
    }

    #[test]
    fn part_survives_an_overflowing_range() {
        let mut w = fib_bytes(0x5D);
        // fcClx = u32::MAX, lcbClx = u32::MAX: no panic, just absent.
        w[154 + 264..154 + 272].copy_from_slice(&[0xFF; 8]);
        let fib = Fib::parse(&w).unwrap();
        assert!(fib.part(&[0u8; 16], Fib::CLX).is_empty());
    }

    #[test]
    fn unsupported_word_version_is_named() {
        let mut data = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/data/doc/sources/docx_lists.doc"
        ))
        .unwrap();
        // The WordDocument stream starts with the FIB magic at a sector
        // boundary; turn it into an unknown version.
        let at = (512..data.len())
            .step_by(512)
            .find(|&o| data[o..o + 2] == [0xEC, 0xA5])
            .expect("FIB sector");
        data[at..at + 4].copy_from_slice(&[0x34, 0x12, 0x40, 0x00]);
        let src = SourceDocument::from_bytes("x.doc", InputFormat::Doc, data);
        let Err(err) = DocBackend.convert(&src) else {
            panic!("an unknown Word version must be rejected");
        };
        let err = err.to_string();
        assert!(
            err.contains("wIdent 0x1234") && err.contains("nFib 64"),
            "{err}"
        );
    }
    use crate::InputFormat;

    fn fixture(name: &str) -> SourceDocument {
        let path = format!(
            "{}/../../tests/data/doc/sources/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        let bytes = std::fs::read(&path).expect("fixture exists");
        SourceDocument::from_bytes(name, InputFormat::Doc, bytes)
    }

    /// #535: the stories after the main text — a text box anchored in a
    /// paragraph and one in a table cell, a page header, a footnote — all
    /// reach the document (Word's own `.doc` of the reporter's repro:
    /// `debut` + footnote MARKFN + text box MARKTB1, a 2×2 table with text
    /// box MARKTB2 in cell (2,1), header MARKHDR, `fin`).
    #[test]
    fn text_boxes_headers_and_footnotes_are_read() {
        let path = format!(
            "{}/tests/data/doc/sources/doc_textbox_header_footnote.doc",
            env!("CARGO_MANIFEST_DIR")
        );
        let bytes = std::fs::read(&path).expect("fixture exists");
        let doc = DocBackend
            .convert(&SourceDocument::from_bytes(
                "doc_textbox_header_footnote.doc",
                InputFormat::Doc,
                bytes,
            ))
            .expect("converts");
        let md = doc.export_to_markdown();
        assert!(md.contains("MARKTB1"), "{md}");
        // In its cell, not after the table.
        assert!(md.contains("| MARKTB2 | b2"), "{md}");
        // Furniture: in the JSON, not the Markdown.
        assert!(!md.contains("MARKHDR") && !md.contains("MARKFN"), "{md}");
        let json: serde_json::Value = serde_json::from_str(&doc.export_to_json()).unwrap();
        let furniture: Vec<(String, String)> = json["texts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["content_layer"] == "furniture")
            .map(|t| {
                (
                    t["label"].as_str().unwrap().to_string(),
                    t["text"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        assert_eq!(
            furniture,
            vec![
                ("page_header".to_string(), "MARKHDR".to_string()),
                ("footnote".to_string(), "MARKFN".to_string()),
            ]
        );
        // The text box sits where it is anchored: right after `debut`.
        let groups = json["groups"].as_array().unwrap();
        assert_eq!(groups[0]["name"], "textbox");
        assert_eq!(json["body"]["children"][1]["$ref"], "#/groups/0");
    }

    /// #574: a text box in the page header — a story of its own after the
    /// body text boxes (`ccpHdrTxbx`), anchored by PlcSpaHdr in the header
    /// story — is header furniture, as the same file saved as `.docx` gives
    /// (the reporter's repro: body paragraph, header text box
    /// MARK_HEADER_TEXTBOX_TEXT, an otherwise empty header).
    #[test]
    fn header_text_boxes_are_header_furniture() {
        let path = format!(
            "{}/tests/data/doc/sources/doc_header_textbox.doc",
            env!("CARGO_MANIFEST_DIR")
        );
        let bytes = std::fs::read(&path).expect("fixture exists");
        let doc = DocBackend
            .convert(&SourceDocument::from_bytes(
                "h.doc",
                InputFormat::Doc,
                bytes,
            ))
            .expect("converts");
        let md = doc.export_to_markdown();
        assert_eq!(md.trim(), "Body paragraph before header test.");
        let json: serde_json::Value = serde_json::from_str(&doc.export_to_json()).unwrap();
        let furniture: Vec<(&str, &str)> = json["texts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["content_layer"] == "furniture")
            .map(|t| (t["label"].as_str().unwrap(), t["text"].as_str().unwrap()))
            .collect();
        assert_eq!(furniture, [("page_header", "MARK_HEADER_TEXTBOX_TEXT")]);
    }

    /// #567: prose is Markdown-escaped like the DOCX backend's — `OBJ_DIR`
    /// becomes `OBJ\_DIR` in the Markdown (the reporter's Word 2.133 and
    /// DOCX outputs), and the JSON export restores the raw text.
    #[test]
    fn underscores_are_escaped_in_prose() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/doc/sources/doc_underscores.doc");
        let bytes = std::fs::read(path).expect("fixture");
        let src = SourceDocument::from_bytes("u", InputFormat::Doc, bytes);
        let doc = DocBackend.convert(&src).expect("converts");
        assert_eq!(
            doc.export_to_markdown(),
            "Identifiers: OBJ\\_DIR, MER\\_BAX, LO\\_SNO02, plain text.\n"
        );
        let json: serde_json::Value = serde_json::from_str(&doc.export_to_json()).unwrap();
        assert_eq!(
            json["texts"][0]["text"],
            "Identifiers: OBJ_DIR, MER_BAX, LO_SNO02, plain text."
        );
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        let src = SourceDocument::from_bytes("x.doc", InputFormat::Doc, vec![0u8; 128]);
        assert!(DocBackend.convert(&src).is_err());
    }

    #[test]
    fn extracts_inline_and_floating_images_with_bytes() {
        let doc = DocBackend
            .convert(&fixture("docx_grouped_images.doc"))
            .expect("converts");
        let images: Vec<_> = doc
            .nodes
            .iter()
            .filter_map(|n| match n {
                Node::Picture { image, .. } => Some(image),
                _ => None,
            })
            .collect();
        // 2 grouped + 2 wrapped (floating, via PlcfSpa → Escher pib → delay
        // stream) + 2 inline (Data-stream PICF) — and every one decodable.
        assert_eq!(images.len(), 6, "expected 6 pictures: {:?}", doc.nodes);
        assert!(
            images.iter().all(|i| i.is_some()),
            "every picture should carry decoded bytes"
        );
        let img = images[0].as_ref().unwrap();
        assert!(img.width > 0 && img.height > 0 && !img.data.is_empty());
    }

    /// #566: a flat Word for Windows 2.0 file (`wIdent` 0xA5DB) converts —
    /// paragraphs, the 4 × 3 table through the `\r\x07` cell marks and the
    /// row-end PAPX, bold / italic from the CHPX — to the Markdown Word's own
    /// `.docx` re-save of it converts to; the sniffer knows the header too.
    #[test]
    fn word2_flat_file_converts_like_its_docx_resave() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/doc/sources/word2_pcjs.doc");
        let bytes = std::fs::read(path).expect("fixture");
        assert!(is_word2(&bytes));
        assert_eq!(crate::sniff::detect(&bytes), Some(InputFormat::Doc));
        let src = SourceDocument::from_bytes("word2", InputFormat::Doc, bytes.clone());
        let doc = DocBackend.convert(&src).expect("converts");
        let md = doc.export_to_markdown();
        assert_eq!(
            md.trim_end(),
            "This IS a dummy word document\n\n1.\tsdfsdf\n\n2.\tsdfsdf\n\n3.\tlorem\n\n4.\tipsum\n\n\
             **BOLD TEXT**\n\n***Italic text***\n\n***Underligned***\n\n\
             | Animals     | testa     | testb     |\n\
             |-------------|-----------|-----------|\n\
             | cat         | loremtab  | ipsumtab  |\n\
             | dog         | testacell | testbcell |\n\
             | empty cells |           |           |"
        );
        // A fast-saved file is refused with a reason, not a container error.
        let mut complex = bytes;
        complex[0x0A] |= 0x04;
        let err = DocBackend
            .convert(&SourceDocument::from_bytes("c", InputFormat::Doc, complex))
            .unwrap_err()
            .to_string();
        assert!(err.contains("fast-saved"), "{err}");
    }

    /// #573: a Word 2.0 paragraph in a standard heading style (`stc` 254 =
    /// heading 1) converts to the heading its `.docx` normalization gives
    /// (`##`, docling's level for "heading 1"); the stylesheet's based-on
    /// chain lifts a user style based on a heading too, and the rest stay
    /// body text.
    #[test]
    fn word2_heading_styles_are_headings() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/doc/sources/word2_heading_styles.doc");
        let bytes = std::fs::read(path).expect("fixture");
        let styles = word2_styles(&bytes);
        assert_eq!(styles[254].sti, 1);
        assert_eq!(styles[253].sti, 2);
        assert_eq!(styles[246].sti, 9);
        for stc in [0u8, 1, 2, 222, 255] {
            assert_eq!(styles[stc as usize].sti, 0x0FFF, "stc {stc}");
        }
        let src = SourceDocument::from_bytes("w2", InputFormat::Doc, bytes);
        let md = DocBackend
            .convert(&src)
            .expect("converts")
            .export_to_markdown();
        let headings: Vec<&str> = md.lines().filter(|l| l.starts_with('#')).collect();
        assert_eq!(
            headings,
            ["## HEADING NUMBER ONE TITLE", "## HEADING NUMBER"],
            "{md}"
        );
        // A synthesized stylesheet: user style 7 based on heading 3 (stc 252)
        // is a heading 3; one based on Normal (0) is not; a slot whose base
        // is itself stays plain. Layout: cstcStd 0 → stcp == stc.
        let mut fib = vec![0u8; 0x70];
        fib[0..2].copy_from_slice(&0xA5DBu16.to_le_bytes());
        let stsh_at = fib.len() as u32;
        let mut stsh = Vec::new();
        stsh.extend_from_slice(&0u16.to_le_bytes()); // cstcStd
        for block in [vec![0xFFu8; 9], vec![0xFF; 9], vec![0xFF; 9]] {
            stsh.extend_from_slice(&((block.len() + 2) as u16).to_le_bytes());
            stsh.extend_from_slice(&block);
        }
        stsh.extend_from_slice(&9u16.to_le_bytes()); // iMac
        for stc in 0u8..9 {
            let base = match stc {
                7 => 252,
                8 => 0,
                _ => stc,
            };
            stsh.extend_from_slice(&[0, base]); // stcNext, stcBase
        }
        fib[0x5E..0x62].copy_from_slice(&stsh_at.to_le_bytes());
        fib[0x62..0x64].copy_from_slice(&(stsh.len() as u16).to_le_bytes());
        fib.extend_from_slice(&stsh);
        let styles = word2_styles(&fib);
        assert_eq!(styles[7].sti, 3);
        assert_eq!(styles[8].sti, 0x0FFF);
        assert_eq!(styles[0].sti, 0x0FFF);
        assert_eq!(styles[252].sti, 3);
        // No stylesheet at all: the standard codes still read as headings.
        assert_eq!(word2_styles(&[0xDB, 0xA5])[254].sti, 1);
    }

    /// The PAPX sprm patterns the sample's cells and row ends carry.
    #[test]
    fn word2_papx_table_flags() {
        let cell = [0, 0, 0, 0, 0, 0, 0, 0x11];
        assert_eq!(word2_table_flags(&cell), (true, false));
        let row_end = [0, 0, 0, 0, 0, 0, 0, 0x18, 1, 0x19, 1, 0x94, 0x6c];
        assert_eq!(word2_table_flags(&row_end), (true, true));
        let plain = [0, 0, 0, 0, 0, 0, 0];
        assert_eq!(word2_table_flags(&plain), (false, false));
        assert!(!is_word2(b"\xd0\xcf\x11\xe0 not word 2"));
    }

    /// #641: a paragraph's list comes from its PAPX or, failing that, its
    /// style's based-on chain — `ilfo` and `ilvl` inherited independently,
    /// an explicit `sprmPIlfo` 0 ending the search as "no list", a cyclic
    /// chain ending it too.
    #[test]
    fn style_numbering_resolves_through_the_based_on_chain() {
        let style = |ilfo: Option<u16>, ilvl: Option<u8>, base: u16| StyleDef {
            ilfo,
            ilvl,
            base,
            ..StyleDef::NONE
        };
        // 0 Normal; 1 heading 1 (list 1, level 0); 2 heading 2 based on 1
        // with only its level; 3 a user style based on 2 with nothing; 4 a
        // style in a two-member cycle; 5 its partner.
        let stis = vec![
            style(None, None, 0x0FFF),
            style(Some(1), Some(0), 0),
            style(None, Some(1), 1),
            style(None, None, 2),
            style(None, None, 5),
            style(None, Some(2), 4),
        ];
        let para = |istd: u16, ilfo: Option<u16>, ilvl: Option<u8>| ParaProps {
            istd,
            ilfo,
            ilvl,
            ..ParaProps::default()
        };
        assert_eq!(resolve_numbering(&para(0, None, None), &stis), None);
        assert_eq!(resolve_numbering(&para(1, None, None), &stis), Some((1, 0)));
        // heading 2 takes the list from heading 1 and keeps its own level.
        assert_eq!(resolve_numbering(&para(2, None, None), &stis), Some((1, 1)));
        assert_eq!(resolve_numbering(&para(3, None, None), &stis), Some((1, 1)));
        // The paragraph's own sprms win: another list, or none at all.
        assert_eq!(
            resolve_numbering(&para(2, Some(7), None), &stis),
            Some((7, 1))
        );
        assert_eq!(
            resolve_numbering(&para(2, Some(7), Some(3)), &stis),
            Some((7, 3))
        );
        assert_eq!(resolve_numbering(&para(1, Some(0), None), &stis), None);
        // A level without a list is no list; the cycle ends.
        assert_eq!(resolve_numbering(&para(4, None, None), &stis), None);
        assert_eq!(resolve_numbering(&para(9, None, None), &stis), None);
    }

    /// #641: a heading in a style whose list level numbers visibly carries
    /// docling's heading number; a `none` (0xFF) or bullet level leaves the
    /// text as is, and the heading never becomes a list item.
    #[test]
    fn numbered_heading_styles_prefix_the_heading_text() {
        let lists = |nfc: u8| {
            let lvl = |template: Vec<LvlPart>| LvlInfo {
                nfc,
                start: 1,
                template,
            };
            ListTables {
                lfo_lsids: vec![42],
                lists: std::iter::once((
                    42,
                    vec![
                        lvl(vec![LvlPart::Level(0)]),
                        lvl(vec![
                            LvlPart::Level(0),
                            LvlPart::Text(".".into()),
                            LvlPart::Level(1),
                        ]),
                    ],
                ))
                .collect(),
            }
        };
        let stis = vec![
            StyleDef::NONE,
            StyleDef {
                sti: 1,
                ilfo: Some(1),
                ilvl: Some(0),
                ..StyleDef::NONE
            },
            StyleDef {
                sti: 2,
                ilvl: Some(1),
                base: 1,
                ..StyleDef::NONE
            },
        ];
        let render = |nfc: u8| {
            let mut builder = NodeBuilder::new(lists(nfc));
            let mut doc = DoclingDocument::new("t");
            for (istd, text) in [
                (1, "Introduction"),
                (2, "Context"),
                (2, "Goal"),
                (1, "Design"),
            ] {
                let props = ParaProps {
                    istd,
                    ..ParaProps::default()
                };
                builder.paragraph(
                    text.into(),
                    text.into(),
                    Vec::new(),
                    Vec::new(),
                    '\r',
                    props,
                    &stis,
                    &mut doc,
                );
            }
            doc.export_to_markdown().trim_end().to_string()
        };
        assert_eq!(
            render(0x00),
            "## 1 Introduction\n\n### 1.1 Context\n\n### 1.2 Goal\n\n## 2 Design"
        );
        for nfc in [0xFF, 0x17] {
            assert_eq!(
                render(nfc),
                "## Introduction\n\n### Context\n\n### Goal\n\n## Design",
                "nfc {nfc:#x}"
            );
        }
    }
}
