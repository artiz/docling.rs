// Public entry point for the `docling.rs` npm package.
//
// Wraps the native N-API binding (loaded by `native.js`, which picks the right
// prebuilt `.node` for the host platform) with two things:
//   1. dependency guards — converting a PDF/image/METS input throws a clear
//      error unless the ML models are on disk (see
//      scripts/install/download_dependencies.sh);
//   2. `streamFileMarkdown` async generators over Markdown chunks (module-level
//      and on the warm `Pipeline`);
//   3. pluggable backends (#680): `registerBackend` claims inputs by
//      extension, media type or content for a JavaScript function, ahead of
//      the built-in formats.
//
// Works in Node.js and Bun (Bun implements N-API).

'use strict'

const fs = require('node:fs')
const nodePath = require('node:path')
const native = require('./native.js')
const { checkDependencies, assertMlReady, defaultChunkTokenizer } = require('./deps.js')

// Resolve the format id of an input for the ML guard. Uses the native
// extension→format map; falls back to an explicitly-passed format string.
function mlFormatOf(name, format) {
  if (format) {
    return native.formatFromName(`x.${String(format).replace(/^\./, '')}`) || String(format)
  }
  return native.formatFromName(name || '') || ''
}

// Bridge a native (err, chunk) streaming callback into an async generator.
// `start` receives the callback and kicks off the native conversion. Chunks
// are delivered on the event loop (via a threadsafe function); a null chunk
// ends the stream, a non-null err ends it with a throw.
async function* chunkStream(start) {
  const queue = []
  let done = false
  let failure = null
  let notify = null
  const wake = () => {
    if (notify) {
      const n = notify
      notify = null
      n()
    }
  }

  start((err, chunk) => {
    if (err) {
      failure = err
      done = true
    } else if (chunk === null || chunk === undefined) {
      done = true
    } else {
      queue.push(chunk)
    }
    wake()
  })

  while (true) {
    if (queue.length > 0) {
      yield queue.shift()
      continue
    }
    if (failure) throw failure
    if (done) return
    await new Promise((resolve) => {
      notify = resolve
    })
  }
}

// --- pluggable backends (#680) ----------------------------------------------

// The backend trait runs inside Rust with no JavaScript environment at hand,
// so the registry lives here: a claimed input never reaches the native
// dispatch; its bytes go to the registered function, and what that returns —
// Markdown, or a DoclingDocument as an object — is converted by the native
// side as `md` / `json_docling`, under the caller's options, so the output
// options, redaction and the rest apply as for any other format.

const globalBackends = []
let registrations = 0

function extensionOf(name) {
  const m = /\.([^./\\]+)$/.exec(String(name || ''))
  return m ? m[1].toLowerCase() : ''
}

function essence(mediaType) {
  return String(mediaType || '').split(';')[0].trim().toLowerCase()
}

function stemOf(name) {
  const base = nodePath.basename(String(name || ''))
  const dot = base.lastIndexOf('.')
  return dot > 0 ? base.slice(0, dot) : base
}

function stringList(value, what) {
  if (value === undefined || value === null) return []
  if (typeof value === 'string') throw new TypeError(`registerBackend: ${what} is a list of strings, not one string`)
  return Array.from(value, String)
}

function backendSpec(spec) {
  if (!spec || typeof spec.convert !== 'function') {
    throw new TypeError('registerBackend needs a convert(data, name) function')
  }
  const extensions = stringList(spec.extensions, 'extensions')
    .map((e) => e.trim().replace(/^\./, '').toLowerCase())
    .filter(Boolean)
  const mimeTypes = stringList(spec.mimeTypes, 'mimeTypes').map(essence).filter(Boolean)
  if (spec.sniff !== undefined && spec.sniff !== null && typeof spec.sniff !== 'function') {
    throw new TypeError('registerBackend: sniff is a function (data) => boolean')
  }
  const sniff = spec.sniff || null
  if (!extensions.length && !mimeTypes.length && !sniff) {
    throw new Error('registerBackend: the backend claims nothing; give extensions, mimeTypes or sniff')
  }
  return Object.freeze({
    name: String(spec.name || extensions[0] || mimeTypes[0] || 'backend'),
    extensions,
    mimeTypes,
    sniff,
    convert: spec.convert,
    priority: Number(spec.priority) || 0,
    seq: registrations++,
  })
}

