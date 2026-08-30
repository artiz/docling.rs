"""Model/asset download for the docling.rs Python bindings.

Mirrors how Python docling manages its artifacts: models are fetched once
into a per-user cache directory (default ``~/.cache/docling.rs``, override
with ``$DOCLING_RS_CACHE_DIR``) and the pipeline is pointed at them via the
same ``DOCLING_*`` / ``PDFIUM_*`` environment variables the Rust CLI uses.
Assets come from this repo's GitHub model release
(https://github.com/docling-project/docling.rs/releases/tag/models-v1 — override the
base URL with ``$DOCLING_RS_MODELS_URL``).

Usage::

    import docling_rs
    docling_rs.download_models()          # once; idempotent, skips present files

``DocumentConverter`` calls :func:`ensure_env` automatically, so after the
one-time download no configuration is needed at all. Local assets outrank the
cache: when a matching ``.models/`` / ``.pdfium/`` asset exists in the
working directory (e.g. a repo checkout with its own exports), the env var is
left unset and the native pipeline resolves the local path itself, exactly
like the Rust CLI. Re-published release assets are picked up with
``download_models(force=True)`` — the cache has no version stamp. (The
cache's *internal* layout keeps the plain ``models/`` folder name — it lives
under the docling-owned cache root, so nothing collides.)
"""

from __future__ import annotations

import os
import sys
import urllib.request
from pathlib import Path

BASE_URL = os.environ.get(
    "DOCLING_RS_MODELS_URL",
    "https://github.com/docling-project/docling.rs/releases/download/models-v1",
)

# release asset name -> path under the cache dir (the CLI's layout).
_REQUIRED = {
    "layout_heron.onnx": "models/layout_heron.onnx",
    "ocr_rec.onnx": "models/ocr_rec.onnx",
    "ppocr_keys_v1.txt": "models/ppocr_keys_v1.txt",
    "encoder.onnx": "models/tableformer/encoder.onnx",
    "decoder.onnx": "models/tableformer/decoder.onnx",
    "bbox.onnx": "models/tableformer/bbox.onnx",
}
# Fetched when the release hosts them; a 404 is fine (older tag, optional
# sidecars, INT8 variants — the pipeline falls back to fp32 gracefully).
_OPTIONAL = {
    # The English PP-OCRv3 recognition pair — the engine's ocr_lang="en"
    # *default* (#285; the ch_ pair above stays the docling-conformance
    # model, selected with ocr_lang="ch"). The release may not host them;
    # the fallbacks below fetch straight from upstream, mirroring
    # scripts/install/download_dependencies.sh.
    "ocr_rec_en.onnx": "models/ocr_rec_en.onnx",
    "en_dict.txt": "models/en_dict.txt",
    "layout_heron_int8.onnx": "models/layout_heron_int8.onnx",
    "decoder_int8.onnx": "models/tableformer/decoder_int8.onnx",
    # The #97 hoisted-KV TableFormer decoder — byte-exact vs the legacy graph
    # and the fastest variant on every machine measured; ensure_env prefers it.
    "decoder_kv.onnx": "models/tableformer/decoder_kv.onnx",
    "decoder_kv.onnx.data": "models/tableformer/decoder_kv.onnx.data",
    "decoder_kv_int8.onnx": "models/tableformer/decoder_kv_int8.onnx",
    # DocumentFigureClassifier-v2.5 (~17 MB) for do_picture_classification;
    # missing file just skips the enrichment with a one-time warning.
    "picture_classifier.onnx": "models/picture_classifier.onnx",
    "encoder.onnx.data": "models/tableformer/encoder.onnx.data",
    "decoder.onnx.data": "models/tableformer/decoder.onnx.data",
    "bbox.onnx.data": "models/tableformer/bbox.onnx.data",
    # The hybrid chunker's default tokenizer (all-MiniLM-L6-v2's, ~0.5 MB);
    # falls back to Hugging Face below when the release doesn't host it.
    "chunk_tokenizer.json": "models/chunk/tokenizer.json",
}

