# Vendored CodeMirror 5

Used by the `python_sidecar` filter editor for Python syntax highlighting.

- **Version:** 5.65.16
- **License:** MIT (© by Marijn Haverbeke and others)
- **Source:** https://cdnjs.cloudflare.com/ajax/libs/codemirror/5.65.16/
  - `codemirror.js`  ← `codemirror.min.js`
  - `codemirror.css` ← `codemirror.min.css`
  - `python.js`      ← `mode/python/python.min.js`

CodeMirror 5 (not 6) is used deliberately: it ships as a self-contained
file drop with no bundler/build step, which fits the single-binary,
embedded-assets model. Served only on the Python editor page.

To refresh, re-download the three files above from the same base URL.
