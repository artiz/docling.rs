// wasm-side OCR pipeline (stage 2 of #157), used inside worker.js (default) or
// on the main thread (fallback). It owns ONLY the wasm work — model loading,
// per-page `add_page`, image decode. Rasterization stays on the MAIN thread
// (pdf.js + a real HTMLCanvas, which produces correct pixels and uses pdf.js's
// own worker); pages arrive here as ready RGBA buffers. Keeping pdf.js off the
// Web Worker avoids its "fake worker" fallback, which garbled the line crops.
//
// DOM-free: the only host concern is onStatus(msg, spinning) for progress.

import * as ort from "https://cdn.jsdelivr.net/npm/onnxruntime-web/dist/ort.min.mjs";
import init, { ScannedConverter, convert_scanned_image } from "./pkg/docling_wasm.js";

// Multi-threaded wasm when cross-origin isolated (coi.js); else one thread.
ort.env.wasm.numThreads = self.crossOriginIsolated
  ? Math.min(navigator.hardwareConcurrency || 4, 8)
  : 1;
export const THREADS = ort.env.wasm.numThreads;

// Model bases, tried in order after any user-provided file: local ./models/
// first (download_dependencies.sh for local dev), then a CORS-enabled Hugging
// Face mirror so the page works cross-origin (the hosted phone demo on
// raw.githack) — GitHub Release assets carry no CORS header.
const MODEL_BASE = "https://huggingface.co/pivozavrus/docling-rs-models/resolve/main/";

const REC_MODELS = {
  en: {
    model: "https://huggingface.co/SWHL/RapidOCR/resolve/main/PP-OCRv3/en_PP-OCRv3_rec_infer.onnx",
    dict: "https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/main/ppocr/utils/en_dict.txt",
  },
  cyrillic: {
    // PP-OCRv5: markedly better Cyrillic accuracy than the v3 export (spaces
    // and case survive); its dictionary only exists inside the repo's
    // inference.yml, so a flattened copy ships next to this page.
    model: "https://huggingface.co/PaddlePaddle/cyrillic_PP-OCRv5_mobile_rec_onnx/resolve/main/inference.onnx",
    dict: "cyrillic_v5_dict.txt",
  },
  ch: {
    model: "https://huggingface.co/SWHL/RapidOCR/resolve/main/PP-OCRv3/ch_PP-OCRv3_rec_infer.onnx",
    dict: "https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/main/ppocr/utils/ppocr_keys_v1.txt",
  },
};

// TableFormer graphs: local ./models/tableformer/ first (download_dependencies.sh),
// then the CORS Hugging Face mirror for the hosted demo. The encoder is
// self-contained; the decoder/bbox use ONNX external data, so their .onnx.data
// sidecar rides along via ort-web's externalData option (path = the location
// stored in the .onnx). Fetched lazily — ~380 MB total — only when a table
// profile is chosen.
const TF_DIRS = ["./models/tableformer/", MODEL_BASE];

/// The stateful session the wasm TfSession interop expects (see
/// src/tableformer.rs): `encode` runs the image encoder once and stashes the
/// constant cross-attention K/V + enc_out and resets the KV-cache; `step` runs
/// one decoder step feeding the stored cross + growing cache; `bbox` runs the
/// bbox decoder. The heavy tensors stay here — only tags and logits/hidden
/// cross the wasm boundary. Geometry: 6 layers, 8 KV heads, head_dim 64.
class JsTfSession {
  constructor(enc, dec, bbox) {
    this.enc = enc;
    this.dec = dec;
    this.bboxSess = bbox;
    this.cross = null;
    this.encOut = null;
    this.cacheK = null;
    this.cacheV = null;
  }
  async encode(image) {
    const out = await this.enc.run({ image: new ort.Tensor("float32", image, [1, 3, 448, 448]) });
    this.cross = {};
    for (let i = 0; i < 6; i++) {
      this.cross["cross_kt_" + i] = out["cross_kt_" + i];
      this.cross["cross_v_" + i] = out["cross_v_" + i];
    }
    this.encOut = out["enc_out"];
    // Empty first-step KV-cache: [N_LAYERS, 1, KV_HEADS, past=0, head_dim].
    this.cacheK = new ort.Tensor("float32", new Float32Array(0), [6, 1, 8, 0, 64]);
    this.cacheV = new ort.Tensor("float32", new Float32Array(0), [6, 1, 8, 0, 64]);
  }
  async step(tag) {
    const out = await this.dec.run({
      tag: new ort.Tensor("int64", BigInt64Array.from([BigInt(tag)]), [1, 1]),
      cache_k: this.cacheK,
      cache_v: this.cacheV,
      ...this.cross,
    });
    this.cacheK = out["out_cache_k"];
    this.cacheV = out["out_cache_v"];
    return { logits: out["logits"].data, hidden: out["hidden"].data };
  }
  async bbox(tagH, n) {
    const out = await this.bboxSess.run({
      enc_out: this.encOut,
      tag_h: new ort.Tensor("float32", tagH, [n, 512]),
    });
    return { boxes: out["boxes"].data, classes: out["classes"].data };
  }
}

