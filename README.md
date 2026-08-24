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

Originally three gaps, evidence-based (per Step 1's scope, above) rather
than oversights — being closed out one by one ahead of a first
`crates.io` publish (see `plan/DECISIONS.md`), independently of whether
`html-conform` itself ends up needing each one:

- ~~Frameset-related insertion modes not implemented~~ — **done**:
  "in frameset"/"after frameset"/"after after frameset" (§13.2.6.4.18-21)
  are implemented, including `<frameset>` correctly replacing (rather
  than nesting under) `<body>` per §13.2.6.4.7/.4.6's real rules. See
  `plan/04-frameset.md`.
- **`<template>`** is still treated as an ordinary element — no inert
  content-fragment semantics, no template insertion-modes stack, no
  shadow-root handling (§13.2.6.4.4's simplification).
- **`<selectedcontent>`** (the "customizable `<select>`" proposal's
  live-mirrored-content element, a recent, still-evolving spec addition)
  is still parsed as an ordinary element — its content is not populated
  from the initially-selected `<option>` at parse time. Found via the
  html5lib-tests conformance corpus (`webkit02.dat`).

## Testing

Besides hand-written unit/end-to-end tests (`src/tokenizer.rs`,
`src/tree_builder.rs`, `src/lib.rs`), `cargo test` also runs
`tests/html5lib_conformance.rs`: every applicable case (full-document,
non-fragment, non-scripting — see `tests/html5lib-tests/README.md`) from
the vendored [html5lib-tests](https://github.com/html5lib/html5lib-tests)
tree-construction corpus — currently 1,726 applicable cases, 1,610
passing (93%). Known failures (all attributable to the two remaining
"Known limitations" above) are tracked explicitly in
`tests/html5lib_known_failures.txt` rather than silently ignored; see
that file's header for how to regenerate it.

## License

MIT — see `LICENSE`.
