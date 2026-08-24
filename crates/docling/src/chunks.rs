//! Chunk-record JSON export shared by the CLI (`--to chunks`) and the HTTP
//! server (`to=chunks`): the hierarchical chunker's records always, plus the
//! hybrid chunker's when a tokenizer is available — `DOCLING_CHUNK_TOKENIZER`,
//! or `.models/chunk/tokenizer.json` as populated by
//! `scripts/install/download_dependencies.sh` (requires the `chunking` build
//! feature; `DOCLING_CHUNK_MAX_TOKENS` overrides the default budget of 256).
//!
//! Per-call [`ChunkOptions`] (#256, mirroring the fields of docling's
//! service-datamodel `HybridChunkerOptions`) select a single chunker and
//! override the environment-derived tokenizer/budget; the env knobs stay the
//! operator-side defaults.

use docling_core::chunker::{contextualize, DocChunk, HierarchicalChunker};
use docling_core::DoclingDocument;

/// Which chunker's records `to=chunks` returns (docling's service-datamodel
/// `ChunkerType`, #256). `None` on [`ChunkOptions`] keeps the legacy "both"
/// shape: hierarchical always, hybrid best-effort.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChunkerKind {
    Hierarchical,
    Hybrid,
}

impl ChunkerKind {
    /// Parse the wire value (`"hierarchical"` / `"hybrid"`, as in docling's
    /// `ChunkerType` enum).
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "hierarchical" => Ok(Self::Hierarchical),
            "hybrid" => Ok(Self::Hybrid),
            other => Err(format!(
                "unknown chunker {other:?} (expected: hierarchical, hybrid)"
            )),
        }
    }
}

/// Per-call chunking configuration (#256). Every field falls back to the
/// environment-derived behavior [`chunk_records`] always had, so
/// `ChunkOptions::default()` is exactly the legacy export.
#[derive(Default, Clone, Debug)]
pub struct ChunkOptions {
    /// `Some` returns only that chunker's records (an explicit hybrid request
    /// *fails* when no tokenizer is available instead of silently degrading);
    /// `None` keeps the legacy both-chunkers shape.
    pub chunker: Option<ChunkerKind>,
    /// Explicit HuggingFace `tokenizer.json` path for the hybrid chunker —
    /// overrides `DOCLING_CHUNK_TOKENIZER` and the `.models/chunk/` default.
    pub tokenizer: Option<String>,
    /// Hybrid chunk budget — overrides `DOCLING_CHUNK_MAX_TOKENS` (default
    /// 256, docling's MiniLM budget).
    pub max_tokens: Option<usize>,
    /// Hybrid peer-merging (docling's `merge_peers`, default true); `None`
    /// keeps the chunker default.
    pub merge_peers: Option<bool>,
}

fn records(chunks: &[DocChunk]) -> serde_json::Value {
    serde_json::Value::Array(
        chunks
            .iter()
            .map(|c| {
                serde_json::json!({
                    "text": c.text,
                    "headings": c.headings,
                    "doc_items": c.doc_items.iter().map(|i| i.self_ref.clone()).collect::<Vec<_>>(),
                    "contextualize": contextualize(c),
                })
            })
            .collect(),
    )
}

/// The chunk records for `document` as a JSON object
/// (`{"hierarchical": [...], "hybrid": [...]?}`). Tokenizer problems don't
/// fail the export — the hybrid records are skipped and the problem is
/// reported through `warn`. Equivalent to [`chunk_records_with`] under
/// `ChunkOptions::default()`.
pub fn chunk_records(
    document: &DoclingDocument,
    warn: &mut dyn FnMut(String),
) -> serde_json::Value {
    chunk_records_with(document, &ChunkOptions::default(), warn)
        .expect("default options never error")
}

/// [`chunk_records`] with per-call [`ChunkOptions`] (#256). Errors only on an
/// *explicitly requested* configuration that cannot be honored: `chunker:
/// hybrid` without a usable tokenizer (or in a build without the `chunking`
/// feature). The legacy both-chunkers mode still degrades through `warn`.
pub fn chunk_records_with(
    document: &DoclingDocument,
    options: &ChunkOptions,
    warn: &mut dyn FnMut(String),
) -> Result<serde_json::Value, String> {
    let mut out = serde_json::json!({});
    if options.chunker != Some(ChunkerKind::Hybrid) {
        let hierarchical = HierarchicalChunker.chunk(document);
        out["hierarchical"] = records(&hierarchical);
    }
    if options.chunker == Some(ChunkerKind::Hierarchical) {
        return Ok(out);
    }

    #[cfg(feature = "chunking")]
    {
        let explicit = options.chunker == Some(ChunkerKind::Hybrid);
        let tok_path = match options
            .tokenizer
            .clone()
            .or_else(|| docling_core::env::nonempty("DOCLING_CHUNK_TOKENIZER"))
        {
            Some(p) => Some(p),
            // In legacy mode a missing default tokenizer just skips the
            // hybrid records (no warning — it was never configured); an
            // explicit hybrid request surfaces the resolution error.
            None => match docling_core::chunker::resolve_tokenizer_path(None) {
                Ok(p) => Some(p),
                Err(e) if explicit => return Err(e),
                Err(_) => None,
            },
        };
        if let Some(tok_path) = tok_path {
            let max_tokens = options
                .max_tokens
                .or_else(|| docling_core::env::parse("DOCLING_CHUNK_MAX_TOKENS"))
                .unwrap_or(256);
            match docling_core::chunker::HuggingFaceTokenizer::from_file(&tok_path, max_tokens) {
                Ok(tok) => {
                    let mut chunker = docling_core::chunker::HybridChunker::new(tok);
                    if let Some(mp) = options.merge_peers {
                        chunker = chunker.with_merge_peers(mp);
                    }
                    out["hybrid"] = records(&chunker.chunk(document));
                }
                Err(e) if explicit => return Err(e),
                Err(e) => warn(e),
            }
        }
    }
    #[cfg(not(feature = "chunking"))]
    {
        let _ = warn;
        if options.chunker == Some(ChunkerKind::Hybrid) {
            return Err(
                "the hybrid chunker needs the `chunking` build feature (not enabled)".into(),
            );
        }
    }
    Ok(out)
}
