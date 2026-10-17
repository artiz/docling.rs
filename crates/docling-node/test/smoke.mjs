// Minimal smoke test exercising every part of the binding: sync + async
// convert, in-memory bytes, JSON output, the reusable class, streaming, and the
// format helpers. Run with `node test/smoke.mjs` (or `bun test/smoke.mjs`).
//
// Exits non-zero on the first failed assertion, so it doubles as a CI check.

import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import {
  checkDependencies,
  chunk,
  chunkAsync,
  chunkDocument,
  chunkDocumentAsync,
  chunkFileAsync,
  convert,
  convertArchive,
  convertArchiveFile,
  convertArchiveFileAsync,
  convertAsync,
  convertFile,
  convertFileAsync,
  DocumentConverter,
  emailAttachments,
  emailAttachmentsFile,
  emailAttachmentsFileAsync,
  formatFromName,
  detectFormat,
  Pipeline,
  registerBackend,
  registeredBackends,
  unregisterBackend,
  streamChunks,
  streamDocumentChunks,
  streamFileMarkdown,
  supportedFormats,
} from '../index.js'

let passed = 0
const failures = []
// Record and continue instead of rethrowing: a rethrow rejected main(), so the
// first failing check hid every check below it — most of this file, given one
// long-standing failure partway down. The checks are independent, so running
// them all and reporting the full list is strictly more useful.
const check = (name, fn) => {
  return Promise.resolve()
    .then(fn)
    .then(() => {
      passed++
      console.log(`  ok  ${name}`)
    })
    .catch((err) => {
      failures.push(name)
      console.error(`fail  ${name}\n      ${err.message}`)
      process.exitCode = 1
    })
}

const MD = '# Title\n\nHello **world**.\n\n- one\n- two\n'

// A small mixed archive (#557): two documents, an unsupported file, a broken
// DOCX — written once with Python's zipfile (docs/a.md, b.csv, tool.exe,
// broken.docx) and committed.
const BUNDLE = new URL('./fixtures/bundle.zip', import.meta.url).pathname

