# YouTube fixtures (#681)

YouTube answers automated requests with HTTP 429 from the environment this
feature was built in, so these pages were not recorded from the site: they
were written to the shape a `https://www.youtube.com/watch?v=…` page has in
2026 — the `<title>`, the `og:` / `itemprop` / `name` metas, a
`var ytInitialPlayerResponse = {…};` script with `videoDetails` and
`captions.playerCaptionsTracklistRenderer` (`captionTracks` with `baseUrl`,
`languageCode`, `kind: "asr"`, `isTranslatable`; `translationLanguages`), a
`var ytInitialData = {…};` script with `attributedDescriptionBodyText` —
with invented identifiers and a fictional video. `tests/youtube.rs` serves
them from a local HTTP server, the caption `baseUrl`s rewritten to it.

| File | What |
|---|---|
| `watch.html` | three caption tracks: English (manual), English (auto-generated, `kind: asr`), German; translatable into French and Spanish |
| `watch_no_captions.html` | the same page shape with no `captions` object — the no-transcript error |
| `captions_en.vtt`, `captions_de.vtt` | what `baseUrl&fmt=vtt` answers for a manual track (`WEBVTT` + `Kind:` / `Language:` header lines) |
| `captions_en_asr.vtt` | the auto-generated track's form: word-level `<c>` timing tags and the rolling duplicate cues |
