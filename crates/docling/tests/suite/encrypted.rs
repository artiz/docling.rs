//! Encrypted ("password protected") Office documents: every format fails
//! with one clear error without the password (#624) — never an empty
//! document, never "bad zip" — and converts with it (#625).
//!
//! Fixtures (`tests/data/encrypted/`, password `1234`): written by Office
//! 16.0 (build 20430) and attached to #624 — `min_encrypted.*` with an open
//! password (Agile OOXML, RC4 CryptoAPI binaries), `min_writepw.*` with a
//! modify password only, and four PowerPoint COM variants of one slide (`B`
//! open password, `C` modify password, `D` both, `H` a password added to a
//! plain file saved in place — a version 4 compound file). The schemes
//! Office no longer writes — Standard OOXML (`std_encrypted.docx`), Office
//! 97/2000 RC4 (`rc4_encrypted.doc`), 40-bit RC4 CryptoAPI
//! (`rc4_40bit_encrypted.doc`) — and a workbook encrypted with Excel's
//! default password (`velvet_encrypted.xlsx`) are built from those files'
//! plaintext by `make_synthetic.py`, which checks each against
//! msoffcrypto-tool.

use std::io::Write;
use std::path::{Path, PathBuf};

use docling::{ConversionError, DocumentConverter, EncryptionError, InputFormat, SourceDocument};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/encrypted")
        .join(name)
}

fn try_convert(source: SourceDocument, password: Option<&str>) -> Result<String, ConversionError> {
    DocumentConverter::new()
        .password(password.map(str::to_string))
        .convert(source)
        .map(|r| r.document.export_to_markdown())
}

fn convert_with(source: SourceDocument, password: Option<&str>) -> Result<String, String> {
    try_convert(source, password).map_err(|e| e.to_string())
}

fn convert(source: SourceDocument) -> Result<String, String> {
    convert_with(source, None)
}

fn convert_file(name: &str, password: Option<&str>) -> Result<String, String> {
    convert_with(SourceDocument::from_file(fixture(name)).unwrap(), password)
}

/// Every encrypted fixture: (file, format, the text of its one slide /
/// paragraph / cell).
const ENCRYPTED: &[(&str, &str, &str)] = &[
    ("min_encrypted.doc", "doc", "Hello encrypted legacy doc"),
    ("min_encrypted.docx", "docx", "Encrypted docx probe"),
    ("min_encrypted.xls", "xls", "Hello"),
    ("min_encrypted.xlsx", "xlsx", "Encrypted xlsx probe"),
    ("min_encrypted.ppt", "ppt", "Hello"),
    ("min_encrypted.pptx", "pptx", "Hello"),
    ("B_openpw.ppt", "ppt", "Hello"),
    ("D_both.ppt", "ppt", "Hello"),
    ("H_A_addpw_save.ppt", "ppt", "Hello"),
    ("std_encrypted.docx", "docx", "Encrypted docx probe"),
    ("rc4_encrypted.doc", "doc", "Hello encrypted legacy doc"),
    (
        "rc4_40bit_encrypted.doc",
        "doc",
        "Hello encrypted legacy doc",
    ),
];

#[test]
fn without_the_password_every_format_says_it_is_encrypted() {
    for &(name, fmt, _) in ENCRYPTED {
        let err = convert_file(name, None).expect_err(name);
        assert!(
            err.ends_with(&format!(
                "{fmt}: document is encrypted (a password is required to open it)"
            )),
            "{name}: {err}"
        );
    }
}

#[test]
fn with_the_password_every_format_converts() {
    for &(name, _, text) in ENCRYPTED {
        let md = convert_file(name, Some("1234")).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(md.contains(text), "{name}: {md}");
    }
}

#[test]
fn a_wrong_password_is_reported_as_wrong() {
    for &(name, fmt, _) in ENCRYPTED {
        let err = convert_file(name, Some("12345")).expect_err(name);
        assert!(
            err.ends_with(&format!(
                "{fmt}: document is encrypted and the password is wrong"
            )),
            "{name}: {err}"
        );
    }
}

/// Files whose only protection is a *modify* password convert without one:
/// Word and Excel leave them unencrypted, PowerPoint encrypts them with its
/// default password (which used to give an empty document, #624) — tried
/// whatever password is given, as Office opens them without prompting.
/// Excel's default password works the same way for a workbook.
#[test]
fn modify_password_and_default_password_files_convert_without_a_password() {
    for (name, text) in [
        ("min_writepw.docx", "Hello write-only"),
        ("min_writepw.xlsx", "Hello write-only"),
        ("min_writepw.ppt", "Hello write-only"),
        ("C_writepw.ppt", "Hello"),
        ("velvet_encrypted.xlsx", "Encrypted xlsx probe"),
    ] {
        for password in [None, Some("not it")] {
            let md = convert_file(name, password).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(md.contains(text), "{name} ({password:?}): {md}");
        }
    }
}

