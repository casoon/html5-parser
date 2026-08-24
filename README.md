# html5-parser

[![crates.io](https://img.shields.io/crates/v/html5-parser.svg)](https://crates.io/crates/html5-parser)
[![docs.rs](https://img.shields.io/docsrs/html5-parser)](https://docs.rs/html5-parser)

A pure-Rust [WHATWG HTML5](https://html.spec.whatwg.org/multipage/parsing.html)
parser: tokenizer + full tree construction, transcribed directly from the
spec rather than ported from another implementation.

- **Spec-derived, not guessed.** Every state and algorithm is transcribed
  from the raw WHATWG HTML parsing specification text — adoption agency,
  foster parenting, foreign content (SVG/MathML), frameset, `<template>`'s
  real content-fragment model, all included.
- **100% conformant** against the
  [html5lib-tests](https://github.com/html5lib/html5lib-tests)
  tree-construction corpus — 1,726/1,726 applicable cases pass (see
  [Testing](#testing) below).
- **Per-node source positions.** Every node in the resulting tree carries
  its line/column/byte offset in the original input, not just the parsed
  structure — useful for anything that needs to point back at source
  (linters, validators, diagnostics).
- **No dependencies, no `unsafe`.**

## Usage

```rust
use html5_parser::{parse, Document, NodeId, NodeKind};

fn main() {
    let document = parse("<!DOCTYPE html><title>Hi</title><h1>Hello, world!</h1>");
    print_tree(&document, document.root(), 0);
}

fn print_tree(document: &Document, node: NodeId, depth: usize) {
    let indent = "  ".repeat(depth);
    match &document.node(node).kind {
        NodeKind::Element { name, .. } => println!("{indent}<{name}>"),
        NodeKind::Text { content } => println!("{indent}{content:?}"),
        _ => {}
    }
    for child in document.children(node) {
        print_tree(document, child, depth + 1);
    }
}
```

`Document`'s read-only API (`root`/`node`/`parent`/`children`) is
intentionally minimal — just enough to walk the tree and read each
node's kind and source position. See [docs.rs](https://docs.rs/html5-parser)
for the full API.

## Known limitations

Two narrow, deliberately out-of-scope sub-features, neither exercised by
the html5lib-tests corpus:

- **`<template>`**'s classic content-fragment model (a real, inert
  `NodeKind::DocumentFragment` per template element) is implemented, but
  two much newer, still-evolving sub-features layered onto `<template>`
  in the current spec are not: declarative shadow DOM (`shadowrootmode`
  and friends) and content patching (the `for` attribute) — both would
  require modeling shadow roots/custom element registries this crate has
  no other use for.
- **`<selectedcontent>`**'s option-mirroring (the "customizable
  `<select>`" proposal) is implemented for ordinary, non-scripted
  parse-time use, but simplifies a few edge cases: no `multiple`
  `<select>` support, and a practically- rather than fully-generally-
  scoped "list of options" walk (doesn't handle every possible
  `<optgroup>` nesting shape).

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

## Normative basis

Implementation decisions are derived from the
[WHATWG HTML parsing specification](https://html.spec.whatwg.org/multipage/parsing.html).
Other implementations (e.g. `html5ever`) are explanatory references only,
not a source to copy code from.

## Architecture (working title)

```
HTML input (string) → tokenizer (WHATWG tokenizer state machine)
                     → tree_builder (WHATWG tree-construction algorithm,
                       incl. foreign content / SVG / MathML)
                     → document (element/text/comment tree with positions)
```

## Origin

This crate started as a staged, sibling-project effort: build only what
[`html-conform`](https://github.com/casoon/html-conform) needed to
replace its previous HTML5-parsing dependency, prove that against real
usage, and only then decide on a public API and whether to publish
standalone. In practice, full spec coverage and a first `crates.io`
publish happened ahead of that cross-repo validation step, on direct
request — see `plan/DECISIONS.md` for the decision log and `plan/`
generally for the phase-by-phase implementation history (not tracked in
git, see `CLAUDE.md`).

## License

MIT — see `LICENSE`.