# CodeFormula (do_code_enrichment / do_formula_enrichment) — the int8 decoder
# (~165 MB) makes the ~655 MB fp32 decoder unnecessary (same rule as
# download_dependencies.sh), so the fp32 graph is fetched only when the int8
# variant isn't hosted.
_ENRICH = {
    "cf_vision.onnx": "models/code_formula/vision.onnx",
    "cf_embed.onnx": "models/code_formula/embed.onnx",
    "cf_decoder_kv_int8.onnx": "models/code_formula/decoder_kv_int8.onnx",
    "cf_tokenizer.json": "models/code_formula/tokenizer.json",
}
_ENRICH_FP32_DECODER = ("cf_decoder_kv.onnx", "models/code_formula/decoder_kv.onnx")

# Straight-from-upstream fallback for assets older release tags don't host:
# cache path -> upstream URL.
_FALLBACK_URLS = {
    "models/ocr_rec_en.onnx": (
        "https://huggingface.co/SWHL/RapidOCR/resolve/main/PP-OCRv3/en_PP-OCRv3_rec_infer.onnx"
    ),
    "models/en_dict.txt": (
        "https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/main/ppocr/utils/en_dict.txt"
    ),
    "models/chunk/tokenizer.json": (
        "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/tokenizer.json"
    ),
    "models/picture_classifier.onnx": (
        "https://huggingface.co/docling-project/DocumentFigureClassifier-v2.5/resolve/main/model.onnx"
    ),
}
# pdfium rasterizer, selected by host platform (#299 — the mirror of the
# shell installers' #298 fix): Linux x64 takes the release's pinned
# conformance build; Linux arm64 and macOS arm64/x64 take the matching
# bblanchon prebuilt (the source the pinned build comes from), whose tarball
# ships `lib/libpdfium.so` / `lib/libpdfium.dylib`. Anything else skips with
# a note — a wrong-platform binary that "installs fine" and fails at dlopen
# time is exactly the failure mode this exists to avoid.
_PDFIUM_TGZ_BASE = "https://github.com/bblanchon/pdfium-binaries/releases/latest/download"


def _pdfium_plan(system: "str | None" = None, machine: "str | None" = None):
    """The pdfium install plan for a host: ``("release", lib)`` (pinned asset
    from the models release), ``("tarball", url, lib)`` (bblanchon prebuilt),
    or ``None`` (unsupported — skip). Parameters exist for tests; they default
    to the running host."""
    import platform as _platform

    system = system if system is not None else _platform.system()
    machine = (machine if machine is not None else _platform.machine()).lower()
    arch = {"x86_64": "x64", "amd64": "x64", "aarch64": "arm64", "arm64": "arm64"}.get(machine)
    if arch is None:
        return None
    if system == "Darwin":
        return ("tarball", f"{_PDFIUM_TGZ_BASE}/pdfium-mac-{arch}.tgz", "libpdfium.dylib")
    if system == "Linux" and arch == "x64":
        return ("release", "libpdfium.so")
    if system == "Linux":
        return ("tarball", f"{_PDFIUM_TGZ_BASE}/pdfium-linux-{arch}.tgz", "libpdfium.so")
    return None


def _pdfium_lib_name() -> str:
    """The platform pdfium library filename (what pdfium-render dlopens);
    ``libpdfium.so`` for unsupported hosts so path displays stay sensible."""
    plan = _pdfium_plan()
    return plan[-1] if plan else "libpdfium.so"


def cache_dir() -> Path:
    """The asset cache root (``$DOCLING_RS_CACHE_DIR`` or ``~/.cache/docling.rs``)."""
    if env := os.environ.get("DOCLING_RS_CACHE_DIR"):
        return Path(env)
    return Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache")) / "docling.rs"


def _fetch(url: str, dest: Path, optional: bool, progress: bool, force: bool = False) -> bool:
    if dest.exists() and not force:
        return True
    dest.parent.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_suffix(dest.suffix + ".download")
    try:
        if progress:
            print(f"  > {dest}", file=sys.stderr, flush=True)
        with urllib.request.urlopen(url) as r, open(tmp, "wb") as f:
            while chunk := r.read(1 << 20):
                f.write(chunk)
        tmp.rename(dest)
        return True
    except Exception:
        tmp.unlink(missing_ok=True)
        if optional:
            return False
        raise