/// A mislabelled encrypted file reports the encryption, not the declared
/// format's parse error: an encrypted `.ppt` named `.pptx` used to fail as
/// a bad ZIP — and with the password it converts as what it is.
#[test]
fn a_mislabelled_encrypted_file_reports_the_encryption() {
    let bytes = std::fs::read(fixture("min_encrypted.ppt")).unwrap();
    let source = || SourceDocument::from_bytes("deck.pptx", InputFormat::Pptx, bytes.clone());
    let err = convert(source()).expect_err("encrypted");
    assert!(err.contains("ppt: document is encrypted"), "{err}");
    let md = convert_with(source(), Some("1234")).unwrap();
    assert!(md.contains("Hello"), "{md}");
}

/// A package whose declared plaintext size exceeds its ciphertext is
/// refused before anything is decrypted.
#[test]
fn an_oversized_package_size_is_refused() {
    let mut bytes = std::fs::read(fixture("min_encrypted.docx")).unwrap();
    // `EncryptedPackage` opens with its u64 plaintext size; find it by the
    // known value (13316 bytes, the decrypted ZIP).
    let at = bytes
        .windows(8)
        .position(|w| w == 13316u64.to_le_bytes())
        .expect("StreamSize");
    bytes[at..at + 8].copy_from_slice(&u64::MAX.to_le_bytes());
    let err = convert_with(
        SourceDocument::from_bytes("big.docx", InputFormat::Docx, bytes),
        Some("1234"),
    )
    .expect_err("oversized");
    assert!(err.contains("EncryptedPackage size"), "{err}");
}

/// A password-protected ODF package lists `content.xml` with
/// `manifest:encryption-data`; its content is ciphertext, which used to fail
/// as "no content.xml". ODF encryption is not decrypted (#625 covers the
/// Office formats), and the error says so rather than asking for a password.
#[test]
fn an_encrypted_odf_package_says_so() {
    let mut out = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(&mut out);
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.oasis.opendocument.text")
        .unwrap();
    zip.start_file("META-INF/manifest.xml", stored).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.2">
 <manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.text"/>
 <manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml" manifest:size="2048">
  <manifest:encryption-data manifest:checksum-type="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#sha256-1k" manifest:checksum="AAAA">
   <manifest:algorithm manifest:algorithm-name="http://www.w3.org/2001/04/xmlenc#aes256-cbc" manifest:initialisation-vector="AAAA"/>
   <manifest:key-derivation manifest:key-derivation-name="PBKDF2" manifest:key-size="32" manifest:iteration-count="100000" manifest:salt="AAAA"/>
  </manifest:encryption-data>
 </manifest:file-entry>
</manifest:manifest>"#,
    )
    .unwrap();
    zip.start_file("content.xml", stored).unwrap();
    zip.write_all(&[0x9C, 0x01, 0xFF, 0x80, 0x00, 0x7F])
        .unwrap();
    zip.finish().unwrap();
    let err = convert(SourceDocument::from_bytes(
        "locked.odt",
        InputFormat::Odt,
        out.into_inner(),
    ))
    .expect_err("encrypted");
    assert!(
        err.ends_with(
            "odt: document is encrypted with an unsupported scheme (ODF package encryption)"
        ),
        "{err}"
    );
}

/// #636: the reason is typed, not only worded — `ConversionError::encryption()`
/// tells "ask for a password" (`NeedPassword`), "ask for another one"
/// (`WrongPassword`) and "no password will do" apart on every format, and
/// the message is byte for byte what #624 fixed. Every other failure has
/// no encryption value.
#[test]
fn the_encryption_error_is_typed() {
    for &(name, fmt, _) in ENCRYPTED {
        let source = || SourceDocument::from_file(fixture(name)).unwrap();
        let err = try_convert(source(), None).expect_err(name);
        assert_eq!(
            err.encryption(),
            Some(&EncryptionError::NeedPassword),
            "{name}: {err}"
        );
        assert!(err.encryption().unwrap().needs_password());
        assert_eq!(
            err.to_string(),
            format!(
                "parse error: {fmt}: document is encrypted (a password is required to open it)"
            )
        );
        let err = try_convert(source(), Some("12345")).expect_err(name);
        assert_eq!(
            err.encryption(),
            Some(&EncryptionError::WrongPassword),
            "{name}: {err}"
        );
        assert_eq!(
            err.to_string(),
            format!("parse error: {fmt}: document is encrypted and the password is wrong")
        );
    }
    // A damaged file is a parse error without an encryption value, so a
    // client does not prompt for a password it cannot use.
    let err = try_convert(
        SourceDocument::from_bytes(
            "junk.docx",
            InputFormat::Docx,
            b"PK\x03\x04 not a zip".to_vec(),
        ),
        None,
    )
    .expect_err("junk");
    assert_eq!(err.encryption(), None, "{err}");
}

