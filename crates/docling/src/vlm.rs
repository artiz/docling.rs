//! VLM pipeline (issue #77) — remote OpenAI-compatible vision endpoint.
//!
//! The counterpart of docling's `VlmPipeline` in its remote form
//! (`ApiVlmOptions`): each PDF page is rendered to an image, sent to an
//! OpenAI-compatible `chat/completions` endpoint (LM Studio, Ollama, vLLM, or
//! a hosted service) together with a DocLang-eliciting prompt, and the
//! returned markup is parsed by the existing DocLang reader
//! (`backend::doclang`) into a [`DoclingDocument`]. Local ONNX inference of a
//! docling VLM is a later enhancement — this module deliberately contains no
//! model code, just the request loop.
//!
//! HTTP goes over `ureq`, the crate's existing blocking client
//! (`fetch-images` pulls the same one, keeping a single HTTP stack in the
//! graph — the converter is synchronous, so an async client would only add a
//! runtime). Transient failures (transport errors, 408/429, 5xx) retry with
//! exponential backoff; anything else fails the conversion loudly — a VLM
//! conversion with silently dropped pages would be worse than an error.

use std::io::Cursor;
use std::time::Duration;

use docling_core::DoclingDocument;

use crate::backend::doclang::DoclangBackend;
use crate::backend::DeclarativeBackend;
use crate::error::ConversionError;
use crate::format::InputFormat;
use crate::source::SourceDocument;

/// Configuration for the remote VLM conversion. Everything has an env-var
/// fallback so `--pipeline vlm` works without repeating flags:
/// `DOCLING_RS_VLM_ENDPOINT`, `DOCLING_RS_VLM_MODEL`, `DOCLING_RS_VLM_PROMPT`,
/// `DOCLING_RS_VLM_API_KEY`.
#[derive(Debug, Clone)]
pub struct VlmOptions {
    /// Base URL of the OpenAI-compatible server (`http://localhost:11434/v1`)
    /// or the full `…/chat/completions` URL — the suffix is appended when
    /// missing, so both spellings work.
    pub endpoint: String,
    /// Model name as the server knows it (e.g. `granite-docling`).
    pub model: String,
    /// The instruction sent with every page image. Defaults to docling's
    /// DocLang-eliciting prompt ([`DEFAULT_VLM_PROMPT`]).
    pub prompt: Option<String>,
    /// Bearer token, if the endpoint wants one. Local servers don't.
    pub api_key: Option<String>,
    /// 1-based inclusive page window (`--pages` composes with the VLM
    /// pipeline exactly like with the ML one).
    pub page_range: Option<(usize, usize)>,
    /// `max_tokens` for each completion. A dense page of DocLang easily runs
    /// long; the default (8192) fits every corpus page with headroom.
    pub max_tokens: usize,
}

/// docling's page-conversion instruction for its DocLang-emitting VLMs.
pub const DEFAULT_VLM_PROMPT: &str = "Convert this page to docling.";

impl VlmOptions {
    /// Build options from explicit values, falling back to the
    /// `DOCLING_RS_VLM_*` environment. Endpoint and model are required —
    /// there is no sensible default server to talk to.
    pub fn resolve(
        endpoint: Option<String>,
        model: Option<String>,
    ) -> Result<Self, ConversionError> {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let endpoint = endpoint
            .or_else(|| env("DOCLING_RS_VLM_ENDPOINT"))
            .ok_or_else(|| {
                ConversionError::Parse(
                    "vlm: no endpoint (pass --vlm-endpoint or set DOCLING_RS_VLM_ENDPOINT)".into(),
                )
            })?;
        let model = model
            .or_else(|| env("DOCLING_RS_VLM_MODEL"))
            .ok_or_else(|| {
                ConversionError::Parse(
                    "vlm: no model (pass --vlm-model or set DOCLING_RS_VLM_MODEL)".into(),
                )
            })?;
        Ok(Self {
            endpoint,
            model,
            prompt: env("DOCLING_RS_VLM_PROMPT"),
            api_key: env("DOCLING_RS_VLM_API_KEY"),
            page_range: None,
            max_tokens: 8192,
        })
    }

    fn url(&self) -> String {
        let base = self.endpoint.trim_end_matches('/');
        if base.ends_with("/chat/completions") {
            base.to_string()
        } else {
            format!("{base}/chat/completions")
        }
    }
}

