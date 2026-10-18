//! The integration tests of `docling`, as one test binary (`--test suite`).
//!
//! Each file here was its own test binary, and each binary links the whole
//! crate with every backend: with thin LTO that link was most of a test run's
//! time. As modules of one binary they share one link. Run one module with a
//! filter on its path, e.g.
//!
//! ```sh
//! cargo test -p docling --test suite pptx_list_cascade::
//! ```
//!
//! A test file that changes the process environment (`std::env::set_var`)
//! stays a binary of its own (see `[[test]]` in Cargo.toml), so it cannot
//! leak into these: `image_sources`, `no_ocr`, `pages`,
//! `picture_description`, `textline_orientation_no_model`, `youtube`. So
//! does `regression`, which the fixture docs name (`DOCLING_RS_REGEN=1 cargo
//! test --release -p docling --test regression`).

mod charset;
mod dclx_roundtrip;
mod detect;
mod djvu_prov;
mod document_metadata;
mod document_timeout;
mod docx_upstream;
mod encrypted;
mod feed;
mod heif;
mod html_export;
mod image_output;
mod input_robustness;
mod ipynb;
mod iwork;
mod jpx_pictures;
mod json_prov;
mod options_inventory;
mod page_breaks;
mod pandoc;
mod picture_alt_text;
mod picture_ocr;
mod plain_text;
mod plugins;
mod pptx_charts;
mod pptx_hidden_layers;
mod pptx_list_cascade;
mod redact;
mod scanned;
mod security_doc;
mod svg;
mod tesseract;
mod textline_orientation;
mod vlm;
mod vtt_export;
mod warm_pipeline;
mod word6;
mod xlsx_number_formats;
