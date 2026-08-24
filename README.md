# html5-parser

A pure-Rust WHATWG HTML5 tokenizer and tree-construction implementation.

## Scope

Deliberately staged, not built as a fully generic library from day one:

1. **Step 1:** only what [`html-conform`](https://github.com/casoon/html-conform)
   (a sibling project) actually needs to replace its current HTML5
   parsing dependency — a tokenizer and tree-construction implementation
   whose output can directly feed `html-conform`'s
   `src/infoset.rs::normalize()`, including per-node source positions (its
   current dependency has none). No generic public API commitment yet.
2. **Step 2:** once step 1 is proven against `html-conform`'s real usage,
   extract the generic, `html-conform`-agnostic part (a reusable WHATWG
   HTML5 tokenizer/tree-builder) as this crate's public API.

## Architecture (working title)

```
HTML input (string) → tokenizer (WHATWG tokenizer state machine)
                     → tree_builder (WHATWG tree-construction algorithm,
                       incl. foreign content / SVG / MathML)
                     → document (element/text/comment tree with positions)
```

## Normative basis

Implementation decisions are derived from the
[WHATWG HTML parsing specification](https://html.spec.whatwg.org/multipage/parsing.html).
Other implementations (e.g. `html5ever`) are explanatory references only,
not a source to copy code from.

## Status

The tokenizer (§13.2.5) and tree-construction algorithm (§13.2.6 — all
insertion modes, adoption agency, foster parenting, foreign content) are
implemented and wired end to end.

`pub fn parse(input: &str) -> Document` and the read-only tree types
(`Document`, `NodeId`, `NodeKind`, `Node`, `Attribute`, `Position`,
`Children`) are public — just enough to walk the resulting tree and read
each node's kind and source position, matching what `html-conform`'s
`src/infoset.rs::normalize()` needs. `Tokenizer`/`TreeBuilder` and
everything else stay crate-internal (see Scope above — no commitment to
a generic public API yet, that's Step 2); not published to crates.io.

### Known limitations

Three known gaps — the first two are deliberate, evidence-based scope
decisions; the third is a documented gap, not yet a deliberate decision:

- **Frameset-related insertion modes** ("in frameset", "after frameset",
  "after after frameset") are not implemented; `<frameset>`/`<frame>`/
  `<noframes>` content is not parsed per spec. Rationale: `html-conform`'s
  RELAX NG schema can never validate a frameset document, so there is no
  evidence this is needed for Step 1's scope.
- **`<template>`** is treated as an ordinary element — no inert
  content-fragment semantics, no template insertion-modes stack, no
  shadow-root handling (§13.2.6.4.4's simplification). Rationale: no
  evidence `html-conform` needs real template-content semantics.
- **`<selectedcontent>`** (the "customizable `<select>`" proposal's
  live-mirrored-content element, a recent, still-evolving spec addition)
  is parsed as an ordinary element — its content is not populated from
  the initially-selected `<option>` at parse time. Found via the
  html5lib-tests conformance corpus (`webkit02.dat`), not yet a
  deliberate scope call — no evidence either way on whether
  `html-conform` needs it, tracked as a gap until that's known.

If `html-conform`'s needs change (e.g. it starts validating documents
that can contain `<frameset>`, or gains real `<template>`-content or
`<selectedcontent>` test cases), these should be revisited explicitly
rather than half-implemented silently.

## Testing

Besides hand-written unit/end-to-end tests (`src/tokenizer.rs`,
`src/tree_builder.rs`, `src/lib.rs`), `cargo test` also runs
`tests/html5lib_conformance.rs`: every applicable case (full-document,
non-fragment, non-scripting — see `tests/html5lib-tests/README.md`) from
the vendored [html5lib-tests](https://github.com/html5lib/html5lib-tests)
tree-construction corpus — currently 1,726 applicable cases, 1,547
passing (90%). Known failures (all attributable to the three "Known
limitations" above) are tracked explicitly in
`tests/html5lib_known_failures.txt` rather than silently ignored; see
that file's header for how to regenerate it.

## License

MIT — see `LICENSE`.