// Highest priority first; among equals, the earliest registration.
function addBackend(list, spec) {
  const backend = backendSpec(spec)
  list.push(backend)
  list.sort((a, b) => b.priority - a.priority || a.seq - b.seq)
  return backend.name
}

function describe(list) {
  return list.map(({ name, extensions, mimeTypes, sniff, priority }) => ({
    name,
    extensions: [...extensions],
    mimeTypes: [...mimeTypes],
    sniff: Boolean(sniff),
    priority,
  }))
}

/**
 * Register a backend for every module-level function and every
 * `DocumentConverter` created afterwards (#680). See index.d.ts.
 */
function registerBackend(spec) {
  return addBackend(globalBackends, spec)
}

function registeredBackends() {
  return describe(globalBackends)
}

function unregisterBackend(name) {
  const before = globalBackends.length
  for (let i = globalBackends.length - 1; i >= 0; i--) {
    if (globalBackends[i].name === name) globalBackends.splice(i, 1)
  }
  return globalBackends.length !== before
}

// The backend that claims an input, and its bytes; null when none does.
// `bytes` is a Buffer or a function reading it, called at most once and only
// when a backend needs the bytes (a content check, or the claim itself).
function claim(backends, name, mimeType, bytes) {
  if (!backends.length) return null
  let data = typeof bytes === 'function' ? null : bytes
  const read = () => (data === null ? (data = bytes()) : data)
  const ext = extensionOf(name)
  const media = essence(mimeType)
  for (const backend of backends) {
    let claimed = (ext && backend.extensions.includes(ext)) || (media && backend.mimeTypes.includes(media))
    if (!claimed && backend.sniff) {
      try {
        claimed = Boolean(backend.sniff(read()))
      } catch {
        claimed = false
      }
    }
    if (claimed) return { backend, data: read(), name }
  }
  return null
}

function inputClaim(backends, input) {
  if (!input || !backends.length) return null
  const name = input.name || ''
  // An explicit `format` names the format as an extension would.
  const named = input.format && !extensionOf(name) ? `${name}.${String(input.format).replace(/^\./, '')}` : name
  return claim(backends, named, input.mimeType, Buffer.from(input.data || []))
}

function fileClaim(backends, path) {
  if (!backends.length) return null
  return claim(backends, nodePath.basename(String(path)), undefined, () => fs.readFileSync(path))
}

// What a backend returned, as the native input that reads it.
function backendInput(backend, out, name) {
  const stem = stemOf(name)
  if (typeof out === 'string') {
    return { name: `${stem}.md`, data: Buffer.from(out, 'utf8'), format: 'md' }
  }
  if (out && typeof out === 'object' && !Buffer.isBuffer(out)) {
    return { name: `${stem}.json`, data: Buffer.from(JSON.stringify(out), 'utf8'), format: 'json_docling' }
  }
  throw new TypeError(
    `backend \`${backend.name}\` returned ${out === null ? 'null' : typeof out}: ` +
      'a backend returns Markdown (a string) or a DoclingDocument object',
  )
}

function backendError(backend, err) {
  const wrapped = new Error(`backend \`${backend.name}\`: ${err && err.message ? err.message : err}`)
  wrapped.cause = err
  return wrapped
}

// The result reports the input's own format id when it has a built-in one
// (a backend taking over `.ipynb` still converts an `ipynb`), else `plugin`.
function claimedResult(claimed, result) {
  return {
    ...result,
    format: native.formatFromName(claimed.name) || 'plugin',
    inputName: stemOf(claimed.name),
  }
}

function runClaimed(claimed, convertNative) {
  const { backend } = claimed
  let out
  try {
    out = backend.convert(claimed.data, claimed.name)
  } catch (err) {
    throw backendError(backend, err)
  }
  if (out && typeof out.then === 'function') {
    throw new Error(`backend \`${backend.name}\` is async: use convertAsync / convertFileAsync`)
  }
  return claimedResult(claimed, convertNative(backendInput(backend, out, claimed.name)))
}

