// Tree/node model produced by tree_builder — element/text/comment/
// processing-instruction/doctype nodes with expanded names and per-node
// source positions.
//
// Shaped as a classic arena tree (`NonZeroU32` node ids, doubly-linked
// sibling lists), structurally close enough to the tree API
// `html-conform`'s existing `src/infoset.rs::normalize()` already adapts
// (currently written against its current HTML5-parsing dependency's tree
// shape) that switching `normalize()` over to this crate should need only
// modest changes. See plan/03-tree-construction.md, "Zieldatenmodell".

use std::num::NonZeroU32;

use crate::tokenizer::{Attribute as TokenAttribute, Position};

/// Identifies a node within a [`Document`]'s arena. `NonZeroU32` so that
/// `Option<NodeId>` is the same size as `NodeId` — index 0 is never
/// issued (see [`Document::new`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct NodeId(NonZeroU32);

impl NodeId {
    fn from_index(index: usize) -> Self {
        Self(
            NonZeroU32::new(u32::try_from(index).expect("node arena index overflowed u32"))
                .expect("node arena index must be nonzero"),
        )
    }

    fn index(self) -> usize {
        self.0.get() as usize
    }
}

/// An HTML attribute, resolved to its (possibly foreign-content-adjusted)
/// namespace during tree construction — see plan/03-tree-construction.md's
/// Foreign-Content-Dispatch step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Attribute {
    pub(crate) name: String,
    pub(crate) value: String,
    pub(crate) namespace: Option<String>,
}

impl From<TokenAttribute> for Attribute {
    /// Attributes arrive off the tokenizer with no namespace at all
    /// (`namespace: None`) — HTML tag/attribute parsing itself is
    /// namespace-unaware, per §13.2.5. Namespace resolution (XHTML
    /// default, or the foreign-content adjustment tables for SVG/MathML
    /// attributes like `xlink:href`) is tree-construction's job, applied
    /// on top of this conversion, not part of it.
    fn from(attribute: TokenAttribute) -> Self {
        Attribute {
            name: attribute.name,
            value: attribute.value,
            namespace: None,
        }
    }
}

/// The kind of a document node and its associated data. Deliberately
/// covers only what the HTML5 tokenizer can actually produce a token for
/// (§13.2.5's token kinds) — no `CData`/`EntityRef` variants, since the
/// HTML5 tokenizer never emits those (character references and CDATA
/// content both resolve straight to character tokens, see
/// `tokenizer::TokenKind`'s doc comment).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NodeKind {
    /// The document node — there is exactly one per [`Document`].
    Document,
    /// An element node, e.g. `<div class="x">`.
    Element {
        name: String,
        namespace: Option<String>,
        attributes: Vec<Attribute>,
    },
    /// A text node.
    Text { content: String },
    /// A comment node.
    Comment { content: String },
    /// A processing instruction node, e.g. `<?target data?>`. Every
    /// insertion mode's token dispatch has an explicit "processing
    /// instruction token" branch (verified against the raw spec text,
    /// not assumed) that inserts one of these — `html-conform`'s
    /// `normalize()` drops it afterwards, but tree-construction still
    /// puts it in the tree, so the node kind exists here too.
    ProcessingInstruction { target: String, data: String },
    /// A DOCUMENT TYPE node, e.g. `<!DOCTYPE html>`. Also dropped by
    /// `html-conform::normalize()`, but inserted into the tree by the
    /// "initial" insertion mode per spec, same reasoning as above.
    Doctype {
        name: Option<String>,
        public_identifier: Option<String>,
        system_identifier: Option<String>,
    },
}

/// A single node in a [`Document`]'s arena: its kind/payload, its source
/// position (`None` for the document node itself and for any node a
/// tree-construction algorithm synthesizes rather than parses — e.g. an
/// implied `<html>`/`<head>`/`<body>` — `Some` for everything else), and
/// its tree-navigation links.
#[derive(Debug, Clone)]
pub(crate) struct Node {
    pub(crate) kind: NodeKind,
    // Read only by tests so far — this crate has no real consumer of
    // per-node positions yet (no `pub` API, see CLAUDE.md's Step 1
    // scope), even though tracking them accurately is this crate's
    // whole reason for existing. Not dead in the sense of "unneeded",
    // just "not read outside tests yet".
    #[allow(dead_code)]
    pub(crate) position: Option<Position>,
    parent: Option<NodeId>,
    first_child: Option<NodeId>,
    last_child: Option<NodeId>,
    next_sibling: Option<NodeId>,
    prev_sibling: Option<NodeId>,
}

