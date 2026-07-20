//! Ollama embedding provider (the default). Talks to a local Ollama server's
//! `/api/embed` endpoint. `bge-m3` yields 1024-dimensional vectors.

use super::Embedder;
use crate::{RagConfig, RagError, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Embedder backed by an Ollama server.
#[derive(Debug, Clone)]
pub struct OllamaEmbedder {
    client: reqwest::Client,
    base_url: String,
    model: String,
    dim: usize,
    id: String,
}

#[derive(Serialize)]
struct EmbedReq<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(Deserialize)]
struct EmbedResp {
    #[serde(default)]
    embeddings: Vec<Vec<f32>>,
}

impl OllamaEmbedder {
    /// Build from resolved config (`OLLAMA_BASE_URL`, `RAG_EMBED_MODEL`, `RAG_EMBED_DIM`).
    pub fn from_config(cfg: &RagConfig) -> Self {
        OllamaEmbedder {
            client: reqwest::Client::new(),
            base_url: cfg.ollama_base_url.trim_end_matches('/').to_string(),
            model: cfg.embed_model.clone(),
            dim: cfg.embed_dim,
            id: format!("ollama:{}", cfg.embed_model),
        }
    }
}

#[async_trait]
impl Embedder for OllamaEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let url = format!("{}/api/embed", self.base_url);
        let resp = self
            .client
            .post(&url)
            .json(&EmbedReq {
                model: &self.model,
                input: texts,
            })
            .send()
            .await?;
        // Surface Ollama's error body — it names the actual problem (e.g.
        // `model "bge-m3" not found, try pulling it first`, which arrives as
        // a 404 just like an unknown endpoint would on a pre-0.2.6 server).
        let status = resp.status();
        if !status.is_success() {
            let detail = resp.text().await.unwrap_or_default();
            let hint = if status == reqwest::StatusCode::NOT_FOUND {
                format!(
                    " — if the model is missing run `ollama pull {}`; if the \
                     endpoint is unknown, update Ollama (>= 0.2.6 for /api/embed)",
                    self.model
                )
            } else {
                String::new()
            };
            return Err(RagError::Embedding(format!(
                "ollama {url}: HTTP {status}: {}{hint}",
                detail.trim()
            )));
        }
        let body: EmbedResp = resp.json().await?;
        if body.embeddings.len() != texts.len() {
            return Err(RagError::Embedding(format!(
                "ollama returned {} embeddings for {} inputs",
                body.embeddings.len(),
                texts.len()
            )));
        }
        Ok(body.embeddings)
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn id(&self) -> &str {
        &self.id
    }
}
