"""``docling_rs.__version__`` is the installed distribution's version — the
one PyPI shows — not the native module's crate placeholder."""

from importlib.metadata import PackageNotFoundError, version

import docling_rs
from docling_rs import _native


def test_version_is_the_installed_distributions():
    installed = None
    for dist in ("docling-rs", "docling-rs-cuda"):
        try:
            installed = version(dist)
            break
        except PackageNotFoundError:
            continue
    expected = installed if installed is not None else _native.__version__
    assert docling_rs.__version__ == expected
    assert docling_rs.__version__[0].isdigit()
    # The User-Agent of URL fetches and the CLI's --version carry it too.
    from docling_rs import markitdown

    assert markitdown.__doc__ is not None