impl Node {
    fn new(kind: NodeKind, position: Option<Position>) -> Self {
        Node {
            kind,
            position,
            parent: None,
            first_child: None,
            last_child: None,
            next_sibling: None,
            prev_sibling: None,
        }
    }
}

/// An HTML document tree, built by `tree_builder` (this phase) from the
/// tokenizer's (Phase 02) token stream.
///
/// This is the data model only: navigation and the bare minimum mutation
/// (`new_node`/`append_child`) needed to construct and inspect a tree.
/// The spec-level insertion algorithms ("insert a comment", "insert an
/// HTML element", table foster parenting, ...) are tree_builder's job,
/// built on top of these primitives — see plan/03-tree-construction.md's
/// "gemeinsame Tree-Construction-Infrastruktur" step.
#[derive(Debug)]
pub(crate) struct Document {
    nodes: Vec<Node>,
    root: NodeId,
}

impl Document {
    /// Creates a new document containing only its own [`Document`] node
    /// (index 1 — index 0 is an unused placeholder, so `NodeId`'s
    /// `NonZeroU32` never has to represent zero).
    pub(crate) fn new() -> Self {
        let placeholder = Node::new(NodeKind::Document, None);
        let root_node = Node::new(NodeKind::Document, None);
        Document {
            nodes: vec![placeholder, root_node],
            root: NodeId::from_index(1),
        }
    }

    pub(crate) fn root(&self) -> NodeId {
        self.root
    }

