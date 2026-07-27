// Web Worker host for the wasm OCR pipeline (stage 2 of #157). The heavy work
// — layout + OCR inference and the synchronous Rust pre/post-processing inside
// add_page — runs here, so the main thread stays responsive. Rasterization is
// NOT here: the main thread rasterizes with pdf.js and transfers ready RGBA
// buffers in (pdf.js in a worker falls back to a "fake worker" that garbles the
// raster).
//
// RPC: every request carries an id; the reply is {type:"ok", id, ...data} or
// {type:"error", id, msg}. Progress is a broadcast {type:"status", msg,
// spinning}. Requests: set-models{models} | boot{lang,layoutOnly} | rec{lang} |
// doc-start{lang,useTf} | doc-start-digital{bytes,useTf} |
// doc-page{rgba,w,h,scale,index} | doc-finish{name,to,images} |
// convert-image{bytes,name,lang,to,images}.

import { createOcr, THREADS } from "./pipeline.js";

const post = (type, extra) => self.postMessage({ type, ...extra });

const ocr = createOcr({
  onStatus: (msg, spinning) => post("status", { msg, spinning }),
});

async function handle(m) {
  switch (m.type) {
    case "set-models":
      ocr.setProvidedModels(m.models);
      return {};
    case "boot": {
      const kind = await ocr.boot();
      if (!kind) return { noLayout: true };
      // A digital PDF never recognises anything, so its boot skips the
      // recognition model entirely; the scanned path loads it lazily anyway.
      if (!m.layoutOnly) {
        await ocr.recFor(m.lang);
        await ocr.warmup(m.lang);
      }
      return { kind, threads: THREADS };
    }
    case "rec":
      await ocr.recFor(m.lang);
      await ocr.warmup(m.lang);
      return {};
    case "doc-start":
      await ocr.startDoc(m.lang, m.useTf);
      return {};
    case "doc-start-digital":
      return { pages: await ocr.startDigital(m.bytes, m.useTf) };
    case "doc-page":
      await ocr.addPage(new Uint8Array(m.rgba), m.w, m.h, m.scale, m.index);
      return {};
    case "doc-finish":
      return { md: ocr.finishDoc(m.name, m.to, m.images) };
    case "convert-image":
      return { md: await ocr.convertImage(m.bytes, m.name, m.lang, m.to, m.images) };
    default:
      throw new Error(`unknown request ${m.type}`);
  }
}

self.onmessage = async (e) => {
  const m = e.data;
  try {
    const r = (await handle(m)) || {};
    post("ok", { id: m.id, ...r });
  } catch (err) {
    post("error", { id: m.id, msg: String((err && err.message) || err) });
  }
};

post("up");
