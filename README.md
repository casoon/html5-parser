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
implemented and wired end to end. No public API yet (see Scope above —
everything is crate-internal until Step 2); not published to crates.io.

### Known limitations

Two deliberate, evidence-based scope decisions — not oversights:

- **Frameset-related insertion modes** ("in frameset", "after frameset",
  "after after frameset") are not implemented; `<frameset>`/`<frame>`/
  `<noframes>` content is not parsed per spec. Rationale: `html-conform`'s
  RELAX NG schema can never validate a frameset document, so there is no
  evidence this is needed for Step 1's scope.
- **`<template>`** is treated as an ordinary element — no inert
  content-fragment semantics, no template insertion-modes stack, no
  shadow-root handling (§13.2.6.4.4's simplification). Rationale: no
  evidence `html-conform` needs real template-content semantics.

If `html-conform`'s needs change (e.g. it starts validating documents
that can contain `<frameset>`, or gains real `<template>`-content test
cases), these decisions should be revisited explicitly rather than
half-implemented silently.

## License

MIT — see `LICENSE`.
