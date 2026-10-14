#!/usr/bin/env python3
"""Check that the files in a dist/ directory fit under each PyPI project's
total-size limit before any of them is uploaded.

PyPI caps the sum of all files a project keeps (10 GiB unless raised on
request: https://docs.pypi.org/project-management/storage-limits). An upload
past the cap is refused file by file, after the earlier files of the same
upload went through — v2.22.0 landed with its aarch64 wheel only, every other
wheel, the sdist and docling-rs-cuda refused, and the docling-rs-mcp launcher
pinned to it never published. So pypi-publish.yml runs this first: it fails
before anything is uploaded when a project would overflow, and warns when a
project passes WARN_FRACTION of its limit, while there is still room to
delete old releases (`pypi-cleanup`, crates/docling-py/README.md).

    pypi_room.py DIST_DIR

Files already on PyPI (a re-run with skip-existing) count as present, not as
new. PYPI_SIZE_LIMIT_GIB overrides the limit per project after PyPI raises
it: `docling-rs=20,docling-rs-cuda=15` (unlisted projects keep 10).
"""

import json
import os
import sys
import urllib.error
import urllib.request

GIB = 1 << 30
DEFAULT_LIMIT_GIB = 10.0
WARN_FRACTION = 0.9


def project_of(filename):
    # Wheel and sdist names start with the normalized distribution name,
    # whose `-` became `_`: docling_rs_cuda-2.22.0-… → docling-rs-cuda.
    return filename.split("-", 1)[0].replace("_", "-").lower()


def limits():
    out = {}
    for item in os.environ.get("PYPI_SIZE_LIMIT_GIB", "").split(","):
        if "=" in item:
            name, gib = item.split("=", 1)
            out[name.strip().lower()] = float(gib)
    return out


def published(project):
    """(bytes the project keeps, file names it has) — (0, set()) when new."""
    url = f"https://pypi.org/pypi/{project}/json"
    try:
        with urllib.request.urlopen(url, timeout=60) as r:
            data = json.load(r)
    except urllib.error.HTTPError as e:
        if e.code == 404:
            return 0, set()
        raise
    files = [f for fs in data["releases"].values() for f in fs]
    return sum(f["size"] for f in files), {f["filename"] for f in files}


def main(dist):
    new = {}
    for name in sorted(os.listdir(dist)):
        if name.endswith((".whl", ".tar.gz", ".zip")):
            new.setdefault(project_of(name), []).append(name)
    caps = limits()
    full = []
    for project, names in sorted(new.items()):
        used, existing = published(project)
        adding = sum(os.path.getsize(os.path.join(dist, n)) for n in names if n not in existing)
        limit = caps.get(project, DEFAULT_LIMIT_GIB) * GIB
        after = used + adding
        line = (
            f"{project}: {used / GIB:.2f} GiB on PyPI + {adding / GIB:.2f} GiB new "
            f"= {after / GIB:.2f} of {limit / GIB:.0f} GiB"
        )
        print(line)
        if after > limit:
            full.append(project)
            print(
                f"::error title=PyPI size limit::{line}. Nothing was uploaded. Delete old "
                f"releases of {project} (pypi-cleanup, crates/docling-py/README.md) or have "
                "PyPI raise the limit (then set PYPI_SIZE_LIMIT_GIB), and re-run this job."
            )
        elif after > WARN_FRACTION * limit:
            room = (limit - after) / max(adding, 1)
            print(
                f"::warning title=PyPI size limit::{line} — room for about {room:.0f} more "
                f"releases this size. Delete old releases of {project} or request a raise."
            )
    return 1 if full else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1] if len(sys.argv) > 1 else "dist"))