async function runClaimedAsync(claimed, convertNative) {
  const { backend } = claimed
  let out
  try {
    out = await backend.convert(claimed.data, claimed.name)
  } catch (err) {
    throw backendError(backend, err)
  }
  return claimedResult(claimed, await convertNative(backendInput(backend, out, claimed.name)))
}

// Chunking a claimed input: its docling-core JSON through chunkDocument.
function jsonOptions(options) {
  return { ...(options || {}), to: 'json' }
}

// --- guarded one-shot functions --------------------------------------------

// The guard takes `options` so `pipeline: 'vlm'` can drop the ONNX-model
// requirement (#77) — see assertMlReady in deps.js. The chunk* functions below
// take ChunkOptions, which carry no `pipeline`, and always use the ML pipeline.
// A registered backend comes first and needs no models: the guard only runs
// for what reaches the native dispatch.
function convertFile(path, options) {
  const claimed = fileClaim(globalBackends, path)
  if (claimed) return runClaimed(claimed, (input) => native.convert(input, options))
  assertMlReady(mlFormatOf(path), undefined, options)
  return native.convertFile(path, options)
}

function convert(input, options) {
  const claimed = inputClaim(globalBackends, input)
  if (claimed) return runClaimed(claimed, (input) => native.convert(input, options))
  assertMlReady(mlFormatOf(input && input.name, input && input.format), undefined, options)
  return native.convert(input, options)
}

// A progress callback (#783) as the native layer takes it: the engine hands
// each event up as JSON text through a threadsafe `(err, json)` function;
// the user's `(event) => void` gets the parsed event.
function progressBridge(progress) {
  if (progress == null) return undefined
  if (typeof progress !== 'function') throw new TypeError('progress must be a function')
  return (err, json) => {
    if (!err) progress(JSON.parse(json))
  }
}

// async so a guard failure surfaces as a rejected promise, not a sync throw.
async function convertFileAsync(path, options, progress) {
  const claimed = fileClaim(globalBackends, path)
  if (claimed) return runClaimedAsync(claimed, (input) => native.convertAsync(input, options))
  assertMlReady(mlFormatOf(path), undefined, options)
  return native.convertFileAsync(path, options, progressBridge(progress))
}

async function convertAsync(input, options, progress) {
  const claimed = inputClaim(globalBackends, input)
  if (claimed) return runClaimedAsync(claimed, (input) => native.convertAsync(input, options))
  assertMlReady(mlFormatOf(input && input.name, input && input.format), undefined, options)
  return native.convertAsync(input, options, progressBridge(progress))
}

// --- archives (#557) --------------------------------------------------------

// No ML guard: a ZIP may hold anything, and the native side reports a PDF
// entry that cannot convert (models missing) as that entry's `failed` item,
// never by throwing — the archive's other documents still convert.
function convertArchiveFile(path, options) {
  return native.convertArchiveFile(path, options)
}

function convertArchive(input, options) {
  return native.convertArchive(input, options)
}

async function convertArchiveFileAsync(path, options) {
  return native.convertArchiveFileAsync(path, options)
}

async function convertArchiveAsync(input, options) {
  return native.convertArchiveAsync(input, options)
}

// --- guarded chunking functions ---------------------------------------------

// For the hybrid chunker with no explicit tokenizer, resolve the default one
// (.models/chunk/tokenizer.json) through the same install-home logic as the ML
// models — so DOCLING_RS_HOME / ~/.cache installs work, not only ./.models. The
// native side keeps its own CWD-relative fallback as a backstop.
function withDefaultTokenizer(options) {
  if (!options || options.tokenizer) return options
  if (String(options.chunker || '').toLowerCase() !== 'hybrid') return options
  const tokenizer = defaultChunkTokenizer()
  return tokenizer ? { ...options, tokenizer } : options
}

// A claimed input is converted by its backend to docling-core JSON, then
// chunked like any converted document.
function chunkClaimed(claimed, options) {
  const json = runClaimed(claimed, (input) => native.convert(input, jsonOptions(options))).content
  return native.chunkDocument(json, withDefaultTokenizer(options))
}

async function chunkClaimedAsync(claimed, options) {
  const doc = await runClaimedAsync(claimed, (input) => native.convertAsync(input, jsonOptions(options)))
  return native.chunkDocumentAsync(doc.content, withDefaultTokenizer(options))
}