    pub(crate) fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.index()]
    }

    /// Mutable access to a node — used by `tree_builder` to append to an
    /// existing text node's content ("insert a character", §13.2.6.1)
    /// rather than always creating a new one.
    pub(crate) fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id.index()]
    }

    pub(crate) fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).parent
    }

    pub(crate) fn last_child(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).last_child
    }

    pub(crate) fn prev_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).prev_sibling
    }

    /// Creates a new, detached node and returns its id. Callers attach it
    /// into the tree via [`append_child`](Self::append_child).
    pub(crate) fn new_node(&mut self, kind: NodeKind, position: Option<Position>) -> NodeId {
        self.nodes.push(Node::new(kind, position));
        NodeId::from_index(self.nodes.len() - 1)
    }

    /// Detaches `node` from its current parent and siblings, if any — a
    /// no-op if it has none. The node itself stays in the arena (nothing
    /// is ever freed); it can be reinserted elsewhere afterward via
    /// [`insert_before`](Self::insert_before)/[`append_child`](Self::append_child).
    ///
    /// This is the DOM Standard's "remove" primitive
    /// (<https://dom.spec.whatwg.org/#concept-node-remove>), simplified
    /// to the tree-shape bookkeeping this crate tracks (no live
    /// ranges/mutation records/shadow DOM). Needed once tree-construction
    /// actually relocates already-inserted nodes — first by the adoption
    /// agency algorithm (§13.2.6.4.7) — rather than only ever inserting
    /// freshly created ones.
    pub(crate) fn remove(&mut self, node: NodeId) {
        let Some(parent) = self.node(node).parent else {
            return;
        };
        let previous_sibling = self.node(node).prev_sibling;
        let next_sibling = self.node(node).next_sibling;
        match previous_sibling {
            Some(previous_sibling) => {
                self.nodes[previous_sibling.index()].next_sibling = next_sibling;
            }
            None => self.nodes[parent.index()].first_child = next_sibling,
        }
        match next_sibling {
            Some(next_sibling) => {
                self.nodes[next_sibling.index()].prev_sibling = previous_sibling;
            }
            None => self.nodes[parent.index()].last_child = previous_sibling,
        }
        let node = &mut self.nodes[node.index()];
        node.parent = None;
        node.prev_sibling = None;
        node.next_sibling = None;
    }

    /// True if `ancestor` is `node` itself or one of its ancestors —
    /// the DOM Standard's "(inclusive) ancestor" relation
    /// (<https://dom.spec.whatwg.org/#concept-tree-inclusive-ancestor>),
    /// minus the "host-including" shadow-DOM extension (this crate has
    /// no shadow DOM). Used by the adoption agency algorithm's insertion
    /// guard (§13.2.6.4.7) to avoid creating a cycle.
    pub(crate) fn is_inclusive_ancestor(&self, ancestor: NodeId, node: NodeId) -> bool {
        let mut current = Some(node);
        while let Some(current_node) = current {
            if current_node == ancestor {
                return true;
            }
            current = self.node(current_node).parent;
        }
        false
    }

    /// Inserts `new_node` as a child of `parent`, immediately before
    /// `reference` — or, if `reference` is `None`, as the last child.
    /// If `new_node` is already attached elsewhere, it is
    /// [`remove`](Self::remove)d first — matching the DOM Standard's
    /// "insert" algorithm, whose per-node "adopt" step does the same
    /// (<https://dom.spec.whatwg.org/#concept-node-insert>: "Adopt node
    /// into parent's node document", and adopt: "If node's parent is
    /// non-null, then remove node."). `reference`, if given, must
    /// already be a child of `parent`.
    ///
    /// This is the one primitive tree-construction's insertion algorithms
    /// build on, both for the common "append as the last child of the
    /// current node" path (`reference: None`) and the less common
    /// mid-list cases (e.g. table foster parenting inserting before the
    /// table itself, or the adoption agency algorithm relocating
    /// already-inserted nodes).
    pub(crate) fn insert_before(
        &mut self,
        parent: NodeId,
        reference: Option<NodeId>,
        new_node: NodeId,
    ) {
        self.remove(new_node);
        match reference {
            None => {
                let previous_last_child = self.node(parent).last_child;
                self.nodes[new_node.index()].parent = Some(parent);
                self.nodes[new_node.index()].prev_sibling = previous_last_child;
                if let Some(previous_last_child) = previous_last_child {
                    self.nodes[previous_last_child.index()].next_sibling = Some(new_node);
                } else {
                    self.nodes[parent.index()].first_child = Some(new_node);
                }
                self.nodes[parent.index()].last_child = Some(new_node);
            }
            Some(reference) => {
                debug_assert_eq!(
                    self.node(reference).parent,
                    Some(parent),
                    "insert_before's reference node must already be a child of parent"
                );
                let previous_sibling = self.node(reference).prev_sibling;
                self.nodes[new_node.index()].parent = Some(parent);
                self.nodes[new_node.index()].next_sibling = Some(reference);
                self.nodes[new_node.index()].prev_sibling = previous_sibling;
                self.nodes[reference.index()].prev_sibling = Some(new_node);
                if let Some(previous_sibling) = previous_sibling {
                    self.nodes[previous_sibling.index()].next_sibling = Some(new_node);
                } else {
                    self.nodes[parent.index()].first_child = Some(new_node);
                }
            }
        }
    }

    /// Appends `child` as the last child of `parent`. Shorthand for
    /// [`insert_before`](Self::insert_before) with `reference: None`.
    pub(crate) fn append_child(&mut self, parent: NodeId, child: NodeId) {
        self.insert_before(parent, None, child);
    }

    /// Returns an iterator over the direct children of `id`, in document
    /// order.
    pub(crate) fn children(&self, id: NodeId) -> Children<'_> {
        Children {
            document: self,
            next: self.node(id).first_child,
        }
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

/// Iterator over a node's direct children, in document order. Created by
/// [`Document::children`].
pub(crate) struct Children<'a> {
    document: &'a Document,
    next: Option<NodeId>,
}

impl Iterator for Children<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        let current = self.next?;
        self.next = self.document.node(current).next_sibling;
        Some(current)
    }
}

#[cfg(test)]
mod tests {
    use super::{Document, NodeKind, Position};

