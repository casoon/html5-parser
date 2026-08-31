// Public API surface: `parse` plus the read-only tree types
// (`Document`/`NodeId`/`NodeKind`/`Attribute`/`Position`/`Node`/`Children`)
// needed to walk its output — just enough for html-conform's
// `src/infoset.rs::normalize()` to consume, per Step 1 of this crate's
// two-stage scope (see `CLAUDE.md`). `Tokenizer`/`TreeBuilder` and
// everything else stay crate-internal; there's no commitment to their
// shape yet. See plan/DECISIONS.md.

mod document;
mod entities;
mod tokenizer;
mod tree_builder;

pub use document::{Attribute, Children, Document, Node, NodeId, NodeKind};
pub use tokenizer::{ParseError, ParseErrorKind, Position};

use tokenizer::Tokenizer;
use tree_builder::TreeBuilder;

/// [`parse`]'s return value: the parsed [`Document`] tree plus every
/// WHATWG "parse error" (§13.2.2) encountered along the way. Both fields
/// together, not a `Result` — parse errors are never fatal, `document`
/// is always complete regardless of how many occurred (see
/// [`ParseError`]'s doc comment).
#[derive(Debug)]
pub struct ParseResult {
    pub document: Document,
    pub errors: Vec<ParseError>,
}

/// The driver loop (§13.2 "Parsing HTML documents", the "tokenization and
/// tree construction" step): parses `input` into a [`Document`] tree with
/// per-node source positions, feeding it through the tokenizer and handing
/// each token to the tree builder, applying the two pieces of feedback
/// tree construction sends back to the tokenizer — a state switch
/// (`Tokenizer::switch_to`, for RCDATA/RAWTEXT/script-data/PLAINTEXT
/// elements) and the foreign-content flag (`Tokenizer::set_in_foreign_content`,
/// consulted only by CDATA-section handling).
///
/// The tokenizer's iterator yields exactly one `Eof` token and then ends
/// (`None`) on the next call, so the loop needs no separate condition for
/// *when* to stop feeding it tokens. `TreeBuilder::stop_parsing` (§13.2.7
/// "The end") still runs once, explicitly, right after — its one
/// tree-shape-relevant step ("pop all the nodes off the stack of open
/// elements") isn't implied by the loop simply ending.
///
/// Returns [`ParseResult`], not a bare [`Document`], as of Phase 07
/// (`plan/07-parse-errors.md`) — `errors` covers every tokenizer-level
/// parse error (`src/tokenizer.rs`'s `error()` call sites) plus, as of
/// Phase 08 (`plan/08-tree-construction-errors.md`), the
/// tree-construction-level (§13.2.6) conditions listed there. Both
/// sources are merged and sorted by source position, so `errors` is
/// always in document order regardless of which stage produced each
/// entry (the two stages interleave: the tokenizer runs ahead of the
/// tree builder token by token).
pub fn parse(input: &str) -> ParseResult {
    let mut tokenizer = Tokenizer::new(input);
    let mut tree_builder = TreeBuilder::new();
    while let Some(token) = tokenizer.next() {
        if let Some(state) = tree_builder.process_token(&token.kind, token.position) {
            tokenizer.switch_to(state);
        }
        tokenizer.set_in_foreign_content(tree_builder.is_in_foreign_content());
    }
    tree_builder.stop_parsing();
    let mut errors = tokenizer.take_errors();
    errors.append(&mut tree_builder.take_errors());
    // Stable, so two errors reported at the same position keep their
    // relative order (tokenizer-first, matching the order the stages
    // actually observe a given token).
    errors.sort_by_key(|error| error.position.byte_offset);
    ParseResult {
        document: tree_builder.into_document(),
        errors,
    }
}

#[cfg(test)]
mod tests {
    use super::parse;
    use crate::document::{Document, NodeId, NodeKind};
    use crate::tree_builder::{HTML_NAMESPACE, MATHML_NAMESPACE, SVG_NAMESPACE};

    /// Navigates root -> html -> body, the common starting point for most
    /// of this module's tree-shape assertions.
    fn body_of(document: &Document) -> NodeId {
        let root = document.root();
        // Skip a possible leading DOCTYPE sibling to find the html
        // element, root's first *element* child rather than its first
        // child outright.
        let html = document
            .children(root)
            .find(|&node| matches!(document.node(node).kind, NodeKind::Element { .. }))
            .unwrap();
        document.children(html).nth(1).unwrap()
    }