function chunkFile(path, options) {
  const claimed = fileClaim(globalBackends, path)
  if (claimed) return chunkClaimed(claimed, options)
  assertMlReady(mlFormatOf(path))
  return native.chunkFile(path, withDefaultTokenizer(options))
}

function chunk(input, options) {
  const claimed = inputClaim(globalBackends, input)
  if (claimed) return chunkClaimed(claimed, options)
  assertMlReady(mlFormatOf(input && input.name, input && input.format))
  return native.chunk(input, withDefaultTokenizer(options))
}

// async so a guard failure surfaces as a rejected promise, not a sync throw.
async function chunkFileAsync(path, options) {
  const claimed = fileClaim(globalBackends, path)
  if (claimed) return chunkClaimedAsync(claimed, options)
  assertMlReady(mlFormatOf(path))
  return native.chunkFileAsync(path, withDefaultTokenizer(options))
}

async function chunkAsync(input, options) {
  const claimed = inputClaim(globalBackends, input)
  if (claimed) return chunkClaimedAsync(claimed, options)
  assertMlReady(mlFormatOf(input && input.name, input && input.format))
  return native.chunkAsync(input, withDefaultTokenizer(options))
}

function chunkDocument(documentJson, options) {
  return native.chunkDocument(documentJson, withDefaultTokenizer(options))
}

async function chunkDocumentAsync(documentJson, options) {
  return native.chunkDocumentAsync(documentJson, withDefaultTokenizer(options))
}

// --- guarded classes --------------------------------------------------------

class DocumentConverter {
  constructor(options) {
    // Only the pipeline name, not the options object: the guard needs nothing
    // else, and holding the caller's object would both retain `vlmApiKey` on an
    // enumerable property (any logger that serializes the instance would print
    // the token) and let a later mutation of that object desync the guard from
    // the config the native side already resolved.
    this._pipeline = options && options.pipeline
    this._inner = new native.DocumentConverter(options)
    // The module's registrations as of now (#680), plus this instance's own.
    this._backends = [...globalBackends]
  }

  /** Register a backend on this converter only (#680); see `registerBackend`. */
  registerBackend(spec) {
    return addBackend(this._backends, spec)
  }

  registeredBackends() {
    return describe(this._backends)
  }

  convertFile(path, options) {
    const claimed = fileClaim(this._backends, path)
    if (claimed) return runClaimed(claimed, (input) => this._inner.convert(input, options))
    assertMlReady(mlFormatOf(path), undefined, { pipeline: this._pipeline })
    return this._inner.convertFile(path, options)
  }

  convert(input, options) {
    const claimed = inputClaim(this._backends, input)
    if (claimed) return runClaimed(claimed, (input) => this._inner.convert(input, options))
    assertMlReady(mlFormatOf(input && input.name, input && input.format), undefined, {
      pipeline: this._pipeline,
    })
    return this._inner.convert(input, options)
  }

  async convertFileAsync(path, options, progress) {
    const claimed = fileClaim(this._backends, path)
    if (claimed) return runClaimedAsync(claimed, (input) => this._inner.convertAsync(input, options))
    assertMlReady(mlFormatOf(path), undefined, { pipeline: this._pipeline })
    return this._inner.convertFileAsync(path, options, progressBridge(progress))
  }

  async convertAsync(input, options, progress) {
    const claimed = inputClaim(this._backends, input)
    if (claimed) return runClaimedAsync(claimed, (input) => this._inner.convertAsync(input, options))
    assertMlReady(mlFormatOf(input && input.name, input && input.format), undefined, {
      pipeline: this._pipeline,
    })
    return this._inner.convertAsync(input, options, progressBridge(progress))
  }

  // A claimed file arrives as one chunk, the way a VLM conversion does.
  convertFileStreaming(path, callback, options) {
    const claimed = fileClaim(this._backends, path)
    if (claimed) {
      runClaimedAsync(claimed, (input) => this._inner.convertAsync(input, options)).then(
        (result) => {
          callback(null, result.content)
          callback(null, null)
        },
        (err) => callback(err),
      )
      return
    }
    assertMlReady(mlFormatOf(path), undefined, { pipeline: this._pipeline })
    return this._inner.convertFileStreaming(path, callback, options)
  }