/// Convert a PDF or image through the remote VLM. PDF pages render via
/// pdfium at the ML pipeline's scale; a standalone image is sent as-is (it is
/// its own page). Every page must convert — a failed page fails the document.
pub fn convert_vlm(
    source: &SourceDocument,
    opts: &VlmOptions,
) -> Result<DoclingDocument, ConversionError> {
    let agent = agent();
    let mut fragments: Vec<String> = Vec::new();
    match source.format {
        InputFormat::Pdf => {
            // 1-based window → 0-based inclusive, validated like Pipeline::pages.
            let total = docling_pdf::pdfium_backend::page_count(&source.bytes, None)
                .map_err(|e| ConversionError::Parse(format!("vlm: open pdf: {e}")))?;
            let range = match opts.page_range {
                Some((first, last)) => {
                    if first == 0 || last < first {
                        return Err(ConversionError::Parse(format!(
                            "invalid page range {first}-{last} (pages are 1-based, first <= last)"
                        )));
                    }
                    if first > total {
                        return Err(ConversionError::Parse(format!(
                            "page range {first}-{last} is outside the document ({total} page(s))"
                        )));
                    }
                    Some((first - 1, last.min(total) - 1))
                }
                None => None,
            };
            // `for_each_page`'s error type must absorb pdfium's own errors,
            // so page-level VLM failures travel as PdfError strings and are
            // rewrapped once below.
            docling_pdf::pdfium_backend::for_each_page::<docling_pdf::PdfError, _>(
                &source.bytes,
                None,
                true, // render page images — they are the whole input here
                range,
                |i, _total, page| {
                    let png = encode_png(&page.image).map_err(|e| {
                        docling_pdf::PdfError::Pdfium(format!("page {}: {e}", i + 1))
                    })?;
                    let markup = request_page(&agent, opts, &png).map_err(|e| {
                        docling_pdf::PdfError::Pdfium(format!("vlm: page {}: {e}", i + 1))
                    })?;
                    fragments.push(doclang_fragment(&markup));
                    Ok(())
                },
            )
            .map_err(pdf_err)?;
        }
        InputFormat::Image => {
            // The image file is already the page; no re-encode, no pdfium.
            let markup = request_page(&agent, opts, &source.bytes)
                .map_err(|e| ConversionError::Parse(format!("vlm: {e}")))?;
            fragments.push(doclang_fragment(&markup));
        }
        other => {
            return Err(ConversionError::Parse(format!(
                "vlm pipeline converts PDF and image inputs (got {other:?})"
            )));
        }
    }
    // One document out of the per-page fragments, through the tolerant
    // DocLang reader — exactly what a `.dclg` file would take.
    let xml = format!(
        "<doclang version=\"0.7\">\n{}\n</doclang>",
        fragments.join("\n")
    );
    let synthetic =
        SourceDocument::from_bytes(&source.name, InputFormat::XmlDoclang, xml.into_bytes());
    DoclangBackend.convert(&synthetic)
}

fn pdf_err(e: docling_pdf::PdfError) -> ConversionError {
    ConversionError::with_source("pdf", e)
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(10)))
        // A VLM can chew on a dense page for minutes, especially on CPU.
        .timeout_global(Some(Duration::from_secs(600)))
        // Keep non-2xx as inspectable responses for the retry decision.
        .http_status_as_error(false)
        .build()
        .into()
}