    fn pos(line: u32, column: u32, byte_offset: usize) -> Position {
        Position {
            line,
            column,
            byte_offset,
        }
    }

    #[test]
    fn new_document_has_only_its_own_document_node() {
        let document = Document::new();
        assert_eq!(document.node(document.root()).kind, NodeKind::Document);
        assert_eq!(document.children(document.root()).count(), 0);
        assert_eq!(document.node(document.root()).position, None);
    }

    #[test]
    fn append_child_attaches_a_detached_node_as_the_last_child() {
        let mut document = Document::new();
        let root = document.root();
        let p = document.new_node(
            NodeKind::Element {
                name: "p".to_owned(),
                namespace: Some("http://www.w3.org/1999/xhtml".to_owned()),
                attributes: vec![],
            },
            Some(pos(1, 1, 0)),
        );
        document.append_child(root, p);

        let children: Vec<_> = document.children(root).collect();
        assert_eq!(children, vec![p]);
        assert_eq!(document.parent(p), Some(root));
    }

    #[test]
    fn multiple_children_are_yielded_in_document_order() {
        let mut document = Document::new();
        let root = document.root();
        let first = document.new_node(
            NodeKind::Text {
                content: "a".to_owned(),
            },
            None,
        );
        let second = document.new_node(
            NodeKind::Text {
                content: "b".to_owned(),
            },
            None,
        );
        let third = document.new_node(
            NodeKind::Text {
                content: "c".to_owned(),
            },
            None,
        );
        document.append_child(root, first);
        document.append_child(root, second);
        document.append_child(root, third);

        let children: Vec<_> = document.children(root).collect();
        assert_eq!(children, vec![first, second, third]);
    }

    #[test]
    fn insert_before_a_reference_places_the_new_node_in_the_middle() {
        let mut document = Document::new();
        let root = document.root();
        let first = document.new_node(
            NodeKind::Text {
                content: "a".to_owned(),
            },
            None,
        );
        let third = document.new_node(
            NodeKind::Text {
                content: "c".to_owned(),
            },
            None,
        );
        document.append_child(root, first);
        document.append_child(root, third);
        let second = document.new_node(
            NodeKind::Text {
                content: "b".to_owned(),
            },
            None,
        );
        document.insert_before(root, Some(third), second);

        let children: Vec<_> = document.children(root).collect();
        assert_eq!(children, vec![first, second, third]);
    }

    #[test]
    fn insert_before_at_the_start_updates_first_child() {
        let mut document = Document::new();
        let root = document.root();
        let second = document.new_node(
            NodeKind::Text {
                content: "b".to_owned(),
            },
            None,
        );
        document.append_child(root, second);
        let first = document.new_node(
            NodeKind::Text {
                content: "a".to_owned(),
            },
            None,
        );
        document.insert_before(root, Some(second), first);

        let children: Vec<_> = document.children(root).collect();
        assert_eq!(children, vec![first, second]);
    }

    #[test]
    fn nested_children_are_independent_of_their_parents_siblings() {
        let mut document = Document::new();
        let root = document.root();
        let div = document.new_node(
            NodeKind::Element {
                name: "div".to_owned(),
                namespace: Some("http://www.w3.org/1999/xhtml".to_owned()),
                attributes: vec![],
            },
            Some(pos(1, 1, 0)),
        );
        document.append_child(root, div);
        let text = document.new_node(
            NodeKind::Text {
                content: "hi".to_owned(),
            },
            Some(pos(1, 6, 5)),
        );
        document.append_child(div, text);

        assert_eq!(document.children(root).collect::<Vec<_>>(), vec![div]);
        assert_eq!(document.children(div).collect::<Vec<_>>(), vec![text]);
        assert_eq!(document.parent(text), Some(div));
    }

