# Vendored html5lib tree-construction conformance tests

The `.dat` files in this directory are the WHATWG/html5lib
tree-construction conformance corpus, fetched by
`xtask/fetch-html5lib-tests.py` from
<https://github.com/web-platform-tests/wpt/tree/master/html/syntax/parsing/resources>
(2026-08-24). Two licenses apply — the original html5lib-tests copyright
(MIT) and web-platform-tests' own project license (3-Clause BSD), which
now governs the copy fetched from there — see `LICENSE` for both, in
full.

## Why wpt, not html5lib-tests

html5lib-tests' own `tree-construction/` directory is gone — its
`README.md` (as of 2026-08-24) says tree-construction tests are "now
solely maintained on web-platform-tests". wpt kept the exact same `.dat`
format and content as a resource for its browser-based
`html5lib_write.html` test runner; this crate's harness
(`tests/html5lib_conformance.rs`) reads the `.dat` files directly rather
than running that browser harness.

## Format

See each file's own content and
<https://github.com/web-platform-tests/wpt/blob/master/html/syntax/parsing/resources/README.md>
for the authoritative format description
(`#data`/`#errors`/`#new-errors`/`#document-fragment`/`#script-off`/
`#script-on`/`#document` sections). Summary, as consumed by this crate's
harness:

- `#data` — input HTML, passed to `parse()` unchanged.
- `#errors`/`#new-errors` — expected parse errors. Not applicable: this
  crate has no diagnostics, these sections are skipped entirely.
- `#document-fragment` — marks a fragment-parsing test case (context
  element for the HTML fragment parsing algorithm). This crate has no
  fragment parsing (`parse()` only does full-document parsing) — any
  test case with this section is skipped.
- `#script-off`/`#script-on` — this crate always models scripting as
  disabled (see `README.md`'s "Known limitations" — well, more
  precisely: `src/tree_builder.rs`'s "in head noscript" doc comment).
  `#script-on`-only cases are skipped (not applicable); `#script-off`-only
  and unmarked cases (meant to hold in both modes) are run.
- `#document` — the expected tree dump, compared against this crate's
  own dump of `parse()`'s output (`tests/support/dump.rs`).

## Updating

Re-run `python3 xtask/fetch-html5lib-tests.py` to refresh from upstream.
If wpt's directory listing changes (new files added/removed), update the
`FILES` list at the top of that script from the GitHub API — see the
script's own doc comment.