export function createOcr({ onStatus }) {
  const status = (msg, spinning = true) => onStatus && onStatus(msg, spinning);

  // Model files the user picked from the device (basename → ArrayBuffer),
  // used instead of any network fetch — see index.html's model picker. Reading a
  // File to an ArrayBuffer is a single allocation, so it also sidesteps the
  // double-buffering peak fetchProgress hits on the 225 MB encoder.
  let provided = {};
  function setProvidedModels(map) {
    provided = map || {};
  }
  // Resolve a model by name: a user-provided file wins, else the first base
  // that serves it. Returns { buf, base } (base is null for a provided file).
  async function fetchModel(name, bases, label) {
    if (provided[name]) return { buf: provided[name], base: null };
    let lastErr = null;
    for (const b of bases) {
      try {
        return { buf: await fetchProgress(b + name, label), base: b };
      } catch (e) {
        lastErr = e;
      }
    }
    throw new Error(`${name} failed: ${(lastErr && lastErr.message) || lastErr}`);
  }

  // fetch() with a live "x / y MB" progress line.
  async function fetchProgress(url, label) {
    const resp = await fetch(url, { cache: "force-cache" });
    if (!resp.ok) throw new Error(`${label}: HTTP ${resp.status}`);
    const total = Number(resp.headers.get("Content-Length")) || 0;
    if (!resp.body) return resp.arrayBuffer();
    const reader = resp.body.getReader();
    const chunks = [];
    let got = 0;
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      chunks.push(value);
      got += value.length;
      const mb = (got / 1048576).toFixed(1);
      status(total ? `${label} — ${mb} / ${(total / 1048576).toFixed(1)} MB` : `${label} — ${mb} MB`, true);
    }
    const buf = new Uint8Array(got);
    let off = 0;
    for (const c of chunks) { buf.set(c, off); off += c.length; }
    return buf.buffer;
  }

  const recCache = {};
  async function recFor(lang) {
    if (!recCache[lang]) {
      const [model, dict] = await Promise.all([
        fetchProgress(REC_MODELS[lang].model, `${lang} recognition model`),
        fetch(REC_MODELS[lang].dict, { cache: "force-cache" }).then((r) => r.text()),
      ]);
      status(`starting ${lang} recognition session …`, true);
      const session = await ort.InferenceSession.create(model, {
        executionProviders: ["wasm"],
        logSeverityLevel: 3,
      });
      recCache[lang] = {
        dict,
        rec: {
          run: async (n, h, w, data) => {
            const results = await session.run({
              [session.inputNames[0]]: new ort.Tensor("float32", data, [n, 3, h, w]),
            });
            const t = results[session.outputNames[0]];
            return { data: t.data, dims: Array.from(t.dims) };
          },
        },
      };
    }
    return recCache[lang];
  }

  // Interop wrapper docling_wasm expects (see src/scanned.rs).
  let layout = null;
  let layoutKind = null;
  async function loadLayout() {
    for (const [name, kind] of [
      ["layout_heron_int8.onnx", "int8"],
      ["layout_heron.onnx", "fp32"],
    ]) {
      let buf;
      try {
        ({ buf } = await fetchModel(name, ["./models/", MODEL_BASE], "layout model (first load only)"));
      } catch (e) {
        continue; // not provided and not fetchable → try the next candidate
      }
      status("starting layout session …", true);
      const session = await ort.InferenceSession.create(buf, {
        executionProviders: ["wasm"],
        logSeverityLevel: 3,
      });
      layoutKind = kind;
      layout = {
        run: async (data) => {
          const results = await session.run({
            pixel_values: new ort.Tensor("float32", data, [1, 3, 640, 640]),
          });
          const t = (n) => ({ data: results[n].data, dims: Array.from(results[n].dims) });
          return { logits: t("logits"), boxes: t("pred_boxes") };
        },
      };
      return layoutKind;
    }
    return null;
  }

  // Bring wasm + the layout model up. Returns "int8" | "fp32" | null.
  async function boot() {
    status("loading wasm module …", true);
    await init();
    return loadLayout();
  }

  // One blank inference through layout + rec so ORT's lazy kernel/thread init
  // happens now, not inside the first real page.
  async function warmup(lang) {
    try {
      await layout.run(new Float32Array(3 * 640 * 640));
      const { rec } = await recFor(lang);
      await rec.run(1, 48, 320, new Float32Array(3 * 48 * 320));
    } catch (e) {
      // best-effort — a failure just means the first page pays it.
    }
  }

  // TableFormer sessions, loaded lazily on first use (the encoder alone is
  // ~225 MB, so this only downloads when the table profile is chosen).
  let tf = null;
  async function ensureTf() {
    if (tf) return tf;
    const load = async (name, external) => {
      // A provided file wins; else the first base (local, then HF). External
      // data comes from a provided file or the same base the .onnx came from.
      const { buf: model, base } = await fetchModel(name + ".onnx", TF_DIRS, `tableformer ${name}`);
      const opts = { executionProviders: ["wasm"], logSeverityLevel: 3 };
      if (external) {
        const { buf: data } = await fetchModel(
          name + ".onnx.data",
          base ? [base] : TF_DIRS,
          `tableformer ${name} data`,
        );
        opts.externalData = [{ path: name + ".onnx.data", data: new Uint8Array(data) }];
      }
      return ort.InferenceSession.create(model, opts);
    };
    const enc = await load("encoder", false);
    const dec = await load("decoder_kv", true);
    const bbox = await load("bbox", true);
    status("starting tableformer sessions …", true);
    tf = new JsTfSession(enc, dec, bbox);
    return tf;
  }

  // Multi-page document lifecycle: startDoc → addPage* → finishDoc. Pages come
  // in as RGBA (already rasterized on the main thread), one document at a time.
  let cur = null;
  async function startDoc(lang, useTf) {
    const { dict, rec } = await recFor(lang);
    const tfSess = useTf ? await ensureTf() : null;
    cur = { conv: new ScannedConverter(dict), rec, tf: tfSess };
  }
  async function addPage(rgba, w, h, scale) {
    // Guard the single in-flight document: without it a stray addPage/finish
    // after (or before) the lifecycle reads `cur` as null and the host sees an
    // opaque "Cannot read properties of null".
    if (!cur) throw new Error("addPage called with no document open (startDoc first)");
    if (cur.tf) {
      await cur.conv.addPageTf(rgba, w, h, scale, layout, cur.rec, cur.tf);
    } else {
      await cur.conv.add_page(rgba, w, h, scale, layout, cur.rec);
    }
  }
  function finishDoc(name, to, images) {
    if (!cur) throw new Error("finishDoc called with no document open (startDoc first)");
    const md = cur.conv.finish(name, to || "md", images || "placeholder");
    cur = null;
    return md;
  }

  // Standalone image: the wasm side decodes it (no canvas needed).
  async function convertImage(bytes, name, lang, to, images) {
    const { dict, rec } = await recFor(lang);
    return convert_scanned_image(
      new Uint8Array(bytes), name, dict, layout, rec, to || "md", images || "placeholder",
    );
  }

  return {
    boot, warmup, recFor, startDoc, addPage, finishDoc, convertImage, setProvidedModels,
    get layoutKind() { return layoutKind; },
  };
}
