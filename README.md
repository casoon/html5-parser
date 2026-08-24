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
a generic public API yet, that's Step 2). Published on crates.io as
[`html5-parser`](https://crates.io/crates/html5-parser).

### Known limitations

Three gaps were tracked here, evidence-based (per Step 1's scope, above)
rather than oversights, closed out one by one ahead of the first
`crates.io` publish (see `plan/DECISIONS.md`), independently of whether
`html-conform` itself ends up needing each one — **all three are now
done**:

- ~~Frameset-related insertion modes not implemented~~ — **done**:
  "in frameset"/"after frameset"/"after after frameset" (§13.2.6.4.18-21)
  are implemented, including `<frameset>` correctly replacing (rather
  than nesting under) `<body>` per §13.2.6.4.7/.4.6's real rules. See
  `plan/04-frameset.md`.
- ~~`<template>` treated as an ordinary element~~ — **done** (the
  classic content model): `<template>` gets a real, separate
  `NodeKind::DocumentFragment` "template contents" (§13.2.6.4.4/.16,
  the stack of template insertion modes, the active-formatting-elements
  marker). **Still not implemented within this**: two much newer,
  still-evolving sub-features layered onto `<template>` in the current
  spec, neither exercised by the html5lib-tests corpus: declarative
  shadow DOM (`shadowrootmode` and friends) and content patching (the
  `for` attribute) — both would require modeling shadow roots/custom
  element registries this crate has no other use for. See
  `plan/05-template.md`.
- ~~`<selectedcontent>` treated as an ordinary element~~ — **done**,
  with a scope note: this isn't actually a tree-construction (§13.2.6)
  feature at all — it's the `<option>` element's own HTML-parser-specific
  hook (form-elements.html §4.10.10/.17, "maybe clone an option into
  selectedcontent", run when an `option` is popped off the stack of open
  elements), simplified to this crate's parse-time-only, non-scripted
  needs (no live selectedness mutation, no `multiple` `<select>`
  support, a practically- rather than fully-generally-scoped "list of
  options"/`disabled`-flag walk). See `plan/06-selectedcontent.md`.

## Testing

Besides hand-written unit/end-to-end tests (`src/tokenizer.rs`,
`src/tree_builder.rs`, `src/lib.rs`, `src/document.rs`), `cargo test`
also runs `tests/html5lib_conformance.rs`: every applicable case
(full-document, non-fragment, non-scripting — see
`tests/html5lib-tests/README.md`) from the vendored
[html5lib-tests](https://github.com/html5lib/html5lib-tests)
tree-construction corpus — currently all 1,726 applicable cases pass
(100%). `tests/html5lib_known_failures.txt` is currently empty (kept,
rather than deleted, as the harness's regression-tracking mechanism —
see that file's header — for whenever a future corpus refresh or code
change introduces a real one).

## License

MIT — see `LICENSE`.
