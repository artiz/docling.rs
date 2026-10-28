//! #903 / #904: an HTML conversion keeps the source's blocks — every item
//! of the tree names the element it was made from (tag, `id`, `class`,
//! element path), every block of the source is enumerated with its own
//! normalized text, and a block that produced no item says why. The EUR-Lex
//! consolidated text of Regulation (EU) 2023/1803 the issues were measured
//! on, cut down to IAS 12 paragraphs 77–80 and the reconciliation table of
//! paragraph 81(c) (`fixtures/html_blocks/eurlex_ias12_excerpt.xhtml`).

use std::path::{Path, PathBuf};

use docling::{DocumentConverter, SourceDocument};
use docling_core::tree::{BlockOmission, TreeKind};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/html_blocks/eurlex_ias12_excerpt.xhtml")
}

const P79: &str =
    "79 Die Hauptbestandteile des Steueraufwands (Steuerertrags) sind getrennt anzugeben.";

#[test]
fn items_name_their_source_block_and_blocks_read_on_their_own() {
    let doc = DocumentConverter::new()
        .convert(SourceDocument::from_file(fixture()).expect("fixture"))
        .expect("conversion")
        .document;
    let tree = doc
        .tree
        .as_ref()
        .expect("the HTML backend builds the item tree");
    assert!(!tree.blocks.is_empty());
    // Every item knows its block.
    for (i, it) in tree.items.iter().enumerate() {
        assert!(it.block.is_some(), "item {i} {:?}", it.kind);
    }
    // Paragraph 79 of IAS 12 (#904's reference: 84 bytes, no serializer
    // separator), its locator, and the paragraph after it.
    let p79 = tree
        .blocks
        .iter()
        .position(|b| b.text == P79)
        .expect("paragraph 79");
    let b = &tree.blocks[p79];
    assert_eq!(
        (b.tag.as_str(), b.class.as_deref(), b.id.as_deref()),
        ("p", Some("norm"), None)
    );
    assert_eq!(b.path, "html/body[1]/div[1]/div[1]/div[4]/p[1]");
    assert_eq!(tree.source_text([p79]).len(), 84);
    assert!(tree.blocks[p79 + 1]
        .text
        .starts_with("80 Zu den Bestandteilen"));
    assert_eq!(
        tree.source_text([p79, p79 + 1]),
        format!("{P79}\n{}", tree.blocks[p79 + 1].text)
    );
    // Where `export_to_text()` puts the serializer's blank line after it.
    let text = doc.export_to_text();
    let start = text.find("79 Die Hauptbestandteile").unwrap();
    let end = text.find("80 Zu den Bestandteilen").unwrap();
    assert_eq!(&text[start..end], format!("{P79}\n\n"));
    // The item made of it (one bold run — the three spans simplify to it)
    // is tied to the block; so is every run of paragraph 80's inline
    // group (plain `80`, the spacing span, plain text).
    let [item] = b.items[..] else {
        panic!("{:?}", b.items)
    };
    assert_eq!(tree.items[item].block, Some(p79));
    let p80 = &tree.blocks[p79 + 1];
    for &i in &p80.items {
        assert_eq!(tree.items[i].block, Some(p79 + 1));
        for &c in &tree.items[i].children {
            assert_eq!(tree.items[c].block, Some(p79 + 1));
        }
    }
    // A heading keeps its `id`.
    let angaben = tree.blocks.iter().find(|b| b.text == "ANGABEN").unwrap();
    assert_eq!(
        angaben.id.as_deref(),
        Some("id-c07c7a91-7f80-48d2-a14e-525ebaf11271")
    );
    assert_eq!(angaben.class.as_deref(), Some("title-gr-seq-level-2"));
    // The table's cells are in its grid, the hidden and the empty paragraph
    // say so.
    let table = tree
        .items
        .iter()
        .position(|it| matches!(it.kind, TreeKind::Table { .. }))
        .unwrap();
    let in_table = tree
        .blocks
        .iter()
        .filter(|b| b.omitted == Some(BlockOmission::InTable(table)))
        .count();
    assert!(in_table >= 30, "{in_table}");
    assert!(tree
        .blocks
        .iter()
        .any(|b| b.text == "Versteckter Absatz" && b.omitted == Some(BlockOmission::Hidden)));
    assert!(tree
        .blocks
        .iter()
        .any(|b| b.path == "html/body[1]/p[3]" && b.omitted == Some(BlockOmission::Empty)));
    // The JSON export is docling's schema, blocks and all: nothing new in it.
    let json = doc.export_to_json();
    assert!(!json.contains("\"blocks\"") && !json.contains("\"block\""));
}
