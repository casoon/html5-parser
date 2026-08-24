//! Serializes this crate's public `Document` tree into the html5lib-tests
//! `.dat` format's `#document` dump — see
//! `tests/html5lib-tests/README.md` for the format.
//!
//! Ported from the reference implementation's `serializeTree()`
//! (`resources/test.js` in
//! <https://github.com/web-platform-tests/wpt/tree/master/html/syntax/parsing>),
//! adapted to this crate's `Document`/`NodeKind` instead of a live DOM.

use html5_parser::{Document, NodeId, NodeKind};

// This crate's own namespace URI constants (`src/tree_builder.rs`) are
// `pub(crate)` — deliberately not part of the public API this
// integration test consumes (see plan/DECISIONS.md's "minimal pub API"
// entry) — so these are duplicated here. They're the WHATWG/Infra
// standard's namespace URIs, effectively immutable.
const SVG_NAMESPACE: &str = "http://www.w3.org/2000/svg";
const MATHML_NAMESPACE: &str = "http://www.w3.org/1998/Math/MathML";
const XLINK_NAMESPACE: &str = "http://www.w3.org/1999/xlink";
const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NAMESPACE: &str = "http://www.w3.org/2000/xmlns/";

/// Dumps `document`'s tree (skipping the document node itself, which the
/// `.dat` format's `#document` header already stands in for) in the
/// same format as the corpus's expected `#document` sections, so the two
/// can be compared with plain string equality.
pub fn dump_document(document: &Document) -> String {
    let mut lines = Vec::new();
    for child in document.children(document.root()) {
        dump_node(document, child, 0, &mut lines);
    }
    lines.join("\n")
}

/// `"| "` plus two spaces per level of nesting below the document node
/// (`depth` 0 for the document's direct children).
fn indent(depth: usize) -> String {
    format!("| {}", "  ".repeat(depth))
}

fn element_tag_string(name: &str, namespace: &Option<String>) -> String {
    match namespace.as_deref() {
        Some(SVG_NAMESPACE) => format!("svg {name}"),
        Some(MATHML_NAMESPACE) => format!("math {name}"),
        _ => name.to_owned(),
    }
}

/// Reconstructs the format's "attribute name string" (namespace
/// designator + local name) from this crate's `Attribute`, which keeps
/// an adjusted foreign attribute's *full* original name (e.g.
/// `"xlink:href"`) rather than splitting it into a separate
/// prefix/local-name pair — see `src/tree_builder.rs`'s
/// `FOREIGN_ATTRIBUTE_NAMESPACES` doc comment. Stripping everything up
/// to (and including) a `:` recovers the local name correctly for all
/// of that table's entries, including the colon-less `"xmlns"` case
/// (nothing to strip, the local name is the whole string, matching the
/// adjustment table's `xmlns -> (namespace xmlns, local name "xmlns")`
/// entry).
fn attribute_name_string(name: &str, namespace: &Option<String>) -> String {
    let local = name.split_once(':').map_or(name, |(_, rest)| rest);
    match namespace.as_deref() {
        Some(XLINK_NAMESPACE) => format!("xlink {local}"),
        Some(XML_NAMESPACE) => format!("xml {local}"),
        Some(XMLNS_NAMESPACE) => format!("xmlns {local}"),
        _ => name.to_owned(),
    }
}

fn dump_node(document: &Document, id: NodeId, depth: usize, lines: &mut Vec<String>) {
    let node = document.node(id);
    match &node.kind {
        NodeKind::Document => unreachable!("only called on the document node's own children"),
        NodeKind::Doctype {
            name,
            public_identifier,
            system_identifier,
        } => {
            let name = name.as_deref().unwrap_or("");
            let public_id = public_identifier.as_deref().unwrap_or("");
            let system_id = system_identifier.as_deref().unwrap_or("");
            let line = if name.is_empty() {
                format!("{}<!DOCTYPE >", indent(depth))
            } else if !public_id.is_empty() || !system_id.is_empty() {
                format!(
                    "{}<!DOCTYPE {name} \"{public_id}\" \"{system_id}\">",
                    indent(depth)
                )
            } else {
                format!("{}<!DOCTYPE {name}>", indent(depth))
            };
            lines.push(line);
        }
        NodeKind::Comment { content } => {
            lines.push(format!("{}<!-- {content} -->", indent(depth)));
        }
        NodeKind::Text { content } => lines.push(format!("{}\"{content}\"", indent(depth))),
        NodeKind::ProcessingInstruction { target, data } => {
            lines.push(format!("{}<?{target} {data}?>", indent(depth)));
        }
        NodeKind::Element {
            name,
            namespace,
            attributes,
        } => {
            lines.push(format!(
                "{}<{}>",
                indent(depth),
                element_tag_string(name, namespace)
            ));
            let mut attribute_lines: Vec<(String, &str)> = attributes
                .iter()
                .map(|attribute| {
                    (
                        attribute_name_string(&attribute.name, &attribute.namespace),
                        attribute.value.as_str(),
                    )
                })
                .collect();
            attribute_lines.sort_by(|a, b| a.0.cmp(&b.0));
            for (name, value) in attribute_lines {
                lines.push(format!("{}{name}=\"{value}\"", indent(depth + 1)));
            }
            for child in document.children(id) {
                dump_node(document, child, depth + 1, lines);
            }
        }
    }
}
