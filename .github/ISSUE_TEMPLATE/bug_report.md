---
name: Bug report
about: Something converts wrong, errors out, or crashes
title: ''
labels: ''
assignees: ''
type: Bug

---

**What happened**
A clear description of the bug. If the output is wrong (vs Python docling or
vs expectations), a snippet of *got* vs *expected* is ideal.

**Command / code**

```bash
docling-rs --to md input.pdf
```

**Input document**
Attach the file if you can (drag & drop; zip it if GitHub rejects the
extension) — most conversion bugs are input-specific and reproduce instantly
with the file. If it's confidential, say so; the format + a rough description
(scanned/digital, language, produced by which app) still helps.

**Full error output**

```text
paste stderr here
```

**Environment**

- docling.rs version: `docling-rs --version` / crate / npm / PyPI version
- Installed via: `install.sh` | prebuilt binary | `cargo install` | npm | pip | Docker | source
- OS / arch: e.g. Ubuntu 24.04 x86_64, macOS 15 arm64, Windows 11
- For PDF/image issues: are the models + pdfium downloaded
  (`scripts/install/download_dependencies.sh`)? GPU (`DOCLING_RS_EP`) or CPU?