/// #636: a mislabelled encrypted file (an encrypted `.ppt` named `.pptx`)
/// is typed through the converter's content-sniffing retry too — that retry
/// picks the encryption error by its type now, not by its text.
#[test]
fn a_mislabelled_encrypted_file_is_typed() {
    let bytes = std::fs::read(fixture("min_encrypted.ppt")).unwrap();
    let err = try_convert(
        SourceDocument::from_bytes("deck.pptx", InputFormat::Pptx, bytes),
        None,
    )
    .expect_err("encrypted");
    assert_eq!(
        err.encryption(),
        Some(&EncryptionError::NeedPassword),
        "{err}"
    );
    assert!(
        err.to_string().contains("ppt: document is encrypted"),
        "{err}"
    );
}

/// #636: an undecryptable scheme is typed as such — `needs_password()` is
/// false, so a client does not ask for a password that cannot help.
#[test]
fn an_undecryptable_scheme_is_typed_as_not_needing_a_password() {
    // The ODF package of `an_encrypted_odf_package_says_so`, by content: the
    // sniffed `.odt` route reports ODF encryption as not decryptable.
    let mut out = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(&mut out);
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.oasis.opendocument.text")
        .unwrap();
    zip.start_file("META-INF/manifest.xml", stored).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.2">
 <manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.text"/>
 <manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml" manifest:size="2048">
  <manifest:encryption-data manifest:checksum-type="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#sha256-1k" manifest:checksum="AAAA">
   <manifest:algorithm manifest:algorithm-name="http://www.w3.org/2001/04/xmlenc#aes256-cbc" manifest:initialisation-vector="AAAA"/>
   <manifest:key-derivation manifest:key-derivation-name="PBKDF2" manifest:key-size="32" manifest:iteration-count="100000" manifest:salt="AAAA"/>
  </manifest:encryption-data>
 </manifest:file-entry>
</manifest:manifest>"#,
    )
    .unwrap();
    zip.start_file("content.xml", stored).unwrap();
    zip.write_all(&[0x9C, 0x01, 0xFF, 0x80, 0x00, 0x7F])
        .unwrap();
    zip.finish().unwrap();
    let err = try_convert(
        SourceDocument::from_bytes("locked.odt", InputFormat::Odt, out.into_inner()),
        Some("1234"),
    )
    .expect_err("encrypted");
    let kind = err.encryption().expect("typed");
    assert!(
        matches!(kind, EncryptionError::NotDecryptable(s) if s == "ODF package encryption"),
        "{kind:?}"
    );
    assert!(!kind.needs_password());
    assert_eq!(kind.code(), "not_decryptable");
}

/// #636: an encrypted PDF is typed through the converter like an Office
/// file — on the full pipeline and on the text-layer path — with the PDF
/// reader's own message (docling's) unchanged. `Pipeline::new()` loads no
/// models and the password check runs before any page is rendered, so this
/// needs no `.models/`.
#[test]
fn an_encrypted_pdf_is_typed_through_the_converter() {
    let pdf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/data/pdf_password/sources/2206.01062_pg3.pdf");
    let source = || SourceDocument::from_file(&pdf).unwrap();
    for text_layer_only in [false, true] {
        let convert = |password: Option<&str>| {
            DocumentConverter::new()
                .text_layer_only(text_layer_only)
                .password(password.map(str::to_string))
                .convert(source())
        };
        let err = convert(None).expect_err("no password");
        assert_eq!(
            err.encryption(),
            Some(&EncryptionError::NeedPassword),
            "text_layer_only={text_layer_only}: {err}"
        );
        assert_eq!(
            err.to_string(),
            "parse error: pdf: the PDF is encrypted: a password is required"
        );
        let err = convert(Some("nope")).expect_err("wrong password");
        assert_eq!(
            err.encryption(),
            Some(&EncryptionError::WrongPassword),
            "text_layer_only={text_layer_only}: {err}"
        );
        assert_eq!(
            err.to_string(),
            "parse error: pdf: the PDF is encrypted and the password is wrong"
        );
    }
    // The right password opens it on the text-layer path (no models needed).
    let md = DocumentConverter::new()
        .text_layer_only(true)
        .password(Some("1234".into()))
        .convert(source())
        .map(|r| r.document.export_to_markdown())
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(md.contains("DocLayNet"), "{md}");
}
