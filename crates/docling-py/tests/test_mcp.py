"""The ``docling-rs-mcp`` launcher (#669): ``docling_rs.mcp`` runs the Rust
MCP server — the one ``docling-rs mcp`` runs — over stdio, and
``--download-models`` fetches what conversions need."""

import json
import subprocess
import sys
from pathlib import Path

import pytest

from docling_rs import _native

FIXTURES = Path(__file__).resolve().parents[3] / "tests" / "data"
needs_server = pytest.mark.skipif(
    not hasattr(_native, "mcp_main"), reason="built without the `mcp` feature"
)


def _record_fetches(monkeypatch, m):
    calls = []

    def fake(url, dest, optional, progress, force=False):
        calls.append((url, dest))
        return True

    monkeypatch.setattr(m, "_fetch", fake)
    return calls


def test_download_models_can_skip_the_enrichment_models(tmp_path, monkeypatch):
    """``enrichment=False`` leaves out CodeFormula (~740 MB, only
    ``do_code_enrichment`` / ``do_formula_enrichment`` use it); the default
    still fetches it."""
    from docling_rs import models as m

    calls = _record_fetches(monkeypatch, m)
    m.download_models(tmp_path, progress=False, enrichment=False)
    got = {str(dest.relative_to(tmp_path)) for _, dest in calls}
    assert "models/layout_heron.onnx" in got
    assert not any("code_formula" in p for p in got)
    calls = _record_fetches(monkeypatch, m)
    m.download_models(tmp_path, progress=False)
    assert any("code_formula" in str(dest) for _, dest in calls)


def test_download_models_flag(monkeypatch):
    """``--download-models`` fetches the conversion models and exits;
    ``=all`` adds the enrichment models."""
    from docling_rs import mcp, models

    seen = []
    monkeypatch.setattr(models, "download_models", lambda **kw: seen.append(kw) or Path("/c"))
    for argv, everything in ((["--download-models"], False), (["--download-models=all"], True)):
        with pytest.raises(SystemExit) as exit_:
            mcp.main(argv)
        assert exit_.value.code == 0
        assert seen.pop()["enrichment"] is everything


@needs_server
def test_launcher_serves_the_tools_over_stdio():
    p = subprocess.Popen(
        [sys.executable, "-m", "docling_rs.mcp", "--root", str(FIXTURES)],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )

    def call(id_, method, params):
        p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": id_, "method": method, "params": params}) + "\n")
        p.stdin.flush()
        while True:
            msg = json.loads(p.stdout.readline())
            if msg.get("id") == id_:
                return msg

    try:
        init = call(0, "initialize", {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "pytest", "version": "0"},
        })
        assert init["result"]["serverInfo"]["name"] == "docling-rs"
        p.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
        tools = call(1, "tools/list", {})["result"]["tools"]
        assert {t["name"] for t in tools} == {"convert", "convert_to_markdown", "list_input_formats"}
        res = call(2, "tools/call", {
            "name": "convert_to_markdown",
            "arguments": {"uri": "docx/sources/word_sample.docx"},
        })["result"]
        assert not res.get("isError")
        assert "Swimming in the lake" in res["content"][0]["text"]
        outside = call(3, "tools/call", {
            "name": "convert_to_markdown",
            "arguments": {"uri": "/etc/passwd"},
        })["result"]
        assert outside["isError"]
    finally:
        p.stdin.close()
        p.wait(timeout=60)
    # The launcher names itself on stderr, not the binary's `docling-rs mcp`.
    assert "docling-rs-mcp" in p.stderr.read()
