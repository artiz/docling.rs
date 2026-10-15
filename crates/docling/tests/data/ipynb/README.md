# Jupyter notebook fixtures (#673)

`test_notebook.ipynb` is MarkItDown's
(`packages/markitdown/tests/test_files/`, MIT License, © Microsoft
Corporation) — the notebook its `IpynbConverter` tests read: a metadata
title, two markdown cells, two code cells with stream outputs.

The rest are course notebooks contributed by the project's maintainer:

- `recommendations.ipynb` — a recommender-systems assignment (Russian
  markdown, `%pylab` magics, `execute_result` and `stream` outputs,
  trailing empty cells);
- `milk_quiz.ipynb` — a time-series quiz (a `text/html` + `text/plain`
  DataFrame result, `image/png` plot outputs, no markdown cells);
- `vgg16.ipynb` — an image-classification assignment (long markdown cells
  with links and a fenced tree listing, a `%%time` cell, a 200-line
  stream output).

`cross_entropy_notes.ipynb` is docling.rs's own: display formulas (one on
one line, one spanning three), inline math, dollar amounts that are not
math, a formula inside a link, and a picture linked by URL — the formula
and remote-image cases of the in-browser demo.