  // Archives (#557): unguarded, see convertArchiveFile above.
  convertArchiveFile(path, options) {
    return this._inner.convertArchiveFile(path, options)
  }

  convertArchive(input, options) {
    return this._inner.convertArchive(input, options)
  }

  async convertArchiveFileAsync(path, options) {
    return this._inner.convertArchiveFileAsync(path, options)
  }

  async convertArchiveAsync(input, options) {
    return this._inner.convertArchiveAsync(input, options)
  }
}

// The warm PDF/image pipeline is inherently ML — always guarded.
class Pipeline {
  constructor(options) {
    this._inner = new native.Pipeline(options)
  }

  convertFile(path, options) {
    assertMlReady(mlFormatOf(path))
    return this._inner.convertFile(path, options)
  }

  convert(input, options) {
    assertMlReady(mlFormatOf(input && input.name, input && input.format))
    return this._inner.convert(input, options)
  }

  // async so a guard failure surfaces as a rejected promise, not a sync throw.
  async convertFileAsync(path, options, progress) {
    assertMlReady(mlFormatOf(path))
    return this._inner.convertFileAsync(path, options, progressBridge(progress))
  }

  async convertAsync(input, options, progress) {
    assertMlReady(mlFormatOf(input && input.name, input && input.format))
    return this._inner.convertAsync(input, options, progressBridge(progress))
  }

  convertFileStreaming(path, callback, options) {
    assertMlReady(mlFormatOf(path))
    return this._inner.convertFileStreaming(path, callback, options)
  }

  /**
   * Stream a PDF's Markdown in chunks through the warm pipeline, in document
   * order, as pages finish converting (an image arrives as a single chunk).
   * Same contract as the module-level `streamFileMarkdown`, but reusing this
   * instance's loaded models — no per-call model reload.
   *
   * @param {string} filePath
   * @param {object} [options] output options (`imageMode`: `placeholder` or
   *   `embedded`; `referenced` is rejected; `pageBreakPlaceholder`: text
   *   between pages)
   * @returns {AsyncGenerator<string, void, unknown>}
   */
  async *streamFileMarkdown(filePath, options = {}) {
    assertMlReady(mlFormatOf(filePath))
    const { imageMode, artifactsDir, pageBreakPlaceholder } = options
    yield* chunkStream((callback) =>
      this._inner.convertFileStreaming(filePath, callback, {
        imageMode,
        artifactsDir,
        pageBreakPlaceholder,
      }),
    )
  }
}

// --- streaming --------------------------------------------------------------

/**
 * Stream a file's Markdown in chunks, in document order, as conversion
 * progresses — the win for PDF, whose pages convert in parallel.
 *
 * Yields each Markdown chunk; concatenating every chunk reproduces the buffered
 * `convertFile(path).content` byte-for-byte.
 *
 * @param {string} filePath
 * @param {object} [options]
 * @returns {AsyncGenerator<string, void, unknown>}
 */
async function* streamFileMarkdown(filePath, options = {}) {
  const { imageMode, artifactsDir, pageBreakPlaceholder } = options
  // A registered backend's document arrives as one chunk (#680).
  const claimed = fileClaim(globalBackends, filePath)
  if (claimed) {
    const result = await runClaimedAsync(claimed, (input) =>
      native.convertAsync(input, { ...options, to: 'markdown' }),
    )
    yield result.content
    return
  }
  assertMlReady(mlFormatOf(filePath), undefined, options)
  // The whole object, not a hand-copied allowlist: napi reads the
  // `ConverterOptions` fields it knows and ignores the rest (`imageMode`,
  // `artifactsDir`, `pageBreakPlaceholder`), so every converter knob reaches the stream instead of the
  // three that used to be copied by hand. `pages` above all — under
  // `pipeline: 'vlm'` it decides how many pages are rasterized and POSTed to a
  // metered endpoint, and dropping it would quietly send the whole document.
  //
  // Under `pipeline: 'vlm'` there is nothing to stream — the endpoint answers a
  // whole page per request — so the document arrives as a single chunk. The
  // contract holds either way: concatenating the chunks reproduces the buffered
  // `convertFile(...).content` byte-for-byte.
  const converter = new native.DocumentConverter(options)
  yield* chunkStream((callback) =>
    converter.convertFileStreaming(filePath, callback, {
      imageMode,
      artifactsDir,
      pageBreakPlaceholder,
    }),
  )
}