def download_models(
    dest: "str | Path | None" = None, progress: bool = True, force: bool = False
) -> Path:
    """Fetch the PDF/image pipeline's models + pdfium into the cache (idempotent).

    Returns the cache root. Pass ``dest`` to use a custom directory (also set
    it as ``$DOCLING_RS_CACHE_DIR`` at runtime, or pass the same value as
    ``DocumentConverter(artifacts_path=...)``). Pass ``force=True`` to
    re-download files that are already cached — the cache has no version
    stamp, so this is how a stale cache picks up re-published model assets
    (e.g. the dynamic-batch layout graph or the hoisted-KV TableFormer
    decoder).
    """
    root = Path(dest) if dest else cache_dir()
    if progress:
        print(f"docling.rs: fetching models to {root}", file=sys.stderr, flush=True)
    for name, rel in _REQUIRED.items():
        _fetch(f"{BASE_URL}/{name}", root / rel, optional=False, progress=progress, force=force)
    _fetch_pdfium(root, progress=progress, force=force)
    for name, rel in {**_OPTIONAL, **_ENRICH}.items():
        if not _fetch(
            f"{BASE_URL}/{name}", root / rel, optional=True, progress=progress, force=force
        ):
            if fallback := _FALLBACK_URLS.get(rel):
                _fetch(fallback, root / rel, optional=True, progress=progress, force=force)
    # The huge fp32 CodeFormula decoder only matters when its int8 variant
    # isn't hosted (or DOCLING_RS_FP32 users fetch it here as the fallback).
    name, rel = _ENRICH_FP32_DECODER
    if not (root / _ENRICH["cf_decoder_kv_int8.onnx"]).exists():
        _fetch(f"{BASE_URL}/{name}", root / rel, optional=True, progress=progress, force=force)
    return root


def _fetch_pdfium(root: Path, progress: bool, force: bool) -> None:
    """Install the *platform's* pdfium into ``<root>/.pdfium/lib`` (#299).

    Linux x64 downloads the pinned release ``libpdfium.so`` like any other
    required asset; the tarball platforms extract just their ``lib/<name>``
    member (stdlib ``tarfile``). Idempotent like ``_fetch``; an unsupported
    host prints a note and skips instead of installing a binary that could
    never load."""
    plan = _pdfium_plan()
    if plan is None:
        import platform as _platform

        print(
            f"docling.rs: skipping pdfium ({_platform.system()}/{_platform.machine()} has no "
            "prebuilt); PDF/image rasterization needs PDFIUM_DYNAMIC_LIB_PATH",
            file=sys.stderr,
            flush=True,
        )
        return
    if plan[0] == "release":
        _fetch(
            f"{BASE_URL}/{plan[1]}",
            root / ".pdfium/lib" / plan[1],
            optional=False,
            progress=progress,
            force=force,
        )
        return
    _, url, lib = plan
    dest = root / ".pdfium/lib" / lib
    if dest.exists() and not force:
        return
    import io
    import tarfile

    dest.parent.mkdir(parents=True, exist_ok=True)
    if progress:
        print(f"  > {dest}", file=sys.stderr, flush=True)
    with urllib.request.urlopen(url) as r:
        data = r.read()
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as tar:
        member = tar.extractfile(f"lib/{lib}")
        if member is None:
            raise RuntimeError(f"{url}: tarball has no lib/{lib}")
        tmp = dest.with_suffix(dest.suffix + ".download")
        with open(tmp, "wb") as f:
            f.write(member.read())
        tmp.rename(dest)


def _point_at(var: str, local: "list[str]", cached: Path) -> None:
    """Set ``var`` to ``cached`` unless configuration already exists.

    Two things outrank the cache: an env var the caller already set, and a
    matching asset in the working directory (any of the ``local`` relative
    paths) — the native pipeline resolves those CWD paths itself when the env
    var stays unset, exactly like the Rust CLI run from a checkout. The env
    is also left untouched when ``cached`` doesn't exist."""
    if var in os.environ:
        return
    if any(Path(rel).exists() for rel in local):
        return
    if cached.exists():
        os.environ[var] = str(cached)


def _local(rels: "list[str]") -> "list[str]":
    """Working-directory candidates for cache-relative ``models/…`` paths —
    the checkout keeps them under ``.models/``."""
    for rel in rels:
        assert rel.startswith("models/"), rel
    return [f".{rel}" for rel in rels]