    #[test]
    fn parses_a_minimal_document_into_the_expected_tree_shape() {
        let document = parse(
            "<!DOCTYPE html><html><head><title>Hi</title></head><body><p>Hello</p></body></html>",
        )
        .document;

        let root = document.root();
        let root_children: Vec<_> = document.children(root).collect();
        assert_eq!(root_children.len(), 2);
        assert_eq!(
            document.node(root_children[0]).kind,
            NodeKind::Doctype {
                name: Some("html".to_owned()),
                public_identifier: Some(String::new()),
                system_identifier: Some(String::new()),
            }
        );

        let html = root_children[1];
        let html_children: Vec<_> = document.children(html).collect();
        assert_eq!(html_children.len(), 2);
        let (head, body) = (html_children[0], html_children[1]);

        let title = document.children(head).next().unwrap();
        let NodeKind::Element { name, .. } = &document.node(title).kind else {
            unreachable!()
        };
        assert_eq!(name, "title");
        let title_text = document.children(title).next().unwrap();
        assert_eq!(
            document.node(title_text).kind,
            NodeKind::Text {
                content: "Hi".to_owned()
            }
        );

        let p = document.children(body).next().unwrap();
        let NodeKind::Element { name, .. } = &document.node(p).kind else {
            unreachable!()
        };
        assert_eq!(name, "p");
        let p_text = document.children(p).next().unwrap();
        assert_eq!(
            document.node(p_text).kind,
            NodeKind::Text {
                content: "Hello".to_owned()
            }
        );
    }

    #[test]
    fn parses_implied_html_head_body_when_missing() {
        let document = parse("<p>Hello</p>").document;

        let root = document.root();
        assert_eq!(document.children(root).count(), 1);
        let html = document.children(root).next().unwrap();
        let html_children: Vec<_> = document.children(html).collect();
        assert_eq!(html_children.len(), 2);
        let body = html_children[1];

        let p = document.children(body).next().unwrap();
        let NodeKind::Element { name, .. } = &document.node(p).kind else {
            unreachable!()
        };
        assert_eq!(name, "p");
    }

    #[test]
    fn rcdata_element_content_is_not_parsed_as_markup() {
        let document = parse("<title><b>not bold</b></title>").document;

        let root = document.root();
        let html = document.children(root).next().unwrap();
        let head = document.children(html).next().unwrap();
        let title = document.children(head).next().unwrap();
        let text = document.children(title).next().unwrap();
        assert_eq!(
            document.node(text).kind,
            NodeKind::Text {
                content: "<b>not bold</b>".to_owned()
            }
        );
    }

    #[test]
    fn parse_syncs_in_foreign_content_for_cdata_sections() {
        // Exercises the one piece of driver-loop wiring no lower-level
        // test can reach: Tokenizer::set_in_foreign_content is only
        // ever called from here, based on the *real* TreeBuilder state
        // after processing the <svg> start tag.
        let document = parse("<svg><![CDATA[hello]]></svg>").document;

        let root = document.root();
        let html = document.children(root).next().unwrap();
        let body = document.children(html).nth(1).unwrap();
        let svg = document.children(body).next().unwrap();
        let content = document.children(svg).next().unwrap();
        assert_eq!(
            document.node(content).kind,
            NodeKind::Text {
                content: "hello".to_owned()
            }
        );
    }

    #[test]
    fn cdata_outside_foreign_content_becomes_a_bogus_comment() {
        let document = parse("<p><![CDATA[hello]]></p>").document;

        let root = document.root();
        let html = document.children(root).next().unwrap();
        let body = document.children(html).nth(1).unwrap();
        let p = document.children(body).next().unwrap();
        let content = document.children(p).next().unwrap();
        assert_eq!(
            document.node(content).kind,
            NodeKind::Comment {
                content: "[CDATA[hello]]".to_owned()
            }
        );
    }

    // The test matrix from plan/03-tree-construction.md's "Testmatrix"
    // step: cases ported from html-conform's own src/infoset.rs test
    // matrix (so the eventual switch from its current HTML5-parsing
    // dependency to this crate is behavior-preserving), plus spec-
    // derived cases html-conform doesn't cover at all (adoption agency,
    // quirks mode's effect on tree shape, tables without explicit
    // tbody/tr) that the phase's exit criteria call out explicitly.