/**
 * Stream a file's chunks as the chunkers produce them — the streaming
 * counterpart of `chunkFile`. The first chunk is ready (e.g. for embedding)
 * while the rest of the document is still being chunked, and no all-chunks
 * array is materialized. Abandoning the generator early (`break`) cancels the
 * background chunking.
 *
 * @param {string} filePath
 * @param {object} [options] same `ChunkOptions` as `chunkFile`
 * @returns {AsyncGenerator<Chunk, void, unknown>}
 */
async function* streamFileChunks(filePath, options = {}) {
  assertMlReady(mlFormatOf(filePath))
  const opts = withDefaultTokenizer(options)
  yield* chunkStream((callback) => native.chunkFileStreaming(filePath, callback, opts))
}

/**
 * Streaming counterpart of `chunk`: chunk in-memory bytes, yielding each chunk
 * as it is produced (same contract as {@link streamFileChunks}).
 *
 * @param {object} input same `ConvertInput` as `chunk`
 * @param {object} [options] same `ChunkOptions` as `chunk`
 * @returns {AsyncGenerator<Chunk, void, unknown>}
 */
async function* streamChunks(input, options = {}) {
  assertMlReady(mlFormatOf(input && input.name, input && input.format))
  const opts = withDefaultTokenizer(options)
  yield* chunkStream((callback) => native.chunkStreaming(input, callback, opts))
}

/**
 * Streaming counterpart of `chunkDocument`: chunk an already-converted
 * docling-core JSON document, yielding each chunk as it is produced (same
 * contract as {@link streamFileChunks}). Touches no ML models — unguarded.
 *
 * @param {string} documentJson
 * @param {object} [options] same `ChunkOptions` as `chunkDocument`
 * @returns {AsyncGenerator<Chunk, void, unknown>}
 */
async function* streamDocumentChunks(documentJson, options = {}) {
  const opts = withDefaultTokenizer(options)
  yield* chunkStream((callback) => native.chunkDocumentStreaming(documentJson, callback, opts))
}

// --- exports (explicit, so ESM named imports work in Node and Bun) ----------

module.exports.convert = convert
module.exports.convertFile = convertFile
module.exports.convertAsync = convertAsync
module.exports.convertFileAsync = convertFileAsync
module.exports.convertArchive = convertArchive
module.exports.convertArchiveFile = convertArchiveFile
module.exports.convertArchiveAsync = convertArchiveAsync
module.exports.convertArchiveFileAsync = convertArchiveFileAsync
module.exports.chunk = chunk
module.exports.chunkFile = chunkFile
module.exports.chunkAsync = chunkAsync
module.exports.chunkFileAsync = chunkFileAsync
// Chunking an already-converted JSON document touches no ML models — unguarded.
module.exports.chunkDocument = chunkDocument
module.exports.chunkDocumentAsync = chunkDocumentAsync
module.exports.DocumentConverter = DocumentConverter
module.exports.Pipeline = Pipeline
module.exports.streamFileMarkdown = streamFileMarkdown
module.exports.streamFileChunks = streamFileChunks
module.exports.streamChunks = streamChunks
module.exports.streamDocumentChunks = streamDocumentChunks
module.exports.checkDependencies = checkDependencies
module.exports.registerBackend = registerBackend
module.exports.registeredBackends = registeredBackends
module.exports.unregisterBackend = unregisterBackend
module.exports.supportedFormats = native.supportedFormats
module.exports.formatFromName = native.formatFromName
module.exports.detectFormat = native.detectFormat
// Email attachment payloads (#561): declarative, no ML guard needed.
module.exports.emailAttachments = native.emailAttachments
module.exports.emailAttachmentsFile = native.emailAttachmentsFile
module.exports.emailAttachmentsAsync = native.emailAttachmentsAsync
module.exports.emailAttachmentsFileAsync = native.emailAttachmentsFileAsync