def ensure_env(dest: "str | Path | None" = None) -> Path:
    """Point the native pipeline at the cached assets via the ``DOCLING_*`` /
    ``PDFIUM_*`` env vars. Local assets win: a variable is only filled when it
    is not already set AND no matching ``.models/`` / ``.pdfium/`` asset
    exists in the working directory (the native code resolves those itself, so a repo
    checkout keeps using its own exports). Prefers the INT8 models when
    present, matching the Rust pipeline's default; ``DOCLING_RS_FP32=1`` opts
    out. OCR is handed over as ``DOCLING_RS_MODELS_DIR`` (the cache's models
    directory) rather than per-file pins, so the ``ocr_lang`` kwarg keeps
    selecting the en/ch recognition pair (#285). Safe to call when nothing is
    downloaded yet — missing files simply leave the env untouched (and the
    converter will fail with its usual clear "model not found" message)."""
    # Absolute paths in the env: a later os.chdir() must not orphan them.
    root = (Path(dest) if dest else cache_dir()).expanduser().resolve()
    # Same truthiness vocabulary as Rust's docling_core::env::flag.
    fp32 = os.environ.get("DOCLING_RS_FP32", "").strip().lower() not in ("", "0", "false", "no", "off")
    m = root / "models"

    layout_chain = ["models/layout_heron.onnx"]
    if not fp32:
        layout_chain.insert(0, "models/layout_heron_int8.onnx")
    layout = m / "layout_heron_int8.onnx"
    if fp32 or not layout.exists():
        layout = m / "layout_heron.onnx"
    _point_at("DOCLING_LAYOUT_ONNX", _local(layout_chain), layout)

    # TableFormer decoder preference, mirroring the Rust pipeline's default
    # chain (tableformer.rs): the #97 hoisted-KV graph ranks ahead of the
    # legacy layer-output-cache graph within each precision, and decoder_kv
    # (fp32) ranks above the quantized *legacy* decoder — it is faster on
    # every machine measured and byte-exact.
    if fp32:
        chain = ["tableformer/decoder_kv.onnx", "tableformer/decoder.onnx"]
    else:
        chain = [
            "tableformer/decoder_kv_int8.onnx",
            "tableformer/decoder_kv.onnx",
            "tableformer/decoder_int8.onnx",
            "tableformer/decoder.onnx",
        ]
    decoder = next((p for rel in chain if (p := m / rel).exists()), m / "tableformer/decoder.onnx")
    _point_at("DOCLING_TABLEFORMER_DECODER", _local([f"models/{rel}" for rel in chain]), decoder)

    classifier_chain = ["models/picture_classifier.onnx"]
    if not fp32:
        classifier_chain.insert(0, "models/picture_classifier_int8.onnx")
    classifier = m / "picture_classifier_int8.onnx"
    if fp32 or not classifier.exists():
        classifier = m / "picture_classifier.onnx"
    _point_at("DOCLING_PICTURE_CLASSIFIER_ONNX", _local(classifier_chain), classifier)

    # OCR is deliberately NOT pinned per file (#285): DOCLING_OCR_REC_ONNX /
    # DOCLING_OCR_DICT override the engine's en/ch pair selection outright,
    # which made the ocr_lang kwarg silently inert. Handing over the models
    # *directory* instead lets the engine resolve the pair for the requested
    # language itself (missing English pair → its usual warn-and-fall-back
    # to ch_; re-run download_models() to fetch it). The per-file vars stay
    # honored when the caller sets them — that is the pin-any-model hatch.
    _point_at("DOCLING_RS_MODELS_DIR", [".models"], m)
    _point_at(
        "DOCLING_TABLEFORMER_ENCODER",
        _local(["models/tableformer/encoder.onnx"]),
        m / "tableformer/encoder.onnx",
    )
    _point_at(
        "DOCLING_TABLEFORMER_BBOX",
        _local(["models/tableformer/bbox.onnx"]),
        m / "tableformer/bbox.onnx",
    )
    _point_at(
        "DOCLING_CODE_FORMULA_DIR",
        _local(["models/code_formula"]),
        m / "code_formula",
    )
    # The env var names the *directory*, but what must exist in it is the
    # *platform's* library (#299 — `libpdfium.dylib` on macOS): checking the
    # bare directory kept pointing macOS at a cache holding only a stale
    # Linux `.so`, which pdfium-render then never found.
    if (
        "PDFIUM_DYNAMIC_LIB_PATH" not in os.environ
        and not Path(".pdfium/lib").exists()
        and (root / ".pdfium/lib" / _pdfium_lib_name()).exists()
    ):
        os.environ["PDFIUM_DYNAMIC_LIB_PATH"] = str(root / ".pdfium/lib")
    return root