    #[test]
    fn synthesized_nodes_carry_no_position_while_parsed_nodes_do() {
        let mut document = Document::new();
        let root = document.root();
        let implied_html = document.new_node(
            NodeKind::Element {
                name: "html".to_owned(),
                namespace: Some("http://www.w3.org/1999/xhtml".to_owned()),
                attributes: vec![],
            },
            None,
        );
        document.append_child(root, implied_html);
        let parsed_p = document.new_node(
            NodeKind::Element {
                name: "p".to_owned(),
                namespace: Some("http://www.w3.org/1999/xhtml".to_owned()),
                attributes: vec![],
            },
            Some(pos(1, 1, 0)),
        );
        document.append_child(implied_html, parsed_p);

        assert_eq!(document.node(implied_html).position, None);
        assert_eq!(document.node(parsed_p).position, Some(pos(1, 1, 0)));
    }

    #[test]
    fn remove_detaches_a_node_and_relinks_its_siblings() {
        let mut document = Document::new();
        let root = document.root();
        let first = document.new_node(
            NodeKind::Text {
                content: "a".to_owned(),
            },
            None,
        );
        let second = document.new_node(
            NodeKind::Text {
                content: "b".to_owned(),
            },
            None,
        );
        let third = document.new_node(
            NodeKind::Text {
                content: "c".to_owned(),
            },
            None,
        );
        document.append_child(root, first);
        document.append_child(root, second);
        document.append_child(root, third);

        document.remove(second);

        assert_eq!(
            document.children(root).collect::<Vec<_>>(),
            vec![first, third]
        );
        assert_eq!(document.parent(second), None);
    }

    #[test]
    fn remove_on_a_node_with_no_parent_is_a_no_op() {
        let mut document = Document::new();
        let detached = document.new_node(
            NodeKind::Text {
                content: "a".to_owned(),
            },
            None,
        );
        document.remove(detached);
        assert_eq!(document.parent(detached), None);
    }

    #[test]
    fn insert_before_an_already_attached_node_moves_it() {
        // Matches the DOM Standard's "insert" algorithm, whose "adopt"
        // step removes a node from its old parent before placing it in
        // the new location — exercised for the first time by the
        // adoption agency algorithm (§13.2.6.4.7), which relocates
        // already-inserted nodes rather than only ever inserting fresh
        // ones.
        let mut document = Document::new();
        let root = document.root();
        let old_parent = document.new_node(
            NodeKind::Element {
                name: "div".to_owned(),
                namespace: Some("http://www.w3.org/1999/xhtml".to_owned()),
                attributes: vec![],
            },
            None,
        );
        let new_parent = document.new_node(
            NodeKind::Element {
                name: "span".to_owned(),
                namespace: Some("http://www.w3.org/1999/xhtml".to_owned()),
                attributes: vec![],
            },
            None,
        );
        document.append_child(root, old_parent);
        document.append_child(root, new_parent);
        let child = document.new_node(
            NodeKind::Text {
                content: "hi".to_owned(),
            },
            None,
        );
        document.append_child(old_parent, child);

        document.append_child(new_parent, child);

        assert_eq!(document.children(old_parent).count(), 0);
        assert_eq!(
            document.children(new_parent).collect::<Vec<_>>(),
            vec![child]
        );
        assert_eq!(document.parent(child), Some(new_parent));
    }

    #[test]
    fn is_inclusive_ancestor_covers_self_and_real_ancestors_but_not_others() {
        let mut document = Document::new();
        let root = document.root();
        let div = document.new_node(
            NodeKind::Element {
                name: "div".to_owned(),
                namespace: Some("http://www.w3.org/1999/xhtml".to_owned()),
                attributes: vec![],
            },
            None,
        );
        document.append_child(root, div);
        let span = document.new_node(
            NodeKind::Element {
                name: "span".to_owned(),
                namespace: Some("http://www.w3.org/1999/xhtml".to_owned()),
                attributes: vec![],
            },
            None,
        );
        document.append_child(div, span);
        let unrelated = document.new_node(
            NodeKind::Text {
                content: "x".to_owned(),
            },
            None,
        );
        document.append_child(root, unrelated);

        assert!(document.is_inclusive_ancestor(span, span));
        assert!(document.is_inclusive_ancestor(div, span));
        assert!(document.is_inclusive_ancestor(root, span));
        assert!(!document.is_inclusive_ancestor(unrelated, span));
        assert!(!document.is_inclusive_ancestor(span, div));
    }
}