/// POST one page image, return the model's text. Retries transport errors,
/// 408/429 and 5xx with exponential backoff (2s/4s/8s); other statuses and a
/// malformed body fail immediately.
fn request_page(agent: &ureq::Agent, opts: &VlmOptions, image: &[u8]) -> Result<String, String> {
    let data_uri = format!(
        "data:image/png;base64,{}",
        docling_core::base64::encode(image)
    );
    let body = serde_json::json!({
        "model": opts.model,
        // Deterministic-ish output: sampling noise only hurts a structured
        // markup task (docling's ApiVlmOptions ships temperature 0 too).
        "temperature": 0,
        "max_tokens": opts.max_tokens,
        "messages": [{
            "role": "user",
            "content": [
                { "type": "text",
                  "text": opts.prompt.as_deref().unwrap_or(DEFAULT_VLM_PROMPT) },
                { "type": "image_url", "image_url": { "url": data_uri } },
            ],
        }],
    });
    let url = opts.url();
    let mut delay = Duration::from_secs(2);
    let mut last_err = String::new();
    for attempt in 0..4 {
        if attempt > 0 {
            std::thread::sleep(delay);
            delay *= 2;
        }
        let mut req = agent.post(&url).header("content-type", "application/json");
        if let Some(key) = &opts.api_key {
            req = req.header("authorization", &format!("Bearer {key}"));
        }
        // Hand-serialized body: the crate pulls ureq without its `json`
        // feature (fetch-images doesn't need it), and one to_string keeps it
        // that way.
        let payload = serde_json::to_string(&body).expect("static json shape");
        match req.send(payload.as_bytes()) {
            Ok(mut resp) => {
                let status = resp.status().as_u16();
                let text = resp
                    .body_mut()
                    .read_to_string()
                    .map_err(|e| format!("{url}: read response: {e}"))?;
                if status == 408 || status == 429 || status >= 500 {
                    last_err = format!("{url}: HTTP {status} (attempt {})", attempt + 1);
                    continue;
                }
                if status != 200 {
                    return Err(format!(
                        "{url}: HTTP {status}: {}",
                        text.chars().take(300).collect::<String>()
                    ));
                }
                let parsed: serde_json::Value = serde_json::from_str(&text)
                    .map_err(|e| format!("{url}: malformed JSON response: {e}"))?;
                return parsed["choices"][0]["message"]["content"]
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("{url}: no choices[0].message.content in response"));
            }
            Err(e) => {
                last_err = format!("{url}: {e} (attempt {})", attempt + 1);
            }
        }
    }
    Err(format!("giving up after 4 attempts: {last_err}"))
}

/// Reduce one model response to DocLang *body* markup, ready to concatenate
/// under a single `<doclang>` root.
///
/// Models wrap their answer unpredictably: Markdown code fences, a full
/// `<doclang …>` document, a legacy `<doctag>` root, or a bare fragment of
/// block elements. The DocLang reader is already tolerant of unknown
/// elements/attributes, so normalization only needs to strip the wrappers —
/// content inside an unexpected root still parses (unknown elements recurse).
fn doclang_fragment(response: &str) -> String {
    let mut text = response.trim();
    // ```xml … ``` / ``` … ``` fences.
    if let Some(rest) = text.strip_prefix("```") {
        let rest = rest.split_once('\n').map(|(_, r)| r).unwrap_or(rest);
        text = rest
            .rsplit_once("```")
            .map(|(r, _)| r)
            .unwrap_or(rest)
            .trim();
    }
    // Unwrap a <doclang>/<doctag> root down to its children.
    for root in ["doclang", "doctag", "doctags"] {
        let open = format!("<{root}");
        if let Some(start) = text.find(&open) {
            if let Some(gt) = text[start..].find('>') {
                let inner_start = start + gt + 1;
                let close = format!("</{root}>");
                let inner_end = text.rfind(&close).unwrap_or(text.len());
                if inner_start <= inner_end {
                    return text[inner_start..inner_end].trim().to_string();
                }
            }
        }
    }
    text.to_string()
}

/// PNG-encode a rendered page (the wire format every OpenAI-compatible
/// server accepts as a data URI).
fn encode_png(image: &image::RgbImage) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .map_err(|e| format!("encode page image: {e}"))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::doclang_fragment;

    #[test]
    fn fragment_normalization() {
        // Bare fragment passes through.
        assert_eq!(doclang_fragment("<text>hi</text>"), "<text>hi</text>");
        // Fenced answer is unwrapped.
        assert_eq!(
            doclang_fragment("```xml\n<text>hi</text>\n```"),
            "<text>hi</text>"
        );
        // A full document root is stripped down to its body.
        assert_eq!(
            doclang_fragment("<doclang version=\"0.7\"><text>hi</text></doclang>"),
            "<text>hi</text>"
        );
        // Legacy doctag root likewise.
        assert_eq!(
            doclang_fragment("<doctag><text>hi</text></doctag>"),
            "<text>hi</text>"
        );
        // Prose around a fenced fragment is ignored by the fence rule only
        // when the fence comes first; a root element wins anywhere.
        assert_eq!(
            doclang_fragment("Here you go:\n<doclang><heading level=\"1\">T</heading></doclang>"),
            "<heading level=\"1\">T</heading>"
        );
    }
}
