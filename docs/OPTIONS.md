# Conversion options — one set, every surface

Every way of driving docling.rs — the `docling-rs` CLI, `docling-rs serve` /
`docling-serve` (query string, JSON body or multipart text parts), the
Python `docling_rs.DocumentConverter`, the Node `docling.rs` package, the C
ABI (`docling-ffi`) and the wasm module (`convert_with_options`) — speaks
the same conversion options: the fields of
[`docling::ConvertOptions`](../crates/docling/src/options.rs) (#577).
Validation (`ConvertOptions::validate`) and the mapping onto the converter
(`ConvertOptions::apply`) live there once; the surfaces only parse their
input shape. The engine defaults are defined once too, in
`DocumentConverter::default()`: an option a caller leaves unset means that
default on every surface.

The **wire** column is the JSON key (serve, C ABI, wasm) and the Rust field;
the CLI flag is `--` + the name with `_` → `-`, the Node option is the name
in camelCase, the Python keyword argument is the name itself — except where
the table says otherwise. The inventory test
(`crates/docling/tests/options_inventory.rs`) holds every surface's
documentation to this table, and the table to the struct: an option added
to `ConvertOptions` fails CI until it is a flag, a request option, a kwarg,
a property of the Node option objects and a row here.

What is *not* in the set: how and where to emit the result (`to`, the image
mode, `pandoc_api_version`, the chunker settings). Those depend on what the
surface can do with the output — serve streams, the browser cannot write
files, the CLI writes several formats at once — so each surface keeps its
own output options next to the shared ones.

| Wire name | Values | Default | CLI | Python | Node | Meaning |
|---|---|---|---|---|---|---|
| `strict` | bool | false | `--strict` | — | `strict` | Cleaner, more conformant Markdown instead of the readable default. Python: at export time (`export_to_markdown` reads `document.strict_markdown`). |
| `compact_tables` | bool | false | `--compact-tables` | `compact_tables` | `compactTables` | Unpadded `| a | b |` Markdown tables (#271). |
| `page_break_placeholder` | string | unset = no breaks | `--page-break-placeholder` | — | `pageBreakPlaceholder` | Text inserted between pages in the Markdown export (docling-core's `MarkdownParams.page_break_placeholder`). serve also accepts the historical `md_page_break_placeholder`; Python: `export_to_markdown(page_break_placeholder=…)`; Node: an output option (`OutputOptions`). |
| `fetch_images` | bool | false | `--fetch-images` | `fetch_images` | `fetchImages` | Resolve external `<img src>` for HTML/EPUB/MHTML/JATS (network access) — the pre-#646 switch, the alias of `image_sources=remote` (`false` = `none`; an explicit `image_sources` wins). serve honours it only under `--allow-url-fetch`. |
| `image_sources` | `none` \| `embedded` \| `local` \| `remote` | `none` | `--image-sources` | `image_sources` | `imageSources` | Which image references resolve (#646): `embedded` = `data:` URIs and parts of the same container (EPUB/MHTML entries, email `cid:` attachments — no filesystem, no network); `local` = plus files under the source file's directory (never an absolute path, never outside it); `remote` = plus `http(s)` fetches. HTML, EPUB, MHTML, JATS, AsciiDoc, ODF, Markdown (`![…](…)`, inline `<img>`, embedded HTML) and email bodies. serve: `remote` without `--allow-url-fetch` is held to `embedded`; `local` needs `--allow-local-images` (400 otherwise). |
| `image_hosts` | list / comma-separated | unset = any host | `--image-hosts` | `image_hosts` | `imageHosts` | Hosts a `remote` image fetch may reach (#646): exact names or `*.suffix` wildcards, case-insensitive; a redirect is held to the same list. The private/loopback block-list applies regardless (`DOCLING_RS_ALLOW_PRIVATE_IP_FETCH` is the only way around it). |
| `max_image_bytes` | bytes > 0 | 32 MiB (`DOCLING_RS_MAX_IMAGE_BYTES`) | `--max-image-bytes` | `max_image_bytes` | `maxImageBytes` | Largest image that resolves (#646); a larger one stays a placeholder. Also the remote fetch's read cap. |
| `max_images` | int ≥ 0 | unlimited (`DOCLING_RS_MAX_IMAGES`) | `--max-images` | `max_images` | `maxImages` | Images resolved per document (#646); the rest stay placeholders, one warning per document says so. |
| `max_image_total_mb` | MiB ≥ 0 | unlimited (`DOCLING_RS_MAX_IMAGE_TOTAL_MB`) | `--max-image-total-mb` | `max_image_total_mb` | `maxImageTotalMb` | Total resolved image bytes per document (#646). |
| `min_image_bytes` | bytes ≥ 0 | 0 (`DOCLING_RS_MIN_IMAGE_BYTES`) | `--min-image-bytes` | `min_image_bytes` | `minImageBytes` | Smallest image that resolves (#646) — spacers and tracking pixels are a few dozen bytes; skipped quietly, no budget charged. |
| `list_attachments` | bool | false | `--list-attachments` | `list_attachments` | `listAttachments` | Email (.eml/.msg): append an Attachments section — names and content types, never the payload (#251). |
| `skip_empty_cells` | bool | false | `--skip-empty-cells` | `skip_empty_cells` | `skipEmptyCells` | Omit empty cells from sparse XLSX/XLS table grids (#271). |
| `ebcdic_layout` | string | unset = the `<stem>.layout.json` sidecar | `--ebcdic-layout` | `ebcdic_layout` | `ebcdicLayout` | EBCDIC copybook layout (#252): inline `EbcdicLayout` JSON or a file path. |
| `encoding` | WHATWG label | unset = detect (BOM, UTF-8, windows-1252) | `--encoding` | `encoding` | `encoding` | Character encoding of text inputs (docling's `TextBackendOptions.encoding`). |
| `use_web_browser` | bool | false | `--use-web-browser` | `use_web_browser` | — | Pre-render HTML with a headless browser (the `web-browser` Cargo feature). Not in the npm addon (not built with the feature). |
| `xbrl_taxonomy` | directory | unset = the instance's own directory | `--xbrl-taxonomy` | `xbrl_taxonomy` | `xbrlTaxonomy` | XBRL: the directory the instance's taxonomy is read from (docling's `XBRLBackendOptions.taxonomy`). serve: a server-local relative path without `..`. |
| `asr_model` | preset name | unset = Whisper tiny | `--asr-model` | `asr_model` | `asrModel` | ASR preset for audio/video (`whisper_*`, `parakeet_tdt_0.6b_v3`). |
| `asr_lang` | Whisper code \| `auto` | `auto` | `--asr-lang` | `asr_lang` | `asrLang` | Transcription language for audio/video. |
| `video_frames` | int ≥ 0 \| `all` | 8 | `--video-frames` | `video_frames` | `videoFrames` | Max frames sampled from a video (0 = transcript only; needs ffmpeg). `all` = every distinct cut (#647) — on the JSON surfaces also any count ≥ 4294967295, which is how the npm addon (32-bit integers) asks for it; Python also takes `docling_rs.VIDEO_FRAMES_ALL`. |
| `video_scene_threshold` | 0.0–1.0 | 0.27 (`DOCLING_RS_VIDEO_SCENE_THRESHOLD`) | `--video-scene-threshold` | `video_scene_threshold` | `videoSceneThreshold` | ffmpeg's `scene` score a frame must exceed to count as a cut (#647): a slide change scores well above the default, a fade or re-lighting around 0.5, a hard cut near 1.0 — 0.6 keeps hard cuts only. |
| `video_frame_max_side` | int ≥ 0 px | 0 = source resolution (`DOCLING_RS_VIDEO_FRAME_MAX_SIDE`) | `--video-frame-max-side` | `video_frame_max_side` | `videoFrameMaxSide` | Downscale each sampled frame inside ffmpeg's filter chain so its longer side is at most this (#647): a 1080p recording at 640 yields 640×360 PNGs, so neither the embedded images nor the OCR input reach memory at full size. |
| `video_frame_dedupe` | int 0–64 | unset = keep every frame (`DOCLING_RS_VIDEO_FRAME_DEDUPE`) | `--video-frame-dedupe` | `video_frame_dedupe` | `videoFrameDedupe` | Drop a sampled frame whose 64-bit difference hash (9×8 grey thumbnail) is within this Hamming distance of a frame already kept (#647): the same slide after a fade, or shown again after a camera cut, becomes one picture. 4–6 is a good distance. |
| `pages` | `A-B` \| `N` | unset = every page | `--pages` (also `--page-range`) | `page_range` | `pages` | PDF page window, 1-based inclusive (#80). Python: `page_range=(first, last)`. |
| `document_timeout` | seconds > 0 | unset = unlimited | `--document-timeout` | `document_timeout` | `documentTimeout` | Per-document budget for the PDF pipeline (#497): checked between pages, the pages done so far are the partial document. |
| `no_ocr` | bool | false | `--no-ocr` | `do_ocr` | `noOcr` | Keep layout + TableFormer, never run OCR — docling's `--no-ocr` / `do_ocr=False` (#611; until 2.0 `no_ocr` was the text-layer path, now `text_layer_only`). Python: `do_ocr` (inverted). |
| `skip_ocr` | bool | false | `--skip-ocr` | — | `skipOcr` | `no_ocr` under its pre-2.0 name (#244), still read: either one set skips OCR. Python: `do_ocr`. |
| `text_layer_only` | bool | false | `--text-layer-only` | `text_layer_only` | `textLayerOnly` | Skip the whole ML stack: the embedded text layer as flat paragraphs (what `no_ocr` meant before 2.0, #611). |
| `password` | string | unset | `--password` (also `--pdf-password`, docling's spelling; `--password-file PATH` reads it from a file) | `password` (also `pdf_password`) | `password` (also `pdfPassword`) | The password of an encrypted PDF — docling's `--pdf-password` (#611) — and of an encrypted Office document (`.docx`/`.xlsx`/`.pptx`/`.doc`/`.xls`/`.ppt`, #625; a docling.rs extension). `pdf_password`, docling's PDF-only name, is accepted on every surface. Python also reads docling's `PdfFormatOption(backend_options=PdfBackendOptions(password=…))`. The `pdf-text` / wasm build reads it for Office documents only. |
| `force_full_page_ocr` | bool | false | `--force-full-page-ocr` | `force_full_page_ocr` | `forceFullPageOcr` | OCR every page, discarding the text layer. docling's deprecated `--force-ocr` is this (or `--ocr-mode full_page`); it gets no alias, as docling itself retires it. |
| `no_table_former` | bool | false | `--no-table-former` (also `--no-tables`) | `do_table_structure` | `noTableFormer` | Skip TableFormer (geometric tables instead). Python: `do_table_structure` (inverted). |
| `no_text_panels` | bool | false | `--no-text-panels` | `no_text_panels` | `noTextPanels` | Disable the text-panel heuristic (#173). |
| `heading_hierarchy` | bool | false | `--heading-hierarchy` | `heading_hierarchy` | `headingHierarchy` | Infer PDF/image heading levels after assembly (#302). |
| `ocr_engine` | `ppocr` \| `tesseract` | `ppocr` (`DOCLING_RS_OCR_ENGINE`) | `--ocr-engine` | `ocr_engine` | `ocrEngine` | Which OCR engine reads scanned pages (#460). |
| `ocr_lang` | `en` \| `ch` \| BCP-47; tessdata stems under Tesseract | `en` (`DOCLING_RS_OCR_LANG`) | `--ocr-lang` | `ocr_lang` | `ocrLang` | OCR recognition language, validated against the engine it will drive. |
| `ocr_mode` | `default` \| `full_page` \| `layout_regions` \| `pdf_aware_layout_regions` | `default` | `--ocr-mode` | `ocr_mode` | `ocrMode` | Which regions feed the OCR (docling's `OcrMode`, #254). |
| `ocr_scale` | px/pt > 0 | unset = the 2.0 render | `--ocr-scale` | `ocr_scale` | `ocrScale` | OCR input scale (docling's `OcrOptions.scale`, #254). |
| `images_scale` | 0.1–4.0 px/pt | unset = the 2.0 render | `--images-scale` | `images_scale` | `imagesScale` | Picture-crop / page-image scale (docling's `images_scale`, #520). |
| `page_images` | bool | false | `--page-images` | `generate_page_images` | `pageImages` | Keep each page's render as the JSON page image (docling's `generate_page_images`, #520). Python: `generate_page_images` (also accepted as a JSON alias). |
| `do_picture_classification` | bool | false | `--enrich-picture-classes` | `do_picture_classification` | `doPictureClassification` | Enrichment: classify pictures with DocumentFigureClassifier (#423). CLI: `--enrich-picture-classes`. |
| `do_code_enrichment` | bool | false | `--enrich-code` | `do_code_enrichment` | `doCodeEnrichment` | Enrichment: rewrite code blocks with CodeFormulaV2 (#423). CLI: `--enrich-code`. |
| `do_formula_enrichment` | bool | false | `--enrich-formula` | `do_formula_enrichment` | `doFormulaEnrichment` | Enrichment: decode display formulas to LaTeX with CodeFormulaV2 (#423). CLI: `--enrich-formula`. |
| `do_picture_ocr` | bool | false | `--picture-ocr` | `do_picture_ocr` | `doPictureOcr` | Enrichment: OCR the pictures embedded in non-PDF documents (DOCX/PPTX/HTML/…, video frames) with the OCR models; the text is the picture's description annotation — Markdown prints it after the caption, JSON carries `meta.description` (#645). CLI: `--picture-ocr`. |
| `picture_ocr_classes` | comma-separated classifier labels | unset = every picture | `--picture-ocr-classes` | `picture_ocr_classes` (str or list) | `pictureOcrClasses` | With `do_picture_ocr`: only read pictures the DocumentFigureClassifier labels as one of these (its 26 classes; an unknown label is rejected). |
| `picture_ocr_min_side` | px ≥ 0 | 32 (`DOCLING_RS_PICTURE_OCR_MIN_SIDE`) | `--picture-ocr-min-side` | `picture_ocr_min_side` | `pictureOcrMinSide` | With `do_picture_ocr`: skip pictures whose smaller side is under this (icons, bullets). |
| `keep_picture_images` | bool | true | `--no-picture-images` | `keep_picture_images` | `keepPictureImages` | `false` drops the embedded image bytes from every picture after the enrichment pass: slim JSON/DCLX, placeholder-only Markdown, the OCR text kept. CLI: `--no-picture-images`. |
| `redact_pii` | bool | false | `--redact-pii` | `redact_pii` | `redactPii` | Redact personal data from the converted document before any export (#621): e-mail, phone, card (Luhn), IBAN (mod-97), IP, URL credentials, national IDs, and names / organizations / locations with the NER model. The result carries the counts (`redaction`; serve: `X-Docling-Redaction` / item `redaction`). A docling.rs extension. |
| `redact_mode` | `label` \| `pseudonym` \| `fixed:<text>` | `label` | `--redact-mode` | `redact_mode` | `redactMode` | What a span becomes: `[EMAIL]`; `[EMAIL_1]` / `[EMAIL_2]` (one number per distinct value, consistent within the document); a fixed string. |
| `redact_kinds` | comma-separated kinds | unset = every kind | `--redact-kinds` | `redact_kinds` | `redactKinds` | `email, phone, credit_card, iban, ip_address, url_credentials, national_id, person, organization, location, address`; an unknown kind is rejected. |
| `redact_pattern` | `NAME=REGEX` lines | unset | `--redact-pattern` (repeatable) | `redact_pattern` (str or list) | `redactPattern` | Extra patterns, redacted as `[NAME]` (the match or capture group 1); an invalid regex is rejected. Several entries are newline-separated on the wire. |
| `redact_images` | `drop` \| `box_out` \| `keep` | `drop` | `--redact-images` | `redact_images` | `redactImages` | Embedded images and page renders: removed; OCR'd and painted over where a line carries a value (needs the OCR models; the PDF streaming path drops instead); left alone. |
| `pipeline` | `standard` \| `vlm` | `standard` | `--pipeline` | `pipeline` | `pipeline` | The remote vision-model pipeline instead of the local ML stack (#77); the `vlm_*` options are inert under `standard`. |
| `vlm_endpoint` | URL | `DOCLING_RS_VLM_ENDPOINT` | `--vlm-endpoint` | `vlm_endpoint` | `vlmEndpoint` | OpenAI-compatible endpoint for `pipeline=vlm`. serve: a request-supplied endpoint needs `--allow-url-fetch`. |
| `vlm_model` | string | `DOCLING_RS_VLM_MODEL` | `--vlm-model` | `vlm_model` | `vlmModel` | Model name for `pipeline=vlm`. |
| `vlm_api_key` | string | `DOCLING_RS_VLM_API_KEY` | `--vlm-api-key` | `vlm_api_key` | `vlmApiKey` | Bearer token for the endpoint. |
| `vlm_prompt` | string | `DOCLING_RS_VLM_PROMPT`, else docling's default | `--vlm-prompt` | `vlm_prompt` | `vlmPrompt` | Per-page instruction. |
| `vlm_max_tokens` | int > 0 | 8192 | `--vlm-max-tokens` | `vlm_max_tokens` | `vlmMaxTokens` | `max_tokens` per completion. |

## Validation rules (shared)

Checked before any work starts, with the same message on every surface
(the CLI prints the flag spelling, Node the camelCase one):

- `pages` — `A-B` or `N`, 1-based, `A ≤ B`.
- `document_timeout`, `ocr_scale` — finite and positive.
- `images_scale` — in 0.1–4.0.
- `ocr_engine` — `ppocr` | `tesseract`; `ocr_mode` — one of the four modes.
- `ocr_lang` — a language the selected engine reads (`en`/`ch`/BCP-47 under
  PP-OCR; tessdata stems or BCP-47 tags under Tesseract).
- `pipeline` — `standard` | `vlm`; `vlm_max_tokens` — positive; under `vlm`,
  an endpoint and a model must come from the options or the
  `DOCLING_RS_VLM_*` environment.

`asr_lang`, `encoding` and `ebcdic_layout` are checked against the model,
codec or copybook when the conversion runs.

## Surface notes

- **CLI** — flags fill a `ConvertOptions`, `validate()` runs once after the
  command line is parsed; errors read `--ocr-scale must be a positive
  number, got 0`. Python `docling convert`'s spellings are accepted too
  (#611): `--page-range` (`--pages`), `--image-export-mode` (`--images`),
  `--no-tables` (`--no-table-former`), `--no-ocr` with docling's meaning,
  `--pdf-password` (= `--password`), and `--output-file PATH` — the one result written to
  exactly PATH, refused unless there is exactly one input document and one
  `--to` format (docling's checks and messages).
- **serve** — the query string, the JSON body and multipart text parts are
  three layers of the same struct (body over query, text parts over query),
  merged generically; a text value is read as the option's type (string,
  integer, number, then `1`/`true`/`yes`/`on` and `0`/`false`/`no`/`off` for
  booleans), an unreadable one is a 400 naming the option. Unknown names are
  ignored, as unknown query parameters always were. The two server-side
  policies stay in serve: `fetch_images` / `image_sources=remote` and a
  request-supplied `vlm_endpoint` need `--allow-url-fetch` (`remote` is
  held to `embedded` without it), `image_sources=local` needs
  `--allow-local-images`, `xbrl_taxonomy` must be a relative path without
  `..`.
- **Python** — the keyword arguments keep docling's spellings (`do_ocr`,
  `do_table_structure`, `page_range`, `generate_page_images`) and map onto
  the struct; a rejected option is a `ValueError` at construction.
- **Node** — the option objects (`ConverterOptions`, `ConvertOptions`,
  `StreamOptions`) serialize into the struct, so every property is honoured
  by `convertFile`, `new DocumentConverter`, `new Pipeline` and the
  streaming functions alike; a rejected option throws `InvalidArg` naming
  the property (`ocrLang`, `documentTimeout`).
- **C ABI / wasm** — one JSON object: `to`, `images` plus the wire names;
  an unknown key is an error.

## OCR engines and languages

- **Engines** (`ocr_engine`, #460). `ppocr` is the built-in PP-OCR stack
  every conformance baseline is pinned against: RapidOCR's PP-OCRv6 text
  detector (`.models/ocr_det.onnx`, `DOCLING_OCR_DET_ONNX`) finds the lines
  on a bitmap page — inside the layout regions its boxes are the
  recognizer's crops, the lines no region covers come out as orphan text
  (#570; `DOCLING_RS_OCR_LINES=projection` restores the pre-#570
  ink-projection strips) — and the recognizer reads them: PP-OCRv6
  (`.models/ocr_rec_v6.onnx`) when installed, else the PP-OCRv3 `en` / `ch`
  pairs; `DOCLING_OCR_REC_ONNX` + `DOCLING_OCR_DICT` pin a pair explicitly.
  `tesseract` runs the system `tesseract` binary — docling's
  `TesseractCliOcrOptions`: one subprocess per layout-region crop, `tsv` on
  stdout, no bindings — so any of its 100+ languages reads through the same
  pipeline (regions, orphan recovery, TableFormer word matching); orientation
  comes from Tesseract's own OSD (needs the `osd` traineddata). Install
  `tesseract-ocr` plus language packs (`tesseract --list-langs`; the serve
  image ships `eng`). `DOCLING_TESSERACT` names the binary,
  `DOCLING_RS_TESSERACT_PSM` a page segmentation mode (unset = Tesseract's
  automatic 3; 6 = one uniform block, 11 = sparse text),
  `DOCLING_RS_TESSDATA_DIR` the data directory (`TESSDATA_PREFIX` is honoured
  by Tesseract itself). A missing binary or traineddata warns and degrades
  to no OCR, like a missing model; under `force_full_page_ocr` it is an
  error. Each crop is one process, dealt across the OCR lanes
  (`DOCLING_RS_OCR_SESSIONS`), each pinned to one OpenMP thread. Python also
  maps a docling-shaped `TesseractCliOcrOptions` (`lang`, `tesseract_cmd`,
  `path`, `psm`).
- **Languages** (`ocr_lang`). Under PP-OCR: `en` (the default — the English
  PP-OCRv3 model, good Latin word spacing) or `ch` (the multilingual model
  the docling conformance corpus was generated with — use it for
  byte-for-byte comparisons; `DOCLING_RS_OCR_LANG=ch` process-wide, the
  conformance scripts pin it themselves). BCP-47 tags resolve to the same
  two (#388, docling#4075): `en-US`, `en_GB`, `eng`, `english` → `en`; `zh`,
  `zh-Hans`, `zh-CN`, `zh-TW`, `zho`, `chinese`, EasyOCR's `ch_sim` → `ch`,
  with or without docling's `iso:` prefix — script and region subtags are
  ignored; any other language (`de`, `ja`, …) is rejected (the env var
  warns and defaults). Under Tesseract: tessdata stems (`deu`, `eng+fra`,
  `script/Cyrillic`, a traineddata of your own) or BCP-47 tags mapped onto
  them (`de` → `deu`, `zh-Hant` → `chi_tra`); `en` and `ch` keep working.
- **Regions and scale** (#254). `ocr_mode` is docling's `OcrMode`: the
  default (= `pdf_aware_layout_regions`) keeps the text layer and OCRs what
  lacks one; `full_page` and `layout_regions` both discard the text layer,
  exactly like `force_full_page_ocr` (the whole-page vs per-region *detector*
  input they distinguish upstream has no analogue here: the detector always
  sees the whole page, the recognizer always reads line crops). `ocr_scale`
  is docling's `OcrOptions.scale` in px/pt: unset, OCR reads the pipeline's
  2.0 px/pt (144 dpi) render — the pinned conformance baseline — and an
  image input at docling's effective resolution (#570): 3 px/pt shrunk so
  the longer side stays within RapidOCR's 2000 px (a 754 × 1000 scan reads
  at 2.0; measured best on FUNSD: 0.825 / 0.856 / 0.836 word recall at
  1 / 2 / 3 px/pt); a set value resamples the OCR input only, layout and
  TableFormer pixels are untouched. docling's default is 3 (216 dpi); lower
  it when the source raster is already high-resolution.
- **Detector and recognizer knobs** (env): `DOCLING_RS_OCR_DET_MAX_SIDE`
  (2000 = RapidOCR's `max_side_len`; `0` lifts the cap, 960 — PaddleOCR's
  budget — is about a third of the time at ~0.02 recall on FUNSD),
  `DOCLING_RS_OCR_TEXT_SCORE` (RapidOCR's `text_score`, 0.5 — a line whose
  mean character confidence is under it is dropped; `0` keeps all),
  `DOCLING_RS_OCR_ORIENTATION=off` (no un-rotation probe of raster-rotated
  scans, #225/#571). The detector runs on bitmap pages only and
  concurrently with the layout model.
- **Picture crops and page images** (#520). `images_scale` (0.1–4.0 px/pt)
  resamples the 2.0 px/pt render for picture crops and page images (above
  2.0 upsamples, it does not re-render); each picture's `image.dpi` in the
  JSON is 72·scale (#519). `page_images` keeps every page's render, at
  `images_scale`, as the JSON `pages[n].image` (docling-core's
  `PageItem.image`), so `TableItem.get_image(doc)` / `FormulaItem.get_image(doc)`
  crop from it; off by default (a PNG per page in memory), no render under
  `text_layer_only`, none in streamed Markdown. Python also takes both
  docling-shaped through `PdfPipelineOptions`, where `images_scale` applies
  once `generate_picture_images` or `generate_page_images` is on.

## VLM pipeline

`pipeline=vlm` (#77) renders each page in Rust and sends it, with docling's
page-conversion prompt, to an OpenAI-compatible vision endpoint — LM Studio,
Ollama, vLLM, a hosted service; an image input is sent as-is. No ONNX
models load.

- `vlm_endpoint` takes the server's `/v1` base or the full
  `…/chat/completions` URL; `vlm_api_key` is sent as a Bearer token;
  `vlm_max_tokens` defaults to 8192 (#312). `DOCLING_RS_VLM_ENDPOINT` /
  `_MODEL` / `_API_KEY` / `_PROMPT` are the env fallbacks of the options.
  They supply values, never switch the pipeline on — a stale endpoint
  cannot reroute an ordinary conversion over the network — and the `vlm_*`
  options are inert under `standard` (the Node bindings ignore stray `vlm*`
  options likewise). `DOCLING_RS_VLM_TIMEOUT` (seconds, 600) caps each page
  request for slow, e.g. CPU-served, endpoints; `DOCLING_RS_VLM_EXTRA_BODY`
  merges extra JSON into every request; `DOCLING_RS_VLM_DEBUG` logs the
  exchange.
- Transient failures (timeouts, 408/429, 5xx) retry with exponential
  backoff; a page that still fails fails the conversion — no silently
  dropped pages. On serve a VLM failure (unreachable endpoint, non-200,
  unparseable answer) fails that request only.
- `pages` composes (only the window is rendered and sent); `to` and
  `strict` work as usual.
- Answer grammars are detected per response (#322): DocTags
  (granite-docling-class models), DocLang XML, Chandra layout HTML
  (`<div data-bbox=… data-label=…>` blocks — tables with spans, Form-held
  tables, lists, figures, page furniture; docling 2.123–2.125 semantics
  including `<br>`-as-spacing), Unlimited-OCR grounding output (normalized
  into the DeepSeek-OCR annotation shape) and raw DeepSeek-OCR annotated
  Markdown; plain prose degrades to text — hostile model output never
  errors. Known models get their official prompt when `vlm_prompt` is
  unset: `unlimited*` → the model card's `<image>document parsing.` (any
  other phrasing returns an empty completion) plus the
  `skip_special_tokens=false` request flag its grounding markers need;
  `chandra*` → docling's Chandra layout prompt; everything else the
  DocLang-eliciting default.
- serve: a request-supplied `vlm_endpoint` is outbound traffic steered by
  the caller, so it needs `--allow-url-fetch` and passes the same SSRF
  resolution check as URL inputs (private/loopback endpoints refused;
  `DOCLING_RS_ALLOW_PRIVATE_IP_FETCH=1` for local development). The
  operator-pinned mode — `DOCLING_RS_VLM_ENDPOINT` / `DOCLING_RS_VLM_MODEL`
  on the server, requests send only `pipeline=vlm` — needs no gate.
- Measured (#311): the PDF corpus through one granite-docling endpoint from
  docling.rs and from Python docling's `VlmPipeline` scores 87.7 % mean
  whitespace-normalized similarity over 18 fixtures, 3 byte-exact; the gap
  is dominated by each side rendering at its own scale (144 vs 216 dpi).
  Table and method in `docs/PDF_CONFORMANCE.md`; harness
  `scripts/conformance/vlm_conformance.sh` (a GPU-served endpoint — CPU
  inference measures hours per page).

## CLI batch mode

`docling-rs SOURCE… --output DIR` / `--input GLOB|DIR --output DIR` (#205,
#489): one warm process, every source a batch item.

- **Sources.** Positional files, directories (swept recursively, taking
  every file with a convertible extension — stray `.log` / `.tmp` files are
  ignored, not failures), quoted globs, and `--input` with a glob (quote it)
  or a directory. `--output` with a single file works too (a batch of one).
  Which extensions count is the binary's own list (#603): `--list-input-formats`
  prints them sorted, one per line, no dot, `zip` included;
  `--list-output-formats` the `--to` values; a format behind a cargo feature
  the build lacks (PDF/images without `pdf`, audio/video without `asr`,
  `.heic` without `heif`) is left out. Library: `InputFormat::supported_extensions()`,
  `docling::OUTPUT_FORMATS`.
- **Outputs.** `--to` is repeatable (`--to md --to json` = `--to md,json`):
  each document converts once and is written in every format named, as
  `<stem>.<ext>` (`.md`, `.json`, `.html`, `.txt`, `.dclx`, `.chunks.json`,
  `.tex`, `.pandoc.json`, `.vtt`); several formats need `--output`, since
  stdout carries one document. `--output-file PATH` (#611, docling's)
  writes the one result to exactly PATH — refused unless there is exactly
  one input and one format; `--images referenced` pictures land in
  `<stem>_artifacts/` next to it.
- **Layout** (`--output-dirs`, #496). `auto` (default): a directory or glob
  source mirrors its tree below the directory / the pattern's static
  prefix (`/data/reports/**/*.pdf` → `out/2024/q1/a.json`), a plain file
  lands by stem. `flat`: every output directly in `--output`. `mirror`:
  every input by its path relative to the current directory, explicit files
  included (`a/README.md b/README.md` → `out/a/README.md`, `out/b/README.md`);
  an input outside the current directory is refused. Two sources that would
  write the same file (`sub/b.docx` and `other/b.docx` → `b.md`) are refused
  before anything converts — pass a common parent, use `mirror`, or
  separate `--output` directories.
- **Parallelism and progress.** The PDF/image models load once and every
  file reuses the warm sessions; `--jobs N` converts declarative formats in
  parallel (PDFs already parallelize internally). Output paths print to
  stdout one per line; stderr gets `start: <file> (N pages)`, a dot every
  10 finished pages and `ok: … (12.8s, 800 ms/page)`. Pipeline diagnostics
  are quiet unless `DOCLING_RS_DEBUG=1`.
- **Failures.** A failing file is reported and skipped; the exit code is
  non-zero if anything failed; `--abort-on-error` (Python's flag) stops at
  the first. One exception: an execution-provider failure (an explicit
  `DOCLING_RS_EP` whose runtime libraries are missing) would fail every
  remaining PDF identically, so the first one aborts the batch (`fatal: …`,
  the rest `skipped`).
- **ZIP archives** (#557). A `.zip` named as a source (or matched by a glob)
  converts every document inside, each its own batch item — one broken
  document fails only itself; `out/bundle/<entry path>.<ext>`. Entries are
  listed from the central directory before anything is inflated, nothing is
  extracted to disk, and the ones that do not convert are reported
  (`skip: bundle.zip:tool.exe: unsupported file type`) and counted:
  unsupported types, nested archives (one level only), `__MACOSX` metadata,
  encrypted entries, `..` paths, and entries over the limits — 10 000
  entries, 256 MiB per entry, 1 GiB in all, a 200:1 compression ratio
  (`DOCLING_RS_ZIP_MAX_ENTRIES` / `_MAX_ENTRY_MB` / `_MAX_TOTAL_MB` /
  `_MAX_RATIO`, `ArchiveLimits::from_env`). A directory sweep does not open
  archives it finds — only explicitly named ones expand — and a lone `.zip`
  without `--output` is a usage error. Library:
  `DocumentConverter::convert_archive(reader)`, a lazy iterator of
  `Converted` / `Skipped` / `Failed` outcomes (`docling::archive`); Python
  `convert_archive`, Node `convertArchiveFile` / `convertArchive`.
- **Email attachments** (#561). An `.eml` / `.msg` converts as one document
  (headers + body, the attachment *names* with `--list-attachments`); the
  payloads are reachable from the library and bindings, not the CLI or
  serve. `docling::EmailAttachments::open(bytes, &limits)` lists them — a
  safe unique file name (`report-2.pdf` for a second `report.pdf`), media
  type, size, inline-image flag, and the format it converts as (extension,
  else media type, else the bytes) or why it will not (no payload — an
  attachment by reference or an OLE object; over a limit; a nested archive
  or unsupported type, whose bytes stay available through `data(i)`). A
  forwarded message (`message/rfc822`, an embedded `.msg` message) is an
  `.eml` entry. `DocumentConverter::convert_email_attachments(bytes)`
  converts them with the archive outcomes above under the same
  `ArchiveLimits` (no ratio: MIME cannot bomb). Python:
  `docling_rs.email_attachments(path | bytes | DocumentStream)` →
  `EmailAttachment(…, data)` with `.as_stream()` for `convert` (the stream
  carries the detected format, so a `scan.bin` sent as `application/pdf`
  converts as a PDF); Node: `emailAttachments({ name, data })` /
  `emailAttachmentsFile(path)` (+ `*Async`), then `convert({ name, data,
  format })`. Payloads are the bytes as sent — only the transfer encoding is
  undone (#564).