async function main() {
  await check('convertArchiveFile converts each entry on its own (#557)', () => {
    const items = convertArchiveFile(BUNDLE)
    assert.deepEqual(
      items.map((i) => [i.path, i.outcome]),
      [
        ['docs/a.md', 'converted'],
        ['b.csv', 'converted'],
        ['tool.exe', 'skipped'],
        ['broken.docx', 'failed'],
      ],
    )
    assert.ok(items[0].result.content.includes('alpha text'))
    assert.equal(items[0].result.format, 'md')
    assert.equal(items[2].error, 'unsupported file type')
    assert.ok(items[3].error.includes('docx'))
    // napi leaves an absent `Option` field undefined.
    assert.equal(items[3].result ?? null, null)
  })

  await check('convertArchive takes bytes, async variants and the class agree', async () => {
    const data = (await import('node:fs')).readFileSync(BUNDLE)
    const sync = convertArchive({ name: 'bundle.zip', data }, { to: 'json' })
    const viaFile = await convertArchiveFileAsync(BUNDLE, { to: 'json' })
    const viaClass = await new DocumentConverter().convertArchiveAsync({ name: 'bundle.zip', data }, { to: 'json' })
    for (const items of [sync, viaFile, viaClass]) {
      assert.equal(items.length, 4)
      assert.equal(items.filter((i) => i.outcome === 'converted').length, 2)
      assert.ok(JSON.parse(items[0].result.content).texts.length > 0)
    }
    assert.throws(() => convertArchive({ name: 'x.zip', data: Buffer.from('plain') }))
    // An archive is not a document: the plain converters still refuse it.
    assert.throws(() => convertFile(BUNDLE))
  })

  // Pluggable backends (#680): a JavaScript function claims inputs by
  // extension, media type or content, ahead of the built-in formats.
  const rtf2 = (data) => {
    const lines = data.toString('utf8').split('\n')
    if (lines[0] !== '{\\rtf2}') throw new Error('not rtf2')
    return lines.slice(1).filter(Boolean).map((l) => l.replace(/^\\h1 /, '# ').replace(/^\\p /, '')).join('\n\n')
  }
  const RTF2 = Buffer.from('{\\rtf2}\n\\h1 Notes\n\\p Plain text, in a format of its own.\n')
  const fsp = await import('node:fs')
  const os = await import('node:os')
  const { join: pjoin } = await import('node:path')
  const scratch = fsp.mkdtempSync(pjoin(os.tmpdir(), 'docling-node-plugins-'))

  await check('registerBackend adds a format: by name, media type, content (#680)', async () => {
    // Unregistered, content detection takes it for RTF.
    const before = convert({ name: 'notes.rtf2', data: RTF2 }).content
    assert.ok(!before.startsWith('# Notes'), before)
    const name = registerBackend({
      extensions: ['.RTF2'],
      mimeTypes: ['application/x-rtf2'],
      sniff: (data) => data.subarray(0, 7).toString() === '{\\rtf2}',
      convert: rtf2,
    })
    try {
      assert.equal(name, 'rtf2')
      assert.deepEqual(registeredBackends(), [
        { name: 'rtf2', extensions: ['rtf2'], mimeTypes: ['application/x-rtf2'], sniff: true, priority: 0 },
      ])
      const byName = convert({ name: 'notes.rtf2', data: RTF2 })
      assert.equal(byName.content, '# Notes\n\nPlain text, in a format of its own.\n')
      assert.equal(byName.format, 'plugin')
      assert.equal(byName.inputName, 'notes')
      assert.equal(byName.status, 'success')
      // Content alone, the media type, a file, async, JSON output, the class.
      assert.equal(convert({ name: 'upload', data: RTF2 }).content, byName.content)
      assert.equal(convert({ name: 'upload', data: RTF2, mimeType: 'application/x-rtf2; v=1' }).content, byName.content)
      const file = pjoin(scratch, 'notes.rtf2')
      fsp.writeFileSync(file, RTF2)
      assert.equal(convertFile(file).content, byName.content)
      assert.equal((await convertFileAsync(file)).content, byName.content)
      assert.equal((await convertAsync({ name: 'notes.rtf2', data: RTF2 })).format, 'plugin')
      const json = JSON.parse(convert({ name: 'notes.rtf2', data: RTF2 }, { to: 'json' }).content)
      assert.equal(json.texts[0].label, 'title')
      assert.equal(new DocumentConverter().convertFile(file).content, byName.content)
      const chunks = chunk({ name: 'notes.rtf2', data: RTF2 })
      assert.ok(chunks.some((c) => c.text.includes('Plain text')))
      let streamed = ''
      for await (const piece of streamFileMarkdown(file)) streamed += piece
      assert.equal(streamed, byName.content)
      // Other formats are untouched.
      assert.equal(convert({ name: 'a.md', data: Buffer.from(MD) }).format, 'md')
    } finally {
      assert.ok(unregisterBackend('rtf2'))
    }
    assert.deepEqual(registeredBackends(), [])
    assert.equal(convertFile(pjoin(scratch, 'notes.rtf2')).content, before)
  })

  await check('a backend takes over a built-in format, by priority, and may return a document (#680)', async () => {
    const converter = new DocumentConverter()
    converter.registerBackend({ name: 'low', extensions: ['csv'], convert: () => 'low', priority: -1 })
    converter.registerBackend({ name: 'first', extensions: ['csv'], convert: () => 'first' })
    converter.registerBackend({ name: 'second', extensions: ['csv'], convert: () => 'second' })
    converter.registerBackend({
      name: 'doc',
      extensions: ['csv'],
      priority: 5,
      convert: (data, name) =>
        JSON.parse(convert({ name: 'x.md', data: Buffer.from(`# From ${name}\n`) }, { to: 'json' }).content),
    })
    assert.deepEqual(
      converter.registeredBackends().map((b) => b.name),
      ['doc', 'first', 'second', 'low'],
    )
    const out = converter.convert({ name: 'table.csv', data: Buffer.from('a,b\n1,2\n') })
    assert.equal(out.content, '# From table.csv\n')
    assert.equal(out.format, 'csv')
    // Registered on the instance only: the module functions read CSV as CSV.
    const plain = convert({ name: 'table.csv', data: Buffer.from('a,b\n1,2\n') }).content
    assert.ok(plain.startsWith('|') && !plain.includes('From'), plain)
    assert.deepEqual(registeredBackends(), [])
  })

  await check('backend errors and bad registrations are reported (#680)', async () => {
    const converter = new DocumentConverter()
    converter.registerBackend({ extensions: ['bad'], convert: () => { throw new Error('nope') } })
    converter.registerBackend({ extensions: ['num'], convert: () => 42 })
    converter.registerBackend({ extensions: ['later'], convert: async () => '# Later\n' })
    assert.throws(() => converter.convert({ name: 'x.bad', data: Buffer.from('x') }), /backend `bad`: nope/)
    assert.throws(() => converter.convert({ name: 'x.num', data: Buffer.from('x') }), /returned number/)
    assert.throws(() => converter.convert({ name: 'x.later', data: Buffer.from('x') }), /is async/)
    assert.equal((await converter.convertAsync({ name: 'x.later', data: Buffer.from('x') })).content, '# Later\n')
    await assert.rejects(converter.convertAsync({ name: 'x.bad', data: Buffer.from('x') }), /backend `bad`: nope/)
    assert.throws(() => registerBackend({ extensions: ['x'] }), /convert\(data, name\)/)
    assert.throws(() => registerBackend({ convert: () => '' }), /claims nothing/)
    assert.throws(() => registerBackend({ extensions: 'x', convert: () => '' }), /list of strings/)
    assert.deepEqual(registeredBackends(), [])
  })

  await check('supportedFormats lists md and pdf', () => {
    const formats = supportedFormats()
    assert.ok(formats.includes('md'))
    assert.ok(formats.includes('pdf'))
  })

  await check('detectFormat reads the bytes', () => {
    assert.equal(detectFormat(Buffer.from('%PDF-1.7\n')), 'pdf')
    assert.equal(detectFormat(Buffer.from('WEBVTT\n\n00:00.000 --> 00:01.000\nhi')), 'vtt')
    assert.equal(detectFormat(Buffer.from('just words')), null)
  })

  await check('formatFromName detects extensions', () => {
    assert.equal(formatFromName('report.pdf'), 'pdf')
    assert.equal(formatFromName('page.html'), 'html')
    assert.equal(formatFromName('mystery.zzz'), null)
  })

  await check('emailAttachments lists an .eml attachment with its payload', () => {
    const eml = new URL('../../../tests/data/email/sources/eml_with_attachment.eml', import.meta.url)
      .pathname
    const atts = emailAttachmentsFile(eml)
    assert.equal(atts.length, 1)
    const [att] = atts
    assert.equal(att.name, 'test.txt')
    assert.equal(att.contentType, 'text/plain')
    assert.equal(att.format, 'md')
    assert.equal(att.inline, false)
    // Optional fields are absent (undefined) when unset, like every other
    // napi object.
    assert.equal(att.skipped, undefined)
    assert.ok(Buffer.isBuffer(att.data))
    assert.equal(att.size, att.data.length)
    assert.match(att.data.toString(), /^This is a test attachment file\./)
    // In-memory bytes give the same listing, and the payload converts.
    const fromBytes = emailAttachments({ name: 'm.eml', data: readFileSync(eml) })
    assert.deepEqual(fromBytes, atts)
    // `format` carries what the extension cannot (#564).
    const res = convert({ name: att.name, data: att.data, format: att.format })
    assert.match(res.content, /This is a test attachment file/)
    // A limit skips an attachment and drops its payload.
    const limited = emailAttachmentsFile(eml, { maxEntrySize: 10 })
    assert.equal(limited[0].skipped, 'larger than the per-entry size limit')
    assert.equal(limited[0].data, undefined)
  })

  await check('emailAttachments: payload bytes as sent, inline images only, unique names (#564)', async () => {
    const b64 = (b) => Buffer.from(b).toString('base64')
    const latin1 = Buffer.from('caf\xe9', 'latin1')
    const msg = [
      'From: a@x.com', 'Subject: S', 'MIME-Version: 1.0',
      'Content-Type: multipart/mixed; boundary="bb"', '', '--bb',
      'Content-Type: text/plain', '', 'Body.',
      '--bb', 'Content-Type: text/plain; charset=iso-8859-1',
      'Content-Disposition: attachment; filename="note.txt"',
      'Content-Transfer-Encoding: base64', '', b64(latin1),
      '--bb', 'Content-Type: application/pdf',
      'Content-Disposition: inline; filename="report.pdf"',
      'Content-Transfer-Encoding: base64', '', b64('%PDF-1.4 one'),
      '--bb', 'Content-Type: application/pdf',
      'Content-Disposition: attachment; filename="report.pdf"',
      'Content-Transfer-Encoding: base64', '', b64('%PDF-1.4 two'),
      '--bb--', '',
    ].join('\r\n')
    const atts = emailAttachments({ name: 'm.eml', data: Buffer.from(msg) })
    // The async form lists the same, off the event loop.
    const eml = new URL('../../../tests/data/email/sources/eml_with_attachment.eml', import.meta.url)
      .pathname
    assert.deepEqual(await emailAttachmentsFileAsync(eml), emailAttachmentsFile(eml))
    assert.deepEqual(
      atts.map((a) => [a.name, a.inline]),
      [['note.txt', false], ['report.pdf', false], ['report-2.pdf', false]],
    )
    // Latin-1 stays Latin-1: no UTF-8 re-encoding of a text attachment.
    assert.deepEqual([...atts[0].data], [...latin1])
    assert.equal(atts[2].data.toString(), '%PDF-1.4 two')
  })

  await check('convert (bytes) → Markdown round-trips', () => {
    const res = convert({ name: 'doc', data: Buffer.from(MD), format: 'md' })
    assert.equal(res.status, 'success')
    assert.deepEqual(res.errors, [])
    assert.equal(res.format, 'md')
    assert.match(res.content, /# Title/)
    assert.match(res.content, /Hello/)
  })

  await check('convert (bytes) → JSON is docling-core wire format', () => {
    const res = convert({ name: 'doc', data: Buffer.from(MD), format: 'md' }, { to: 'json' })
    const doc = JSON.parse(res.content)
    assert.equal(doc.schema_name, 'DoclingDocument')
    assert.ok(Array.isArray(doc.texts))
  })

  await check('convert (bytes) → Pandoc AST for pandoc -f json', () => {
    const res = convert({ name: 'doc', data: Buffer.from(MD), format: 'md' }, { to: 'pandoc' })
    const ast = JSON.parse(res.content)
    assert.deepEqual(ast['pandoc-api-version'], [1, 23, 1, 1])
    assert.equal(ast.blocks[0].t, 'Header')
  })

  await check('format inferred from name when omitted', () => {
    const res = convert({ name: 'notes.md', data: Buffer.from(MD) })
    assert.equal(res.format, 'md')
  })

  await check('DocumentConverter class is reusable', () => {
    const converter = new DocumentConverter({ strict: true })
    const a = converter.convert({ name: 'a.md', data: Buffer.from('# A\n') })
    const b = converter.convert({ name: 'b.md', data: Buffer.from('# B\n') })
    assert.match(a.content, /# A/)
    assert.match(b.content, /# B/)
  })

  await check('allowedFormats rejects other formats', () => {
    const converter = new DocumentConverter({ allowedFormats: ['csv'] })
    assert.throws(() => converter.convert({ name: 'x.md', data: Buffer.from(MD) }))
  })

  await check('unknown format string is rejected', () => {
    assert.throws(() => convert({ name: 'x', data: Buffer.from(MD), format: 'nope' }))
  })

  // --- chunking --------------------------------------------------------------

  const CHUNK_MD = '# Guide\n\n## Setup\n\nInstall the tools.\n\n- clone\n- build\n\n## Usage\n\nRun it.\n'

  await check('chunk (hierarchical) carries heading paths and doc items', () => {
    const chunks = chunk({ name: 'guide.md', data: Buffer.from(CHUNK_MD) })
    assert.ok(chunks.length >= 3)
    const setup = chunks.find((c) => c.text.includes('Install'))
    assert.deepEqual(setup.headings, ['Guide', 'Setup'])
    assert.ok(setup.docItems.length >= 1)
    assert.match(setup.docItems[0], /^#\//)
    assert.equal(setup.contextualized, 'Guide\nSetup\nInstall the tools.')
    const list = chunks.find((c) => c.text.includes('clone'))
    assert.equal(list.text, '- clone\n- build')
  })

  await check('chunkAsync resolves off the event loop', async () => {
    const chunks = await chunkAsync({ name: 'guide.md', data: Buffer.from(CHUNK_MD) })
    assert.ok(chunks.length >= 3)
  })

  await check('chunkDocument chunks a converted JSON document', async () => {
    const res = convert({ name: 'guide.md', data: Buffer.from(CHUNK_MD) }, { to: 'json' })
    const sync = chunkDocument(res.content)
    const async_ = await chunkDocumentAsync(res.content)
    assert.deepEqual(async_, sync)
    assert.ok(sync.some((c) => c.text.includes('Install')))
  })

  await check('streamChunks yields the same chunks as chunk, one at a time', async () => {
    const buffered = chunk({ name: 'guide.md', data: Buffer.from(CHUNK_MD) })
    const streamed = []
    for await (const c of streamChunks({ name: 'guide.md', data: Buffer.from(CHUNK_MD) })) {
      streamed.push(c)
    }
    assert.deepEqual(streamed, buffered)
  })

  await check('streamChunks supports early break', async () => {
    let first = null
    for await (const c of streamChunks({ name: 'guide.md', data: Buffer.from(CHUNK_MD) })) {
      first = c
      break // abandoning the generator cancels the background chunking
    }
    assert.ok(first && typeof first.text === 'string')
  })

  await check('streamDocumentChunks streams a converted JSON document', async () => {
    const res = convert({ name: 'guide.md', data: Buffer.from(CHUNK_MD) }, { to: 'json' })
    const streamed = []
    for await (const c of streamDocumentChunks(res.content)) streamed.push(c)
    assert.deepEqual(streamed, chunkDocument(res.content))
  })

  await check('streamChunks surfaces conversion errors', async () => {
    await assert.rejects(async () => {
      for await (const _ of streamChunks({ name: 'x', data: Buffer.from(MD), format: 'nope' })) {
        void _
      }
    })
  })

  await check('hybrid without any tokenizer errors with the download hint', () => {
    // No explicit path and no models/chunk/tokenizer.json in this test cwd.
    assert.throws(
      () => chunk({ name: 'g.md', data: Buffer.from(CHUNK_MD) }, { chunker: 'hybrid' }),
      /download_dependencies|tokenizer/,
    )
  })

  await check('unknown chunker name is rejected', () => {
    assert.throws(
      () => chunk({ name: 'g.md', data: Buffer.from(CHUNK_MD) }, { chunker: 'semantic' }),
      /unknown chunker/,
    )
  })

  // Hybrid end-to-end only when a tokenizer.json is available (repo checkout).
  const { existsSync } = await import('node:fs')
  const TOKENIZER = new URL('../../../tests/data/chunks/tokenizer.json', import.meta.url).pathname
  if (existsSync(TOKENIZER)) {
    await check('hybrid chunker splits against the token budget', async () => {
      const long = '# Doc\n\n' + Array.from({ length: 40 }, (_, i) => `Sentence number ${i} padding words here.`).join(' ') + '\n'
      const hier = chunk({ name: 'l.md', data: Buffer.from(long) })
      const hybrid = await chunkAsync(
        { name: 'l.md', data: Buffer.from(long) },
        { chunker: 'hybrid', tokenizer: TOKENIZER, maxTokens: 64 },
      )
      assert.ok(hybrid.length > hier.length, `expected split: hybrid ${hybrid.length} vs hierarchical ${hier.length}`)
      assert.deepEqual(hybrid[0].headings, ['Doc'])
    })
    await check('hybrid picks up .models/chunk/tokenizer.json by default', async () => {
      const { mkdirSync, copyFileSync, mkdtempSync: mktemp } = await import('node:fs')
      const { tmpdir: osTmp } = await import('node:os')
      const { join: joinPath } = await import('node:path')
      const home = mktemp(joinPath(osTmp(), 'fw-chunk-'))
      // `.models`, not `models`: deps.js's homeDir() only adopts the cwd as the
      // install home when it holds `.models/`. The plain
      // `models/` layout is the one internal to ~/.cache/docling.rs, reachable
      // through DOCLING_RS_HOME — never by chdir alone, which is what this
      // check does. (Long-standing failure; unrelated to the VLM work.)
      mkdirSync(joinPath(home, '.models', 'chunk'), { recursive: true })
      copyFileSync(TOKENIZER, joinPath(home, '.models', 'chunk', 'tokenizer.json'))
      const prevCwd = process.cwd()
      process.chdir(home) // deps.js resolves the install home from the cwd
      try {
        const chunks = chunk(
          { name: 'g.md', data: Buffer.from(CHUNK_MD) },
          { chunker: 'hybrid', maxTokens: 64 },
        )
        // Undersized same-heading peers merge: Setup's paragraph + list
        // become one chunk, so hybrid yields fewer chunks than hierarchical.
        assert.ok(chunks.length >= 2)
        assert.ok(chunks.some((c) => c.text.includes('Install') && c.text.includes('clone')))
      } finally {
        process.chdir(prevCwd)
      }
    })
  } else {
    console.log('  --  tokenizer.json not found; skipping hybrid end-to-end check')
  }

  // --- ML dependency guards (models not installed in this test env) ---------

  await check('checkDependencies reports status without downloading', () => {
    const status = checkDependencies()
    assert.equal(typeof status.ready, 'boolean')
    assert.ok(Array.isArray(status.missing))
  })

  // These assume the ML models are NOT installed (true on a fresh CI checkout).
  const depsInstalled = checkDependencies().ready
  if (!depsInstalled) {
    await check('convert PDF (sync) throws pointing at download_dependencies.sh', () => {
      assert.throws(
        () => convert({ name: 'doc.pdf', data: Buffer.from('%PDF-1.4') }),
        /download_dependencies\.sh/,
      )
    })

    await check('convertFileAsync PDF rejects (not a sync throw)', async () => {
      await assert.rejects(convertFileAsync('missing.pdf'), /download_dependencies\.sh/)
    })

    await check('image bytes are guarded too', () => {
      assert.throws(() => convert({ name: 'scan.png', data: Buffer.from([0]) }), /download_dependencies\.sh/)
    })

    await check('Pipeline convertFile is guarded', () => {
      const pipe = new Pipeline()
      assert.throws(() => pipe.convertFile('x.pdf'), /download_dependencies\.sh/)
    })

    await check('Pipeline convertFileAsync rejects (not a sync throw)', async () => {
      const pipe = new Pipeline()
      await assert.rejects(pipe.convertFileAsync('x.pdf'), /download_dependencies\.sh/)
    })

    await check('Pipeline streamFileMarkdown rejects on iteration', async () => {
      const pipe = new Pipeline()
      await assert.rejects(pipe.streamFileMarkdown('x.pdf').next(), /download_dependencies\.sh/)
    })
  } else {
    console.log('  --  ML deps installed; skipping guard checks')
  }

  // --- VLM pipeline (#77) ----------------------------------------------------
  //
  // `pipeline: 'vlm'` replaces the ONNX stack with a remote OpenAI-compatible
  // endpoint, so every check here runs with no models on disk. They all use
  // image input on purpose: an image is already its own page, so this leg needs
  // no layout model and runs in any environment — the same reasoning as
  // `vlm_converts_an_image_without_a_pdf_stack` in crates/docling/tests/vlm.rs.

  // A 1x1 PNG. `convert_vlm` forwards image bytes untouched, but a real file
  // keeps the fixture honest if that ever changes.
  const PNG = Buffer.from(
    'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==',
    'base64',
  )

  // A mock of an OpenAI-compatible /chat/completions endpoint, the JS
  // counterpart of crates/docling/tests/vlm.rs's `mock_openai`. Node's own
  // http module — no dependency. `onRequest` sees each parsed request body; a
  // throw from it answers 400, which `convert_vlm` treats as fatal (only
  // transport errors, 408/429 and 5xx retry), so an assertion failure surfaces
  // immediately instead of after the retry backoff.
  const mockVlm = async (content, onRequest) => {
    const { createServer } = await import('node:http')
    const state = { served: 0 }
    const server = createServer((req, res) => {
      const body = []
      req.on('data', (d) => body.push(d))
      req.on('end', () => {
        state.served++
        try {
          if (onRequest) onRequest(JSON.parse(Buffer.concat(body).toString('utf8')), req)
        } catch (err) {
          res.writeHead(400).end(err.message)
          return
        }
        res.writeHead(200, { 'content-type': 'application/json' })
        res.end(JSON.stringify({ choices: [{ message: { role: 'assistant', content } }] }))
      })
    })
    await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
    return {
      url: `http://127.0.0.1:${server.address().port}/v1`,
      // How many requests actually reached the endpoint — the only way to tell
      // "rejected before converting" from "converted anyway".
      get served() {
        return state.served
      },
      // close() in a finally: a live listener keeps the process alive past
      // main(), and closed keep-alive sockets would too.
      close: () => {
        server.closeAllConnections?.()
        server.close()
      },
    }
  }

  // Assertions on request defaults only hold when the ambient environment isn't
  // supplying them — `VlmOptions::resolve` seeds the prompt from
  // DOCLING_RS_VLM_PROMPT, and DOCLING_RS_VLM_EXTRA_BODY is merged over the
  // body. A VLM developer very plausibly has these exported.
  const vlmEnv = (name) => !!process.env[`DOCLING_RS_VLM_${name}`]

  await check('unknown pipeline name is rejected', () => {
    assert.throws(
      () => convert({ name: 'x.md', data: Buffer.from(MD) }, { pipeline: 'magic' }),
      /unknown pipeline/,
    )
  })

  await check("Pipeline (the warm-ONNX class) refuses pipeline: 'vlm'", () => {
    assert.throws(() => new Pipeline({ pipeline: 'vlm' }), /loads no models/)
  })

  // The ignore is deliberate, not an oversight: an option configures the VLM,
  // only `pipeline` selects it — the same rule as the CLI, which parses a
  // stray `--vlm-endpoint` without `--pipeline vlm` and drops it. Pinned from
  // the caller's side (the `resolve_vlm` unit test pins the resolution side):
  // with an endpoint that *would* answer, nothing reaches it. Markdown input
  // keeps the check environment-independent — the standard path converts it
  // with no models on disk.
  await check("vlm* options without pipeline: 'vlm' are ignored, not an error", async () => {
    const mock = await mockVlm('<text>never asked</text>')
    try {
      for (const pipeline of [undefined, 'standard']) {
        const res = await convertAsync(
          { name: 'notes.md', data: Buffer.from(MD) },
          {
            pipeline,
            vlmEndpoint: mock.url,
            vlmModel: 'granite-docling',
            vlmApiKey: 'sekret',
            vlmPrompt: 'Describe this page.',
            vlmMaxTokens: 512,
          },
        )
        assert.equal(res.status, 'success')
        assert.match(res.content, /# Title/)
        assert.equal(mock.served, 0, `pipeline ${pipeline} must not contact the VLM`)
      }
    } finally {
      mock.close()
    }
  })

  await check('Pipeline rejects an unknown pipeline name too', () => {
    assert.throws(() => new Pipeline({ pipeline: 'magic' }), /unknown pipeline/)
  })

  // #471: the warm class reads every PDF/image option the one-shot path does,
  // and validates it on `new` the way DocumentConverter does — before, only
  // `strict` and the enrichment switches were read, so a `por+eng` Tesseract
  // pipeline constructed fine and silently ran PP-OCR English. The builders
  // probe nothing (Tesseract is located on first use), so this needs no
  // models or binary on disk.
  await check('Pipeline validates its OCR options like DocumentConverter', () => {
    assert.throws(() => new Pipeline({ ocrEngine: 'tesseract', ocrLang: 'xx' }), /ocrLang/)
    assert.throws(() => new DocumentConverter({ ocrEngine: 'tesseract', ocrLang: 'xx' }), /ocrLang/)
    assert.throws(() => new Pipeline({ ocrEngine: 'bogus' }), /ocrEngine/)
    assert.throws(() => new Pipeline({ ocrMode: 'sideways' }), /ocrMode/)
    assert.throws(() => new Pipeline({ ocrScale: 0 }), /ocrScale/)
    assert.throws(() => new Pipeline({ pages: '3-1' }), /pages/)
    assert.throws(() => new Pipeline({ documentTimeout: 0 }), /documentTimeout/)
    assert.throws(() => new DocumentConverter({ documentTimeout: -5 }), /documentTimeout/)
    new DocumentConverter({ documentTimeout: 90 })
    new Pipeline({ ocrEngine: 'tesseract', ocrLang: 'por+eng', ocrMode: 'full_page', pages: '1-2' })
    new Pipeline({ ocrEngine: 'ppocr', ocrLang: 'zh-Hans', ocrScale: 3, headingHierarchy: true })
  })

  await check('a misspelled pipeline name reports the typo, not a missing model', () => {
    // The JS guard runs before native option parsing, so it has to recognize a
    // bad name itself — otherwise a capitalization slip falls through to the
    // strict ONNX requirement and reads as "download hundreds of MB of models".
    assert.throws(
      () =>
        convert(
          { name: 'doc.pdf', data: Buffer.from('%PDF-1.4') },
          { pipeline: 'VLM', vlmEndpoint: 'http://127.0.0.1:1/v1', vlmModel: 'm' },
        ),
      (e) => /unknown pipeline 'VLM'/.test(e.message) && !/download_dependencies/.test(e.message),
    )
  })

  await check('mets_gbs under vlm says what is wrong instead of asking for models', () => {
    // METS-GBS is an ML format with no VLM path; fetching models can't help.
    assert.throws(
      () =>
        convert(
          { name: 'scans.tar.gz', data: Buffer.from([0]) },
          { pipeline: 'vlm', vlmEndpoint: 'http://127.0.0.1:1/v1', vlmModel: 'm' },
        ),
      (e) =>
        /converts PDF and image inputs/.test(e.message) &&
        !/download_dependencies/.test(e.message),
    )
  })

  await check('vlmMaxTokens: 0 is rejected at option-parse time', () => {
    assert.throws(
      () =>
        convert(
          { name: 'scan.png', data: PNG },
          {
            pipeline: 'vlm',
            vlmEndpoint: 'http://127.0.0.1:1/v1',
            vlmModel: 'm',
            vlmMaxTokens: 0,
          },
        ),
      /vlmMaxTokens must be a positive integer/,
    )
  })

  if (!process.env.DOCLING_RS_VLM_ENDPOINT) {
    await check("pipeline: 'vlm' skips the ML guard and fails on the missing endpoint", () => {
      // Reaching the endpoint error at all is the assertion: under the standard
      // pipeline this same call demands layout_heron.onnx (see the guard checks
      // above), so a download_dependencies hint here would mean the VLM
      // exemption in deps.js never fired.
      assert.throws(
        () => convert({ name: 'scan.png', data: PNG }, { pipeline: 'vlm' }),
        /DOCLING_RS_VLM_ENDPOINT/,
      )
    })
  } else {
    console.log('  --  DOCLING_RS_VLM_ENDPOINT set; skipping the missing-endpoint check')
  }

  await check("pipeline: 'vlm' converts an image through the endpoint", async () => {
    let seen = null
    const mock = await mockVlm('<text>Hello from the VLM.</text>', (body, req) => {
      seen = { body, auth: req.headers.authorization }
    })
    try {
      // Async, not sync: a sync convert would block the event loop the mock
      // server answers on, and the request would only ever time out.
      const res = await convertAsync(
        { name: 'scan.png', data: PNG, format: 'image' },
        {
          pipeline: 'vlm',
          vlmEndpoint: mock.url,
          vlmModel: 'mock-docling',
          vlmMaxTokens: 512,
          vlmApiKey: 'sekret',
        },
      )
      assert.equal(res.status, 'success')
      assert.equal(res.format, 'image')
      assert.match(res.content, /Hello from the VLM\./)
    } finally {
      mock.close()
    }
    // The wire shape the CLI's --pipeline vlm sends too.
    assert.equal(seen.body.model, 'mock-docling')
    const image = seen.body.messages[0].content.find((c) => c.type === 'image_url')
    assert.ok(image.image_url.url.startsWith('data:image/png;base64,'))
    // vlmApiKey travels as a Bearer header, not in the body — the only place
    // that plumbing is observable.
    assert.equal(seen.auth, 'Bearer sekret')
    if (!vlmEnv('EXTRA_BODY')) assert.equal(seen.body.max_tokens, 512)
    if (!vlmEnv('PROMPT')) {
      assert.equal(seen.body.messages[0].content[0].text, 'Convert this page to docling.')
    }
  })

  await check("pipeline: 'vlm' honours to: 'json'", async () => {
    const mock = await mockVlm('<heading level="1">Doc</heading><text>Body.</text>')
    try {
      const res = await convertAsync(
        { name: 'scan.png', data: PNG, format: 'image' },
        { pipeline: 'vlm', vlmEndpoint: mock.url, vlmModel: 'm', to: 'json' },
      )
      const doc = JSON.parse(res.content)
      assert.equal(doc.schema_name, 'DoclingDocument')
      assert.ok(JSON.stringify(doc.texts).includes('Body.'))
    } finally {
      mock.close()
    }
  })

  await check('vlmPrompt overrides the default page instruction', async () => {
    let seen = null
    const mock = await mockVlm('<text>ok</text>', (body) => {
      seen = body
    })
    try {
      await convertAsync(
        { name: 'scan.png', data: PNG, format: 'image' },
        { pipeline: 'vlm', vlmEndpoint: mock.url, vlmModel: 'm', vlmPrompt: 'Describe this page.' },
      )
    } finally {
      mock.close()
    }
    assert.equal(seen.messages[0].content[0].text, 'Describe this page.')
  })

  await check('DocumentConverter carries the vlm pipeline across calls', async () => {
    const mock = await mockVlm('<text>Reused.</text>')
    try {
      const converter = new DocumentConverter({
        pipeline: 'vlm',
        vlmEndpoint: mock.url,
        vlmModel: 'm',
      })
      const a = await converter.convertAsync({ name: 'a.png', data: PNG, format: 'image' })
      const b = await converter.convertAsync({ name: 'b.png', data: PNG, format: 'image' })
      assert.match(a.content, /Reused\./)
      assert.equal(a.content, b.content)
    } finally {
      mock.close()
    }
  })

  await check('allowedFormats still applies under the vlm pipeline', async () => {
    const mock = await mockVlm('<text>never asked</text>')
    try {
      const converter = new DocumentConverter({
        allowedFormats: ['pdf'],
        pipeline: 'vlm',
        vlmEndpoint: mock.url,
        vlmModel: 'm',
      })
      // The VLM branch converts without going through the Rust
      // DocumentConverter, where this restriction normally lives — so it has to
      // re-check, or allowedFormats would lapse exactly under `pipeline: 'vlm'`.
      await assert.rejects(
        converter.convertAsync({ name: 'scan.png', data: PNG, format: 'image' }),
        /no backend implemented yet for format 'image'/,
      )
      // The message alone doesn't prove it: the standard path emits the same
      // text. What pins the VLM branch is that the endpoint was never called.
      assert.equal(mock.served, 0, 'the VLM must not be contacted for a disallowed format')
    } finally {
      mock.close()
    }
  })

  await check('vlm rejects a non-visual format instead of falling back', async () => {
    const mock = await mockVlm('<text>never asked</text>')
    try {
      await assert.rejects(
        convertAsync(
          { name: 'x.md', data: Buffer.from(MD) },
          { pipeline: 'vlm', vlmEndpoint: mock.url, vlmModel: 'm' },
        ),
        /vlm pipeline converts PDF and image/,
      )
    } finally {
      mock.close()
    }
  })

  await check('streamFileMarkdown under vlm reproduces the buffered Markdown', async () => {
    const { writeFileSync: wf, mkdtempSync: mt } = await import('node:fs')
    const { tmpdir: tmp } = await import('node:os')
    const { join: jn } = await import('node:path')
    const png = jn(mt(jn(tmp(), 'fw-vlm-')), 'scan.png')
    wf(png, PNG)
    const mock = await mockVlm('<heading level="1">Streamed</heading><text>Body.</text>')
    try {
      const opts = { pipeline: 'vlm', vlmEndpoint: mock.url, vlmModel: 'm' }
      let streamed = ''
      for await (const c of streamFileMarkdown(png, opts)) streamed += c
      // Nothing streams out of a VLM — the document arrives as one chunk — but
      // the generator's contract still holds: the concatenation is the buffered
      // output, byte for byte.
      assert.equal(streamed, (await convertFileAsync(png, opts)).content)
      assert.match(streamed, /# Streamed/)
    } finally {
      mock.close()
    }
  })

  // File-based sync + async + streaming, using a temp Markdown file.
  const { writeFileSync, mkdtempSync } = await import('node:fs')
  const { tmpdir } = await import('node:os')
  const { join } = await import('node:path')
  const dir = mkdtempSync(join(tmpdir(), 'fw-smoke-'))
  const file = join(dir, 'doc.md')
  writeFileSync(file, MD)

  await check('convertFile (sync) reads from disk', () => {
    const res = convertFile(file)
    assert.match(res.content, /# Title/)
    assert.equal(res.inputName, 'doc')
  })

  await check('convertFileAsync returns a Promise', async () => {
    const res = await convertFileAsync(file, { to: 'json' })
    assert.equal(JSON.parse(res.content).schema_name, 'DoclingDocument')
  })

  await check('streamFileMarkdown yields chunks equal to buffered output', async () => {
    let streamed = ''
    for await (const chunk of streamFileMarkdown(file)) {
      streamed += chunk
    }
    assert.equal(streamed, convertFile(file).content)
    assert.ok(streamed.length > 0)
  })

  // Warm-pipeline async + streaming, only when the ML deps are on disk (they
  // are in the repo dev environment; a fresh CI checkout skips these).
  if (depsInstalled) {
    const pdf = new URL('../../../tests/data/pdf/sources/code_and_formula.pdf', import.meta.url)
      .pathname
    const { existsSync } = await import('node:fs')
    if (existsSync(pdf)) {
      const pipe = new Pipeline()

      await check('Pipeline convertFileAsync resolves with the buffered output', async () => {
        const buffered = pipe.convertFile(pdf)
        const res = await pipe.convertFileAsync(pdf)
        assert.equal(res.status, 'success')
        assert.equal(res.content, buffered.content)
      })

      await check('Pipeline convertFileAsync to JSON', async () => {
        const res = await pipe.convertFileAsync(pdf, { to: 'json' })
        assert.equal(JSON.parse(res.content).schema_name, 'DoclingDocument')
      })

      await check('Pipeline convertAsync (bytes) matches convertFileAsync', async () => {
        const { readFileSync } = await import('node:fs')
        const res = await pipe.convertAsync({ name: 'doc.pdf', data: readFileSync(pdf) })
        assert.equal(res.content, (await pipe.convertFileAsync(pdf)).content)
      })

      await check('Pipeline streamFileMarkdown reproduces the buffered Markdown', async () => {
        let streamed = ''
        for await (const chunk of pipe.streamFileMarkdown(pdf)) {
          streamed += chunk
        }
        assert.equal(streamed, pipe.convertFile(pdf).content)
        assert.ok(streamed.length > 0)
      })

      await check('Pipeline streamFileMarkdown rejects referenced image mode', async () => {
        await assert.rejects(
          pipe.streamFileMarkdown(pdf, { imageMode: 'referenced' }).next(),
          /placeholder.*embedded|referenced/,
        )
      })

      await check('overlapping Pipeline async calls both resolve', async () => {
        const [a, b] = await Promise.all([pipe.convertFileAsync(pdf), pipe.convertFileAsync(pdf)])
        assert.equal(a.content, b.content)
      })
    } else {
      console.log('  --  PDF fixture not found; skipping warm-pipeline checks')
    }
  }

  console.log(`\n${passed} checks passed`)
  if (failures.length > 0) {
    console.error(`${failures.length} failed:\n  - ${failures.join('\n  - ')}`)
  }
}

main().catch((err) => {
  console.error(err)
  process.exitCode = 1
})