    #[test]
    fn optional_end_tags_produce_sibling_li_elements() {
        // Ported from html-conform's optional_end_tags_produce_sibling_elements.
        let document = parse("<ul><li>a<li>b</ul>").document;
        let body = body_of(&document);
        let ul = document.children(body).next().unwrap();
        let items: Vec<_> = document.children(ul).collect();
        assert_eq!(items.len(), 2);
        for (&li, expected_text) in items.iter().zip(["a", "b"]) {
            let NodeKind::Element { name, .. } = &document.node(li).kind else {
                unreachable!()
            };
            assert_eq!(name, "li");
            let text = document.children(li).next().unwrap();
            assert_eq!(
                document.node(text).kind,
                NodeKind::Text {
                    content: expected_text.to_owned()
                }
            );
        }
    }

    #[test]
    fn svg_element_keeps_svg_namespace_end_to_end() {
        // Ported from html-conform's svg_elements_keep_svg_namespace.
        let document = parse("<svg><circle/></svg>").document;
        let body = body_of(&document);
        let svg = document.children(body).next().unwrap();
        assert_eq!(
            document.node(svg).kind,
            NodeKind::Element {
                name: "svg".to_owned(),
                namespace: Some(SVG_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
        let circle = document.children(svg).next().unwrap();
        assert_eq!(
            document.node(circle).kind,
            NodeKind::Element {
                name: "circle".to_owned(),
                namespace: Some(SVG_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
    }

    #[test]
    fn mathml_element_keeps_mathml_namespace_end_to_end() {
        // Ported from html-conform's mathml_elements_keep_mathml_namespace.
        let document = parse("<math><mi>x</mi></math>").document;
        let body = body_of(&document);
        let math = document.children(body).next().unwrap();
        assert_eq!(
            document.node(math).kind,
            NodeKind::Element {
                name: "math".to_owned(),
                namespace: Some(MATHML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
        let mi = document.children(math).next().unwrap();
        assert_eq!(
            document.node(mi).kind,
            NodeKind::Element {
                name: "mi".to_owned(),
                namespace: Some(MATHML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
        let text = document.children(mi).next().unwrap();
        assert_eq!(
            document.node(text).kind,
            NodeKind::Text {
                content: "x".to_owned()
            }
        );
    }

    #[test]
    fn script_content_is_not_tokenized_as_markup() {
        // Ported from html-conform's script_and_style_content_normalize_to_plain_text.
        let document = parse("<script>1 < 2;</script>").document;
        let root = document.root();
        let html = document.children(root).next().unwrap();
        let head = document.children(html).next().unwrap();
        let script = document.children(head).next().unwrap();
        let NodeKind::Element { name, .. } = &document.node(script).kind else {
            unreachable!()
        };
        assert_eq!(name, "script");
        let text = document.children(script).next().unwrap();
        assert_eq!(
            document.node(text).kind,
            NodeKind::Text {
                content: "1 < 2;".to_owned()
            }
        );
    }

    #[test]
    fn style_content_is_not_tokenized_as_markup() {
        // Ported from html-conform's script_and_style_content_normalize_to_plain_text.
        let document = parse("<style>a{color:red}</style>").document;
        let root = document.root();
        let html = document.children(root).next().unwrap();
        let head = document.children(html).next().unwrap();
        let style = document.children(head).next().unwrap();
        let NodeKind::Element { name, .. } = &document.node(style).kind else {
            unreachable!()
        };
        assert_eq!(name, "style");
        let text = document.children(style).next().unwrap();
        assert_eq!(
            document.node(text).kind,
            NodeKind::Text {
                content: "a{color:red}".to_owned()
            }
        );
    }

    #[test]
    fn named_character_references_resolve_to_decoded_text() {
        // Ported from html-conform's named_entities_resolve_to_decoded_text.
        let document = parse("<p>&amp; &copy;</p>").document;
        let body = body_of(&document);
        let p = document.children(body).next().unwrap();
        let text = document.children(p).next().unwrap();
        assert_eq!(
            document.node(text).kind,
            NodeKind::Text {
                content: "& \u{a9}".to_owned()
            }
        );
    }

    #[test]
    fn custom_element_gets_html_namespace_like_any_plain_element() {
        // Ported from html-conform's custom_element_gets_xhtml_namespace_like_any_plain_element.
        let document = parse("<my-widget>hi</my-widget>").document;
        let body = body_of(&document);
        let widget = document.children(body).next().unwrap();
        assert_eq!(
            document.node(widget).kind,
            NodeKind::Element {
                name: "my-widget".to_owned(),
                namespace: Some(HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
        let text = document.children(widget).next().unwrap();
        assert_eq!(
            document.node(text).kind,
            NodeKind::Text {
                content: "hi".to_owned()
            }
        );
    }

    #[test]
    fn xml_lang_attribute_stays_a_literal_unnamespaced_attribute_name() {
        // On a plain HTML element (not inside SVG/MathML foreign
        // content), general HTML5 parsing never namespace-splits
        // xml:lang - "adjust foreign attributes" (§13.2.6.1) only
        // applies while inserting a foreign element. html-conform's own
        // xml:lang tests are about its RELAX-NG-schema adapter's own
        // remapping on top of this, a schema-validation-specific
        // concern, not a general parsing fact this crate should assert
        // - this just verifies the parser's own (unremapped) output.
        let document = parse(r#"<p xml:lang="de">hi</p>"#).document;
        let body = body_of(&document);
        let p = document.children(body).next().unwrap();
        let NodeKind::Element { attributes, .. } = &document.node(p).kind else {
            unreachable!()
        };
        assert_eq!(attributes.len(), 1);
        assert_eq!(attributes[0].name, "xml:lang");
        assert_eq!(attributes[0].value, "de");
        assert_eq!(attributes[0].namespace, None);
    }

    #[test]
    fn table_without_tbody_or_tr_gets_them_synthesized() {
        // Spec-derived case (§13.2.6.4.9/.13/.14's implied-tag rules),
        // not covered by html-conform's own test matrix at all.
        let document = parse("<table><td>x</td></table>").document;
        let body = body_of(&document);
        let table = document.children(body).next().unwrap();
        let tbody = document.children(table).next().unwrap();
        let NodeKind::Element { name, .. } = &document.node(tbody).kind else {
            unreachable!()
        };
        assert_eq!(name, "tbody");
        let tr = document.children(tbody).next().unwrap();
        let NodeKind::Element { name, .. } = &document.node(tr).kind else {
            unreachable!()
        };
        assert_eq!(name, "tr");
        let td = document.children(tr).next().unwrap();
        let NodeKind::Element { name, .. } = &document.node(td).kind else {
            unreachable!()
        };
        assert_eq!(name, "td");
        let text = document.children(td).next().unwrap();
        assert_eq!(
            document.node(text).kind,
            NodeKind::Text {
                content: "x".to_owned()
            }
        );
    }

    #[test]
    fn adoption_agency_spec_example_misnested_b_i_tags() {
        // §13.2.10.1 "Misnested tags: <b><i></b></i>" - the spec's own
        // fully worked, non-normative example, including its final DOM
        // tree spelled out in prose: html > head, body > p > [#text:1,
        // b > [#text:2, i > #text:3], i > #text:4, #text:5]. Not
        // covered by html-conform's own test matrix at all.
        let document = parse("<p>1<b>2<i>3</b>4</i>5</p>").document;
        let body = body_of(&document);
        let p = document.children(body).next().unwrap();
        let p_children: Vec<_> = document.children(p).collect();
        assert_eq!(p_children.len(), 4);
        assert_eq!(
            document.node(p_children[0]).kind,
            NodeKind::Text {
                content: "1".to_owned()
            }
        );

        let b = p_children[1];
        let NodeKind::Element { name, .. } = &document.node(b).kind else {
            unreachable!()
        };
        assert_eq!(name, "b");
        let b_children: Vec<_> = document.children(b).collect();
        assert_eq!(b_children.len(), 2);
        assert_eq!(
            document.node(b_children[0]).kind,
            NodeKind::Text {
                content: "2".to_owned()
            }
        );
        let inner_i = b_children[1];
        let NodeKind::Element { name, .. } = &document.node(inner_i).kind else {
            unreachable!()
        };
        assert_eq!(name, "i");
        let inner_i_text = document.children(inner_i).next().unwrap();
        assert_eq!(
            document.node(inner_i_text).kind,
            NodeKind::Text {
                content: "3".to_owned()
            }
        );

        let outer_i = p_children[2];
        let NodeKind::Element { name, .. } = &document.node(outer_i).kind else {
            unreachable!()
        };
        assert_eq!(name, "i");
        let outer_i_text = document.children(outer_i).next().unwrap();
        assert_eq!(
            document.node(outer_i_text).kind,
            NodeKind::Text {
                content: "4".to_owned()
            }
        );

        // The trailing "5" lands as p's own child, not inside the
        // reconstructed i - </i> closes before it arrives.
        assert_eq!(
            document.node(p_children[3]).kind,
            NodeKind::Text {
                content: "5".to_owned()
            }
        );
    }

    #[test]
    fn adoption_agency_spec_example_indirectly_nests_two_a_elements_via_table_misnesting() {
        // The spec's own example, quoted directly in §13.2.6.4.7's <a>
        // start-tag rule: "In the non-conforming stream <a href="a">
        // a<table><a href="b">b</table>x, the first a element would be
        // closed upon seeing the second one, and the "x" character
        // would be inside a link to "b", not to "a" [...] The result is
        // that the two a elements are indirectly nested inside each
        // other." Not covered by html-conform's own test matrix.
        let document = parse(r#"<a href="a">a<table><a href="b">b</table>x"#).document;
        let body = body_of(&document);
        let body_children: Vec<_> = document.children(body).collect();
        assert_eq!(body_children.len(), 2);

        let a1 = body_children[0];
        let a1_children: Vec<_> = document.children(a1).collect();
        assert_eq!(a1_children.len(), 3);
        assert_eq!(
            document.node(a1_children[0]).kind,
            NodeKind::Text {
                content: "a".to_owned()
            }
        );
        let a2 = a1_children[1];
        let NodeKind::Element { name, .. } = &document.node(a2).kind else {
            unreachable!()
        };
        assert_eq!(name, "a");
        let a2_text = document.children(a2).next().unwrap();
        assert_eq!(
            document.node(a2_text).kind,
            NodeKind::Text {
                content: "b".to_owned()
            }
        );
        let table = a1_children[2];
        let NodeKind::Element { name, .. } = &document.node(table).kind else {
            unreachable!()
        };
        assert_eq!(name, "table");
        assert_eq!(document.children(table).count(), 0);

        // The final "x" lands in a *fresh* a element (href="b",
        // reconstructed from the active formatting elements list),
        // a sibling of a1 - not a1 itself.
        let a3 = body_children[1];
        let NodeKind::Element { name, .. } = &document.node(a3).kind else {
            unreachable!()
        };
        assert_eq!(name, "a");
        let a3_text = document.children(a3).next().unwrap();
        assert_eq!(
            document.node(a3_text).kind,
            NodeKind::Text {
                content: "x".to_owned()
            }
        );
    }

    #[test]
    fn quirks_mode_changes_whether_table_closes_an_open_p_element() {
        // Spec-derived case, not covered by html-conform's own test
        // matrix (which drops DOCTYPE/quirks-mode entirely) - the one
        // place quirks mode actually shapes the produced tree
        // (§13.2.6.4.7's <table> start-tag rule).
        let no_quirks = parse("<!DOCTYPE html><p><table></table>").document;
        let body = body_of(&no_quirks);
        let children: Vec<_> = no_quirks.children(body).collect();
        assert_eq!(children.len(), 2);
        let NodeKind::Element { name, .. } = &no_quirks.node(children[0]).kind else {
            unreachable!()
        };
        assert_eq!(name, "p");
        assert_eq!(no_quirks.children(children[0]).count(), 0);
        let NodeKind::Element { name, .. } = &no_quirks.node(children[1]).kind else {
            unreachable!()
        };
        assert_eq!(name, "table");

        let quirks = parse("<p><table></table>").document; // no DOCTYPE at all -> quirks mode
        let body = body_of(&quirks);
        let children: Vec<_> = quirks.children(body).collect();
        assert_eq!(children.len(), 1);
        let NodeKind::Element { name, .. } = &quirks.node(children[0]).kind else {
            unreachable!()
        };
        assert_eq!(name, "p");
        let p_children: Vec<_> = quirks.children(children[0]).collect();
        assert_eq!(p_children.len(), 1);
        let NodeKind::Element { name, .. } = &quirks.node(p_children[0]).kind else {
            unreachable!()
        };
        assert_eq!(name, "table");
    }

    // The next two tests pin the minimal reproductions of two infinite
    // loops the html5lib-tests conformance corpus (tests/html5lib_conformance.rs)
    // surfaced — both fixed in `tree_builder.rs`. Neither asserts much
    // about the resulting tree shape; the property under test is that
    // `parse` returns at all (a regression would hang this test rather
    // than fail it cleanly, same as it hung `cargo test` before the fix).

    #[test]
    fn end_tag_that_walks_out_of_foreign_content_does_not_loop_forever() {
        // `</a>` while the current node is `<svg>` (no HTML-special
        // element between them) sends foreign content's "any other end
        // tag" rule (§13.2.6.5) all the way up to the first HTML-namespace
        // ancestor without ever popping anything itself — it must then
        // hand off to that insertion mode's own HTML-content rules
        // directly, not via `TokenOutcome::Reprocess` (which re-checks
        // foreign-content dispatch against the very node whose
        // foreign-namespace-ness never changed, looping forever).
        let document = parse("<a><svg></a>").document;
        let body = body_of(&document);
        assert_eq!(document.children(body).count(), 1);
    }

    #[test]
    fn template_end_tag_resets_a_stale_insertion_mode() {
        // `<template>` inside `<thead>` implicitly opens a `<tr><td>`
        // (this crate treats `<template>` as a plain element — no
        // template insertion-modes stack, see README.md's "Known
        // limitations" — so the insertion mode active for that implicit
        // `<td>`, `InCell`, is never restored). `</template>` then pops
        // `<td>`/`<tr>`/`<template>` off the stack without resetting the
        // insertion mode; the next token (`</table>`) processed under
        // the still-`InCell` mode assumes a `td`/`th` remains on the
        // stack, which no longer holds — `close_the_cell` would then
        // pop the now-empty stack forever looking for one.
        let document = parse("<table><thead><template><td></template></table>").document;
        let body = body_of(&document);
        assert_eq!(document.children(body).count(), 1);
    }

    #[test]
    fn frameset_document_replaces_body_and_ignores_stray_text() {
        // html5lib-tests' tests2.dat#5: a bare `<frameset>` after the
        // DOCTYPE, with trailing character data "in frameset" mode's
        // "anything else" rule drops entirely (no `<body>` at all in
        // the result — frameset and body are mutually exclusive).
        let document = parse("<!DOCTYPE html><frameset>test").document;

        let root = document.root();
        let root_children: Vec<_> = document.children(root).collect();
        assert_eq!(root_children.len(), 2);
        assert_eq!(
            document.node(root_children[0]).kind,
            NodeKind::Doctype {
                name: Some("html".to_owned()),
                public_identifier: Some(String::new()),
                system_identifier: Some(String::new()),
            }
        );
        let html = root_children[1];

        let html_children: Vec<_> = document.children(html).collect();
        assert_eq!(html_children.len(), 2);
        let NodeKind::Element { name, .. } = &document.node(html_children[0]).kind else {
            unreachable!()
        };
        assert_eq!(name, "head");
        let frameset = html_children[1];
        let NodeKind::Element { name, .. } = &document.node(frameset).kind else {
            unreachable!()
        };
        assert_eq!(name, "frameset");
        assert_eq!(document.children(frameset).count(), 0);
    }

    #[test]
    fn template_content_is_a_separate_fragment_from_the_template_element() {
        // html5lib-tests' template.dat#0: `<template>`'s real content
        // model (§13.2.6.4.4/.16) — its child is a `DocumentFragment`
        // ("template contents"), not the text directly.
        let document = parse("<body><template>Hello</template>").document;
        let body = body_of(&document);

        let template = document.children(body).next().unwrap();
        let NodeKind::Element { name, .. } = &document.node(template).kind else {
            unreachable!()
        };
        assert_eq!(name, "template");

        let template_children: Vec<_> = document.children(template).collect();
        assert_eq!(template_children.len(), 1);
        let content = template_children[0];
        assert_eq!(document.node(content).kind, NodeKind::DocumentFragment);

        let content_children: Vec<_> = document.children(content).collect();
        assert_eq!(content_children.len(), 1);
        assert_eq!(
            document.node(content_children[0]).kind,
            NodeKind::Text {
                content: "Hello".to_owned()
            }
        );
    }

    #[test]
    fn selected_option_content_is_mirrored_into_selectedcontent() {
        // html5lib-tests' webkit02.dat#47: the explicitly `selected`
        // option (not the first one) is the one mirrored, and — since
        // it's the last token in the input — only `stop_parsing`'s
        // final pop of the still-open `<option>` makes that observable.
        let document =
            parse("<select><button><selectedcontent></button><option>X<option selected>Y").document;
        let body = body_of(&document);

        let select = document.children(body).next().unwrap();
        let button = document.children(select).next().unwrap();
        let selectedcontent = document.children(button).next().unwrap();
        let selectedcontent_children: Vec<_> = document.children(selectedcontent).collect();
        assert_eq!(selectedcontent_children.len(), 1);
        assert_eq!(
            document.node(selectedcontent_children[0]).kind,
            NodeKind::Text {
                content: "Y".to_owned()
            }
        );
    }
}

/// Phase 08 (`plan/08-tree-construction-errors.md`): one minimal
/// trigger per tree-construction [`ParseErrorKind`] variant, mirroring
/// Phase 07's per-variant table test for the tokenizer-level kinds.
///
/// Each case asserts the expected kind is *present*, not that it's the
/// only one — several of these inputs legitimately raise more than one
/// error (an unclosed `<div>`, for instance, is both a stray-end-tag
/// trigger and an unclosed-element-at-EOF trigger), and pinning the
/// exact multiset would make the tests brittle without testing anything
/// more.
#[cfg(test)]
mod tree_construction_error_tests {
    use super::parse;
    use crate::tokenizer::ParseErrorKind;

    fn kinds(input: &str) -> Vec<ParseErrorKind> {
        parse(input)
            .errors
            .into_iter()
            .map(|error| error.kind)
            .collect()
    }

    #[track_caller]
    fn assert_raises(input: &str, expected: ParseErrorKind) {
        let raised = kinds(input);
        assert!(
            raised.contains(&expected),
            "expected {expected:?} for {input:?}, got {raised:?}"
        );
    }

    #[track_caller]
    fn assert_does_not_raise(input: &str, unexpected: ParseErrorKind) {
        let raised = kinds(input);
        assert!(
            !raised.contains(&unexpected),
            "expected no {unexpected:?} for {input:?}, got {raised:?}"
        );
    }

    /// §13.2.6.4.7's "close a p element": the `<div>` closes the open
    /// `<p>`, but `<span>` (not in the implied-end-tag set) is still
    /// open when it does.
    #[test]
    fn implied_p_end_tag_with_unclosed_elements() {
        assert_raises(
            "<!doctype html><p><span><div>",
            ParseErrorKind::ImpliedEndTagWithUnclosedElements,
        );
        // ...but a `<p>` that is itself the current node closes cleanly.
        assert_does_not_raise(
            "<!doctype html><p>text<div>",
            ParseErrorKind::ImpliedEndTagWithUnclosedElements,
        );
    }

    /// Note the explicit `<body>`: a bare `</p>` straight after the
    /// DOCTYPE is still in "before head", whose *own* "any other end
    /// tag" rule (a separate, deliberately unimplemented condition —
    /// see `plan/08-tree-construction-errors.md`) swallows it before
    /// "in body" ever sees it.
    #[test]
    fn p_end_tag_without_p_in_button_scope() {
        assert_raises(
            "<!doctype html><body></p>",
            ParseErrorKind::EndTagPWithoutPInButtonScope,
        );
        assert_does_not_raise(
            "<!doctype html><p>text</p>",
            ParseErrorKind::EndTagPWithoutPInButtonScope,
        );
    }

    /// §13.2.6.4.7's "any other end tag", step 3: `body` is in the
    /// special category, so the walk up the stack stops there.
    #[test]
    fn stray_end_tag_with_no_matching_open_element() {
        assert_raises("<!doctype html><body></span>", ParseErrorKind::StrayEndTag);
        assert_does_not_raise("<!doctype html><span>x</span>", ParseErrorKind::StrayEndTag);
    }

    #[test]
    fn end_tag_br() {
        assert_raises("<!doctype html></br>", ParseErrorKind::EndTagBr);
    }

    /// §13.2.5: a self-closing start tag whose handling rule never
    /// acknowledges the flag. `<div>` is not a void element; `<br>` is.
    #[test]
    fn self_closing_syntax_on_a_non_void_element() {
        assert_raises(
            "<!doctype html><div/></div>",
            ParseErrorKind::NonVoidHtmlElementStartTagWithTrailingSolidus,
        );
        assert_does_not_raise(
            "<!doctype html><br/>",
            ParseErrorKind::NonVoidHtmlElementStartTagWithTrailingSolidus,
        );
        // Foreign content acknowledges it too (§13.2.6.5).
        assert_does_not_raise(
            "<!doctype html><svg><rect/></svg>",
            ParseErrorKind::NonVoidHtmlElementStartTagWithTrailingSolidus,
        );
    }

    /// §13.2.6.4.7's end-of-file rule: `div` is not in the "may still be
    /// open" list, `p` is.
    #[test]
    fn eof_with_unclosed_elements() {
        assert_raises(
            "<!doctype html><div>",
            ParseErrorKind::EofWithUnclosedElements,
        );
        assert_does_not_raise(
            "<!doctype html><p>text",
            ParseErrorKind::EofWithUnclosedElements,
        );
    }

    /// §13.2.6.4.8: EOF while still inside a RAWTEXT/RCDATA/script
    /// element's text.
    #[test]
    fn eof_in_text_mode() {
        assert_raises(
            "<!doctype html><script>var x = 1;",
            ParseErrorKind::EofInTextMode,
        );
        assert_does_not_raise(
            "<!doctype html><script>var x = 1;</script>",
            ParseErrorKind::EofInTextMode,
        );
    }

    #[test]
    fn start_tag_image() {
        assert_raises(
            "<!doctype html><image src=x>",
            ParseErrorKind::StartTagImage,
        );
        assert_does_not_raise("<!doctype html><img src=x>", ParseErrorKind::StartTagImage);
    }

    #[test]
    fn nested_form() {
        assert_raises("<!doctype html><form><form>", ParseErrorKind::NestedForm);
        assert_does_not_raise(
            "<!doctype html><form></form><form>",
            ParseErrorKind::NestedForm,
        );
    }

    #[test]
    fn start_tag_table_while_a_table_is_open() {
        assert_raises(
            "<!doctype html><table><table></table></table>",
            ParseErrorKind::StartTagTableInTable,
        );
        assert_does_not_raise(
            "<!doctype html><table></table><table></table>",
            ParseErrorKind::StartTagTableInTable,
        );
    }

    /// §13.2.6.4.9's "anything else" — the foster-parenting fallback.
    #[test]
    fn misplaced_token_in_table() {
        assert_raises(
            "<!doctype html><table><select></select></table>",
            ParseErrorKind::MisplacedTokenInTable,
        );
        assert_raises(
            "<!doctype html><table><input></table>",
            ParseErrorKind::MisplacedTokenInTable,
        );
        assert_does_not_raise(
            "<!doctype html><table><tr><td>x</td></tr></table>",
            ParseErrorKind::MisplacedTokenInTable,
        );
    }

    /// §13.2.6.4.10: reported once per non-whitespace character run, not
    /// once per character.
    #[test]
    fn non_space_characters_in_table() {
        let raised = kinds("<!doctype html><table>text</table>");
        assert_eq!(
            raised
                .iter()
                .filter(|kind| **kind == ParseErrorKind::NonSpaceCharactersInTable)
                .count(),
            1,
            "got {raised:?}"
        );
        assert_does_not_raise(
            "<!doctype html><table>   </table>",
            ParseErrorKind::NonSpaceCharactersInTable,
        );
    }

    #[test]
    fn stray_end_tag_in_table() {
        assert_raises(
            "<!doctype html><table></tr></table>",
            ParseErrorKind::StrayEndTagInTable,
        );
    }

    /// §13.2.6.4.17's "anything else" — any non-whitespace content once
    /// `</body>` has been seen.
    #[test]
    fn token_after_body() {
        assert_raises(
            "<!doctype html><body></body>text",
            ParseErrorKind::TokenAfterBody,
        );
        assert_raises(
            "<!doctype html><body></body><p>x</p>",
            ParseErrorKind::TokenAfterBody,
        );
        assert_does_not_raise(
            "<!doctype html><body></body>\n",
            ParseErrorKind::TokenAfterBody,
        );
    }

    /// A second DOCTYPE, anywhere after the "initial" insertion mode has
    /// already consumed the first one.
    #[test]
    fn stray_doctype() {
        assert_raises(
            "<!doctype html><title>t</title><!doctype html>",
            ParseErrorKind::StrayDoctype,
        );
        assert_raises(
            "<!doctype html><body>x<!doctype html>",
            ParseErrorKind::StrayDoctype,
        );
        assert_does_not_raise(
            "<!doctype html><title>t</title>",
            ParseErrorKind::StrayDoctype,
        );
    }

    /// The merged error list stays in document order even though the two
    /// stages produce entries independently (`parse`'s own sort).
    #[test]
    fn errors_from_both_stages_are_merged_in_document_order() {
        let errors = parse("<!doctype html><p>&notAnEntity;<span><div></p>").errors;
        assert!(
            errors.len() >= 2,
            "expected both a tokenizer and a tree-construction error, got {errors:?}"
        );
        assert!(
            errors
                .windows(2)
                .all(|pair| pair[0].position.byte_offset <= pair[1].position.byte_offset),
            "not in document order: {errors:?}"
        );
    }
}
