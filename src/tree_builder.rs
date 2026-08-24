// WHATWG HTML5 tree-construction algorithm
// (https://html.spec.whatwg.org/multipage/parsing.html#tree-construction),
// including foreign-content (SVG/MathML) dispatch and attribute/tag-name
// adjustment.
//
// All insertion modes (§13.2.6.4.1-.20, minus the frameset-related ones —
// see plan/03-tree-construction.md's scope decision) and the foreign-
// content dispatcher (§13.2.6.5) are implemented; `TreeBuilder::process_token`
// is the real entry point, driven end to end by `lib.rs::parse()`.

use crate::document::{Attribute, Document, NodeId, NodeKind};
use crate::tokenizer::{DoctypeToken, ExternalState, Position, TagToken, TokenKind};

/// The HTML namespace URI (https://infra.spec.whatwg.org/#html-namespace).
pub(crate) const HTML_NAMESPACE: &str = "http://www.w3.org/1999/xhtml";
/// The SVG namespace URI.
pub(crate) const SVG_NAMESPACE: &str = "http://www.w3.org/2000/svg";
/// The MathML namespace URI.
pub(crate) const MATHML_NAMESPACE: &str = "http://www.w3.org/1998/Math/MathML";
/// The XLink namespace URI (https://infra.spec.whatwg.org/#xlink-namespace).
const XLINK_NAMESPACE: &str = "http://www.w3.org/1999/xlink";
/// The XML namespace URI.
const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";
/// The XMLNS namespace URI.
const XMLNS_NAMESPACE: &str = "http://www.w3.org/2000/xmlns/";

/// "Adjust foreign attributes" (§13.2.6.1): attribute names that get
/// reassigned to a namespaced identity (a prefix, local name, and
/// namespace) rather than staying a single unqualified name — mostly
/// XLink attributes on SVG elements. This crate's `document::Attribute`
/// has no separate prefix/local-name split, so this table maps the full
/// original name straight to the namespace URI, keeping `name` as-is
/// (e.g. `"xlink:href"` stays `"xlink:href"`, gains
/// `namespace: Some(XLINK_NAMESPACE)`).
const FOREIGN_ATTRIBUTE_NAMESPACES: &[(&str, &str)] = &[
    ("xlink:actuate", XLINK_NAMESPACE),
    ("xlink:arcrole", XLINK_NAMESPACE),
    ("xlink:href", XLINK_NAMESPACE),
    ("xlink:role", XLINK_NAMESPACE),
    ("xlink:show", XLINK_NAMESPACE),
    ("xlink:title", XLINK_NAMESPACE),
    ("xlink:type", XLINK_NAMESPACE),
    ("xml:lang", XML_NAMESPACE),
    ("xml:space", XML_NAMESPACE),
    ("xmlns", XMLNS_NAMESPACE),
    ("xmlns:xlink", XMLNS_NAMESPACE),
];

/// "Adjust SVG attributes" (§13.2.6.1): attribute names that are
/// case-fixed when found on an SVG-namespace element's token (token
/// attribute names always arrive all-lowercase from the tokenizer,
/// which has no namespace awareness).
const SVG_ATTRIBUTE_ADJUSTMENTS: &[(&str, &str)] = &[
    ("attributename", "attributeName"),
    ("attributetype", "attributeType"),
    ("basefrequency", "baseFrequency"),
    ("baseprofile", "baseProfile"),
    ("calcmode", "calcMode"),
    ("clippathunits", "clipPathUnits"),
    ("diffuseconstant", "diffuseConstant"),
    ("edgemode", "edgeMode"),
    ("filterunits", "filterUnits"),
    ("glyphref", "glyphRef"),
    ("gradienttransform", "gradientTransform"),
    ("gradientunits", "gradientUnits"),
    ("kernelmatrix", "kernelMatrix"),
    ("kernelunitlength", "kernelUnitLength"),
    ("keypoints", "keyPoints"),
    ("keysplines", "keySplines"),
    ("keytimes", "keyTimes"),
    ("lengthadjust", "lengthAdjust"),
    ("limitingconeangle", "limitingConeAngle"),
    ("markerheight", "markerHeight"),
    ("markerunits", "markerUnits"),
    ("markerwidth", "markerWidth"),
    ("maskcontentunits", "maskContentUnits"),
    ("maskunits", "maskUnits"),
    ("numoctaves", "numOctaves"),
    ("pathlength", "pathLength"),
    ("patterncontentunits", "patternContentUnits"),
    ("patterntransform", "patternTransform"),
    ("patternunits", "patternUnits"),
    ("pointsatx", "pointsAtX"),
    ("pointsaty", "pointsAtY"),
    ("pointsatz", "pointsAtZ"),
    ("preservealpha", "preserveAlpha"),
    ("preserveaspectratio", "preserveAspectRatio"),
    ("primitiveunits", "primitiveUnits"),
    ("refx", "refX"),
    ("refy", "refY"),
    ("repeatcount", "repeatCount"),
    ("repeatdur", "repeatDur"),
    ("requiredextensions", "requiredExtensions"),
    ("requiredfeatures", "requiredFeatures"),
    ("specularconstant", "specularConstant"),
    ("specularexponent", "specularExponent"),
    ("spreadmethod", "spreadMethod"),
    ("startoffset", "startOffset"),
    ("stddeviation", "stdDeviation"),
    ("stitchtiles", "stitchTiles"),
    ("surfacescale", "surfaceScale"),
    ("systemlanguage", "systemLanguage"),
    ("tablevalues", "tableValues"),
    ("targetx", "targetX"),
    ("targety", "targetY"),
    ("textlength", "textLength"),
    ("viewbox", "viewBox"),
    ("viewtarget", "viewTarget"),
    ("xchannelselector", "xChannelSelector"),
    ("ychannelselector", "yChannelSelector"),
    ("zoomandpan", "zoomAndPan"),
];

/// "Adjust MathML attributes" (§13.2.6.1) — trivial compared to SVG's:
/// only `definitionurl` is case-fixed.
const MATHML_ATTRIBUTE_ADJUSTMENTS: &[(&str, &str)] = &[("definitionurl", "definitionURL")];

/// Applies "adjust MathML attributes"/"adjust SVG attributes"/"adjust
/// foreign attributes" (§13.2.6.1) to `tag`'s attributes for an element
/// being inserted into `namespace` (MathML or SVG — never called for
/// HTML-namespace insertions, see
/// [`create_element_for_token`](TreeBuilder::create_element_for_token)).
/// The MathML/SVG name-casing fixups apply only when `namespace`
/// matches; the foreign-attribute namespacing always applies,
/// regardless of which foreign namespace this is.
fn adjust_attributes_for_foreign_element(tag: &TagToken, namespace: &str) -> Vec<Attribute> {
    let name_adjustments: &[(&str, &str)] = if namespace == MATHML_NAMESPACE {
        MATHML_ATTRIBUTE_ADJUSTMENTS
    } else if namespace == SVG_NAMESPACE {
        SVG_ATTRIBUTE_ADJUSTMENTS
    } else {
        &[]
    };
    tag.attributes
        .iter()
        .map(|attribute| {
            let name = name_adjustments
                .iter()
                .find(|&&(from, _)| from == attribute.name)
                .map_or_else(|| attribute.name.clone(), |&(_, to)| to.to_owned());
            let namespace = FOREIGN_ATTRIBUTE_NAMESPACES
                .iter()
                .find(|&&(from, _)| from == name)
                .map(|&(_, ns)| ns.to_owned());
            Attribute {
                name,
                value: attribute.value.clone(),
                namespace,
            }
        })
        .collect()
}

/// "Adjust SVG tag names" — the table foreign content's "any other
/// start tag" rule (§13.2.6.5) uses to case-fix an SVG element's tag
/// name (token tag names always arrive all-lowercase from the
/// tokenizer). Returns `name` unchanged if it isn't in the table.
const SVG_TAG_NAME_ADJUSTMENTS: &[(&str, &str)] = &[
    ("altglyph", "altGlyph"),
    ("altglyphdef", "altGlyphDef"),
    ("altglyphitem", "altGlyphItem"),
    ("animatecolor", "animateColor"),
    ("animatemotion", "animateMotion"),
    ("animatetransform", "animateTransform"),
    ("clippath", "clipPath"),
    ("feblend", "feBlend"),
    ("fecolormatrix", "feColorMatrix"),
    ("fecomponenttransfer", "feComponentTransfer"),
    ("fecomposite", "feComposite"),
    ("feconvolvematrix", "feConvolveMatrix"),
    ("fediffuselighting", "feDiffuseLighting"),
    ("fedisplacementmap", "feDisplacementMap"),
    ("fedistantlight", "feDistantLight"),
    ("fedropshadow", "feDropShadow"),
    ("feflood", "feFlood"),
    ("fefunca", "feFuncA"),
    ("fefuncb", "feFuncB"),
    ("fefuncg", "feFuncG"),
    ("fefuncr", "feFuncR"),
    ("fegaussianblur", "feGaussianBlur"),
    ("feimage", "feImage"),
    ("femerge", "feMerge"),
    ("femergenode", "feMergeNode"),
    ("femorphology", "feMorphology"),
    ("feoffset", "feOffset"),
    ("fepointlight", "fePointLight"),
    ("fespecularlighting", "feSpecularLighting"),
    ("fespotlight", "feSpotLight"),
    ("fetile", "feTile"),
    ("feturbulence", "feTurbulence"),
    ("foreignobject", "foreignObject"),
    ("glyphref", "glyphRef"),
    ("lineargradient", "linearGradient"),
    ("radialgradient", "radialGradient"),
    ("textpath", "textPath"),
];

fn adjust_svg_tag_name(name: &str) -> &str {
    SVG_TAG_NAME_ADJUSTMENTS
        .iter()
        .find(|&&(from, _)| from == name)
        .map_or(name, |&(_, to)| to)
}

/// A node is a "MathML text integration point" (§13.2.6.5) if it's one
/// of these five MathML elements.
fn is_mathml_text_integration_point(document: &Document, node: NodeId) -> bool {
    let NodeKind::Element {
        name, namespace, ..
    } = &document.node(node).kind
    else {
        return false;
    };
    namespace.as_deref() == Some(MATHML_NAMESPACE)
        && matches!(name.as_str(), "mi" | "mo" | "mn" | "ms" | "mtext")
}

/// A node is an "HTML integration point" (§13.2.6.5) if it's an SVG
/// `foreignObject`/`desc`/`title`, or a MathML `annotation-xml` whose
/// start tag had an `encoding` attribute (ASCII case-insensitive)
/// matching `"text/html"` or `"application/xhtml+xml"`. Checked against
/// the element's current attributes rather than a separately stored
/// flag — this crate never mutates attributes after element creation
/// (no script execution), so they're equivalent.
fn is_html_integration_point(document: &Document, node: NodeId) -> bool {
    let NodeKind::Element {
        name,
        namespace,
        attributes,
    } = &document.node(node).kind
    else {
        return false;
    };
    match namespace.as_deref() {
        Some(MATHML_NAMESPACE) if name == "annotation-xml" => attributes.iter().any(|attribute| {
            attribute.name == "encoding"
                && (attribute.value.eq_ignore_ascii_case("text/html")
                    || attribute
                        .value
                        .eq_ignore_ascii_case("application/xhtml+xml"))
        }),
        Some(SVG_NAMESPACE) => matches!(name.as_str(), "foreignObject" | "desc" | "title"),
        _ => false,
    }
}

/// A (namespace, local name) pair, used to describe the scope-check
/// algorithms' target element-type lists (§13.2.4.2) as plain data,
/// without needing a live node to compare against.
#[derive(Debug, Clone, Copy)]
struct ElementType {
    namespace: &'static str,
    name: &'static str,
}

fn element_type_matches(list: &[ElementType], namespace: &str, name: &str) -> bool {
    list.iter()
        .any(|element_type| element_type.namespace == namespace && element_type.name == name)
}

/// The element-type list for "has a particular element in scope"
/// (§13.2.4.2, the default/unqualified scope) — also the common base list
/// every other named scope (list item, button) extends.
const DEFAULT_SCOPE: &[ElementType] = &[
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "applet",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "caption",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "html",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "table",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "td",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "th",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "marquee",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "object",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "select",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "template",
    },
    ElementType {
        namespace: MATHML_NAMESPACE,
        name: "mi",
    },
    ElementType {
        namespace: MATHML_NAMESPACE,
        name: "mo",
    },
    ElementType {
        namespace: MATHML_NAMESPACE,
        name: "mn",
    },
    ElementType {
        namespace: MATHML_NAMESPACE,
        name: "ms",
    },
    ElementType {
        namespace: MATHML_NAMESPACE,
        name: "mtext",
    },
    ElementType {
        namespace: MATHML_NAMESPACE,
        name: "annotation-xml",
    },
    ElementType {
        namespace: SVG_NAMESPACE,
        name: "foreignObject",
    },
    ElementType {
        namespace: SVG_NAMESPACE,
        name: "desc",
    },
    ElementType {
        namespace: SVG_NAMESPACE,
        name: "title",
    },
];

/// Extra element types "has a particular element in list item scope"
/// adds on top of [`DEFAULT_SCOPE`].
const LIST_ITEM_SCOPE_EXTRA: &[ElementType] = &[
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "ol",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "ul",
    },
];

/// Extra element types "has a particular element in button scope" adds on
/// top of [`DEFAULT_SCOPE`].
const BUTTON_SCOPE_EXTRA: &[ElementType] = &[ElementType {
    namespace: HTML_NAMESPACE,
    name: "button",
}];

/// The element-type list for "has a particular element in table scope" —
/// deliberately *not* an extension of [`DEFAULT_SCOPE`], per spec (a much
/// shorter, unrelated list).
const TABLE_SCOPE: &[ElementType] = &[
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "html",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "table",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "template",
    },
];

/// The "special" category (§13.2.4.2) — elements with varying levels of
/// special parsing rules. Used by the adoption agency algorithm
/// (§13.2.6.4.7) to find the "furthest block" between a misnested
/// formatting element and the current node. Transcribed directly from
/// the spec's element list (83 HTML elements, 6 MathML, 3 SVG) —
/// extracted from the raw spec markup, not assumed. Note `title`
/// legitimately appears twice: once for the HTML element, once more for
/// the (distinctly namespaced) SVG element of the same name.
const SPECIAL_CATEGORY: &[ElementType] = &[
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "address",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "applet",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "area",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "article",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "aside",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "base",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "basefont",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "bgsound",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "blockquote",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "body",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "br",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "button",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "caption",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "center",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "col",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "colgroup",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "dd",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "details",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "dir",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "div",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "dl",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "dt",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "embed",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "fieldset",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "figcaption",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "figure",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "footer",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "form",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "frame",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "frameset",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "h1",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "h2",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "h3",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "h4",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "h5",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "h6",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "head",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "header",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "hgroup",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "hr",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "html",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "iframe",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "img",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "input",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "keygen",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "li",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "link",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "listing",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "main",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "marquee",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "menu",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "meta",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "nav",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "noembed",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "noframes",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "noscript",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "object",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "ol",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "p",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "param",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "plaintext",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "pre",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "script",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "search",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "section",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "select",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "source",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "style",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "summary",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "table",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "tbody",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "td",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "template",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "textarea",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "tfoot",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "th",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "thead",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "title",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "tr",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "track",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "ul",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "wbr",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "xmp",
    },
    ElementType {
        namespace: MATHML_NAMESPACE,
        name: "mi",
    },
    ElementType {
        namespace: MATHML_NAMESPACE,
        name: "mo",
    },
    ElementType {
        namespace: MATHML_NAMESPACE,
        name: "mn",
    },
    ElementType {
        namespace: MATHML_NAMESPACE,
        name: "ms",
    },
    ElementType {
        namespace: MATHML_NAMESPACE,
        name: "mtext",
    },
    ElementType {
        namespace: MATHML_NAMESPACE,
        name: "annotation-xml",
    },
    ElementType {
        namespace: SVG_NAMESPACE,
        name: "foreignObject",
    },
    ElementType {
        namespace: SVG_NAMESPACE,
        name: "desc",
    },
    ElementType {
        namespace: SVG_NAMESPACE,
        name: "title",
    },
];

fn is_special(document: &Document, node: NodeId) -> bool {
    let NodeKind::Element {
        name, namespace, ..
    } = &document.node(node).kind
    else {
        return false;
    };
    element_type_matches(SPECIAL_CATEGORY, namespace.as_deref().unwrap_or(""), name)
}

/// The element-type list "generate implied end tags" pops through
/// (§13.2.6.3).
const IMPLIED_END_TAGS: &[ElementType] = &[
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "dd",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "dt",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "li",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "optgroup",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "option",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "p",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "rb",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "rp",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "rt",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "rtc",
    },
];

/// The element-type list "generate all implied end tags thoroughly" pops
/// through (§13.2.6.3) — a superset of [`IMPLIED_END_TAGS`].
const IMPLIED_END_TAGS_THOROUGHLY: &[ElementType] = &[
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "caption",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "colgroup",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "dd",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "dt",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "li",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "optgroup",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "option",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "p",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "rb",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "rp",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "rt",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "rtc",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "tbody",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "td",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "tfoot",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "th",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "thead",
    },
    ElementType {
        namespace: HTML_NAMESPACE,
        name: "tr",
    },
];

/// The stack of open elements (§13.2.4.2). Grows downwards in spec terms —
/// modeled here as a `Vec` where the topmost node (the `html` element) is
/// index 0 and the current node (bottommost) is the last entry, so
/// "push"/"pop" are plain `Vec` operations.
///
/// Note there is no "select scope" here: despite `<select>`/`<option>`
/// needing special parsing behavior, the spec has no dedicated "in
/// select" insertion mode or "select scope" algorithm (verified against
/// the raw spec text, not assumed) — that handling lives inline in the
/// "in body" insertion mode instead.
#[derive(Debug, Default)]
pub(crate) struct OpenElementsStack {
    entries: Vec<NodeId>,
}

impl OpenElementsStack {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn push(&mut self, node: NodeId) {
        self.entries.push(node);
    }

    pub(crate) fn pop(&mut self) -> Option<NodeId> {
        self.entries.pop()
    }

    /// The current node (§13.2.4.2): the bottommost node on the stack.
    pub(crate) fn current_node(&self) -> Option<NodeId> {
        self.entries.last().copied()
    }

    /// True if `node` is currently on the stack — used by "reconstruct
    /// the active formatting elements" (§13.2.4.3) to check whether a
    /// formatting entry's element is still open.
    pub(crate) fn contains(&self, node: NodeId) -> bool {
        self.entries.contains(&node)
    }

    /// §13.2.4.2's generic "have an element *target node* in a specific
    /// scope consisting of a list of element types *list*" algorithm.
    /// `target_namespace`/`target_name` identify the element **by tag
    /// name**, not by a specific node instance — matching how every named
    /// scope check is actually invoked in the spec (e.g. "the stack of
    /// open elements has a `p` element in button scope"). `extra_boundary`
    /// lets [`has_element_in_list_item_scope`](Self::has_element_in_list_item_scope)/
    /// [`has_element_in_button_scope`](Self::has_element_in_button_scope) extend
    /// [`DEFAULT_SCOPE`] without duplicating it; pass `&[]` for scopes
    /// that don't extend it.
    fn has_element_in_specific_scope(
        &self,
        document: &Document,
        target_namespace: &str,
        target_name: &str,
        boundary: &[ElementType],
        extra_boundary: &[ElementType],
    ) -> bool {
        for &node in self.entries.iter().rev() {
            let NodeKind::Element {
                name, namespace, ..
            } = &document.node(node).kind
            else {
                continue;
            };
            let namespace = namespace.as_deref().unwrap_or("");
            if namespace == target_namespace && name == target_name {
                return true;
            }
            if element_type_matches(boundary, namespace, name)
                || element_type_matches(extra_boundary, namespace, name)
            {
                return false;
            }
        }
        // Unreachable for a well-formed stack: the `html` element is
        // always index 0, and `html` is itself always in every scope's
        // boundary list, so the loop always terminates via one of the
        // branches above before running out of entries.
        false
    }

    /// "Have a particular element in scope" (default scope, §13.2.4.2).
    pub(crate) fn has_element_in_scope(&self, document: &Document, name: &str) -> bool {
        self.has_element_in_specific_scope(document, HTML_NAMESPACE, name, DEFAULT_SCOPE, &[])
    }

    /// "Have a particular element in list item scope" (§13.2.4.2).
    pub(crate) fn has_element_in_list_item_scope(&self, document: &Document, name: &str) -> bool {
        self.has_element_in_specific_scope(
            document,
            HTML_NAMESPACE,
            name,
            DEFAULT_SCOPE,
            LIST_ITEM_SCOPE_EXTRA,
        )
    }

    /// "Have a particular element in button scope" (§13.2.4.2).
    pub(crate) fn has_element_in_button_scope(&self, document: &Document, name: &str) -> bool {
        self.has_element_in_specific_scope(
            document,
            HTML_NAMESPACE,
            name,
            DEFAULT_SCOPE,
            BUTTON_SCOPE_EXTRA,
        )
    }

    /// "Have a particular element in table scope" (§13.2.4.2).
    pub(crate) fn has_element_in_table_scope(&self, document: &Document, name: &str) -> bool {
        self.has_element_in_specific_scope(document, HTML_NAMESPACE, name, TABLE_SCOPE, &[])
    }

    /// The topmost node on the stack — the `html` element, once one has
    /// been pushed. Needed by foster parenting's "no last table" fallback
    /// (§13.2.6.1).
    fn topmost(&self) -> Option<NodeId> {
        self.entries.first().copied()
    }

    /// The last (most recently pushed, i.e. bottommost) element on the
    /// stack matching `namespace`/`name`, if any — "the last `table`
    /// element in the stack of open elements" and similar phrasing used
    /// throughout §13.2.6.1.
    fn last_matching(&self, document: &Document, namespace: &str, name: &str) -> Option<NodeId> {
        self.entries.iter().rev().copied().find(|&node| {
            let NodeKind::Element {
                name: node_name,
                namespace: node_namespace,
                ..
            } = &document.node(node).kind
            else {
                return false;
            };
            node_namespace.as_deref() == Some(namespace) && node_name == name
        })
    }

    /// True if `a` is lower (more recently added, i.e. further from the
    /// top) than `b` in the stack. Both must currently be on the stack.
    fn is_lower(&self, a: NodeId, b: NodeId) -> bool {
        let position_of = |target: NodeId| {
            self.entries
                .iter()
                .position(|&node| node == target)
                .expect("is_lower called with a node that isn't on the stack")
        };
        position_of(a) > position_of(b)
    }

    /// The element immediately above `node` on the stack (i.e. pushed
    /// immediately before it) — §13.2.6.1's foster-parenting fallback for
    /// a table with no parent (unreachable in this single-pass parser,
    /// since nothing ever removes a node from its parent once inserted,
    /// but implemented for spec fidelity regardless).
    fn element_immediately_above(&self, node: NodeId) -> Option<NodeId> {
        let position = self.entries.iter().position(|&entry| entry == node)?;
        position.checked_sub(1).map(|above| self.entries[above])
    }

    /// The topmost node lower in the stack than `formatting_element` that
    /// is an element in the [`SPECIAL_CATEGORY`] (§13.2.4.2) — the
    /// adoption agency algorithm's "furthest block" (§13.2.6.4.7, step
    /// 7). `None` if there is no such node.
    fn topmost_special_element_below(
        &self,
        document: &Document,
        formatting_element: NodeId,
    ) -> Option<NodeId> {
        let position = self
            .entries
            .iter()
            .position(|&entry| entry == formatting_element)?;
        self.entries[position + 1..]
            .iter()
            .copied()
            .find(|&node| is_special(document, node))
    }

    /// "Generate implied end tags" (§13.2.6.3): pop elements off the stack
    /// while the current node is a `dd`, `dt`, `li`, `optgroup`, `option`,
    /// `p`, `rb`, `rp`, `rt`, or `rtc` element. If `exclude` is given, that
    /// element name is treated as if it were not in the list (so popping
    /// stops there instead of past it), per the spec's "lists an element
    /// to exclude from the process" clause.
    pub(crate) fn generate_implied_end_tags(&mut self, document: &Document, exclude: Option<&str>) {
        self.generate_implied_end_tags_impl(document, IMPLIED_END_TAGS, exclude);
    }

    /// "Generate all implied end tags thoroughly" (§13.2.6.3) — the same
    /// idea over the larger [`IMPLIED_END_TAGS_THOROUGHLY`] list. The spec
    /// never invokes this variant with an exclusion, so there's no
    /// `exclude` parameter here.
    pub(crate) fn generate_implied_end_tags_thoroughly(&mut self, document: &Document) {
        self.generate_implied_end_tags_impl(document, IMPLIED_END_TAGS_THOROUGHLY, None);
    }

    fn generate_implied_end_tags_impl(
        &mut self,
        document: &Document,
        list: &[ElementType],
        exclude: Option<&str>,
    ) {
        while let Some(node) = self.current_node() {
            let NodeKind::Element {
                name, namespace, ..
            } = &document.node(node).kind
            else {
                break;
            };
            let namespace = namespace.as_deref().unwrap_or("");
            if Some(name.as_str()) == exclude {
                break;
            }
            if !element_type_matches(list, namespace, name) {
                break;
            }
            self.pop();
        }
    }
}

/// An entry in the [`ActiveFormattingElements`] list (§13.2.4.3): either a
/// marker, or a formatting element. Stores only the `NodeId`, not a
/// separate copy of the creating token — the token needed to recreate an
/// equivalent element during "reconstruct the active formatting
/// elements" is re-derived from the node's current name/attributes
/// (immutable once parsed), avoiding duplicated data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FormattingEntry {
    Marker,
    Element(NodeId),
}

/// The list of active formatting elements (§13.2.4.3): used to handle
/// mis-nested formatting element tags (`a`/`b`/`i`/... — the adoption
/// agency algorithm, not yet implemented) by "reopening" formatting
/// elements that got implicitly closed.
#[derive(Debug, Default)]
pub(crate) struct ActiveFormattingElements {
    entries: Vec<FormattingEntry>,
}

impl ActiveFormattingElements {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Markers are inserted when entering `applet`/`object`/`marquee`/
    /// `template`/`td`/`th`/`caption` elements, to stop formatting from
    /// "leaking" into them — not called by anything yet (none of those
    /// elements' insertion-mode handling is implemented), but part of
    /// the list's real interface.
    pub(crate) fn push_marker(&mut self) {
        self.entries.push(FormattingEntry::Marker);
    }

    /// "Push onto the list of active formatting elements" (§13.2.4.3),
    /// including the Noah's Ark clause: if three elements with the same
    /// tag name, namespace, and attributes already exist after the last
    /// marker (or anywhere, if there is none), the earliest is removed
    /// first.
    pub(crate) fn push(&mut self, document: &Document, element: NodeId) {
        let boundary = self
            .entries
            .iter()
            .rposition(|entry| matches!(entry, FormattingEntry::Marker))
            .map_or(0, |marker_index| marker_index + 1);
        let matching: Vec<usize> = self.entries[boundary..]
            .iter()
            .enumerate()
            .filter_map(|(offset, entry)| match entry {
                FormattingEntry::Element(node)
                    if elements_match_for_noahs_ark(document, *node, element) =>
                {
                    Some(boundary + offset)
                }
                _ => None,
            })
            .collect();
        if matching.len() >= 3 {
            self.entries.remove(matching[0]);
        }
        self.entries.push(FormattingEntry::Element(element));
    }

    /// "Clear the list of active formatting elements up to the last
    /// marker" (§13.2.4.3) — not called by anything yet (nothing that
    /// would trigger it, e.g. table-related insertion modes, is
    /// implemented), but part of the list's real interface.
    pub(crate) fn clear_up_to_last_marker(&mut self) {
        while let Some(entry) = self.entries.pop() {
            if matches!(entry, FormattingEntry::Marker) {
                break;
            }
        }
    }
}

/// §13.2.4.3's "two elements have the same attributes if all their parsed
/// attributes can be paired such that the two attributes in each pair
/// have identical names, namespaces, and values (the order of the
/// attributes does not matter)" — plus the tag name/namespace match the
/// same clause requires.
fn elements_match_for_noahs_ark(document: &Document, a: NodeId, b: NodeId) -> bool {
    let NodeKind::Element {
        name: name_a,
        namespace: namespace_a,
        attributes: attributes_a,
    } = &document.node(a).kind
    else {
        return false;
    };
    let NodeKind::Element {
        name: name_b,
        namespace: namespace_b,
        attributes: attributes_b,
    } = &document.node(b).kind
    else {
        return false;
    };
    if name_a != name_b || namespace_a != namespace_b || attributes_a.len() != attributes_b.len() {
        return false;
    }
    let mut sorted_a: Vec<_> = attributes_a
        .iter()
        .map(|attribute| (&attribute.name, &attribute.namespace, &attribute.value))
        .collect();
    let mut sorted_b: Vec<_> = attributes_b
        .iter()
        .map(|attribute| (&attribute.name, &attribute.namespace, &attribute.value))
        .collect();
    sorted_a.sort();
    sorted_b.sort();
    sorted_a == sorted_b
}

/// A document's quirks-mode flag, determined once from its `DOCTYPE`
/// token (§13.2.6.4.1, the "initial" insertion mode). Not just inert
/// metadata: e.g. "in body"'s `<table>` start-tag rule (§13.2.6.4.7) only
/// auto-closes an open `p` element when the document is *not* in quirks
/// mode, so this genuinely shapes the produced tree — that's why it's
/// implemented here despite `html-conform`'s `normalize()` dropping the
/// `DocumentType` node itself from its output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum QuirksMode {
    #[default]
    NoQuirks,
    Quirks,
    LimitedQuirks,
}

/// Public identifiers that set the document to quirks mode when matched
/// *exactly* (§13.2.6.4.1).
const QUIRKS_PUBLIC_IDENTIFIER_EXACT: &[&str] = &[
    "-//W3O//DTD W3 HTML Strict 3.0//EN//",
    "-/W3C/DTD HTML 4.0 Transitional/EN",
    "HTML",
];

/// The system identifier that sets the document to quirks mode when
/// matched exactly (§13.2.6.4.1).
const QUIRKS_SYSTEM_IDENTIFIER_EXACT: &str =
    "http://www.ibm.com/data/dtd/v11/ibmxhtml1-transitional.dtd";

/// Public identifier prefixes that set the document to quirks mode
/// (§13.2.6.4.1), regardless of the system identifier.
const QUIRKS_PUBLIC_IDENTIFIER_PREFIXES: &[&str] = &[
    "+//Silmaril//dtd html Pro v0r11 19970101//",
    "-//AS//DTD HTML 3.0 asWedit + extensions//",
    "-//AdvaSoft Ltd//DTD HTML 3.0 asWedit + extensions//",
    "-//IETF//DTD HTML 2.0 Level 1//",
    "-//IETF//DTD HTML 2.0 Level 2//",
    "-//IETF//DTD HTML 2.0 Strict Level 1//",
    "-//IETF//DTD HTML 2.0 Strict Level 2//",
    "-//IETF//DTD HTML 2.0 Strict//",
    "-//IETF//DTD HTML 2.0//",
    "-//IETF//DTD HTML 2.1E//",
    "-//IETF//DTD HTML 3.0//",
    "-//IETF//DTD HTML 3.2 Final//",
    "-//IETF//DTD HTML 3.2//",
    "-//IETF//DTD HTML 3//",
    "-//IETF//DTD HTML Level 0//",
    "-//IETF//DTD HTML Level 1//",
    "-//IETF//DTD HTML Level 2//",
    "-//IETF//DTD HTML Level 3//",
    "-//IETF//DTD HTML Strict Level 0//",
    "-//IETF//DTD HTML Strict Level 1//",
    "-//IETF//DTD HTML Strict Level 2//",
    "-//IETF//DTD HTML Strict Level 3//",
    "-//IETF//DTD HTML Strict//",
    "-//IETF//DTD HTML//",
    "-//Metrius//DTD Metrius Presentational//",
    "-//Microsoft//DTD Internet Explorer 2.0 HTML Strict//",
    "-//Microsoft//DTD Internet Explorer 2.0 HTML//",
    "-//Microsoft//DTD Internet Explorer 2.0 Tables//",
    "-//Microsoft//DTD Internet Explorer 3.0 HTML Strict//",
    "-//Microsoft//DTD Internet Explorer 3.0 HTML//",
    "-//Microsoft//DTD Internet Explorer 3.0 Tables//",
    "-//Netscape Comm. Corp.//DTD HTML//",
    "-//Netscape Comm. Corp.//DTD Strict HTML//",
    "-//O'Reilly and Associates//DTD HTML 2.0//",
    "-//O'Reilly and Associates//DTD HTML Extended 1.0//",
    "-//O'Reilly and Associates//DTD HTML Extended Relaxed 1.0//",
    "-//SQ//DTD HTML 2.0 HoTMetaL + extensions//",
    "-//SoftQuad Software//DTD HoTMetaL PRO 6.0::19990601::extensions to HTML 4.0//",
    "-//SoftQuad//DTD HoTMetaL PRO 4.0::19971010::extensions to HTML 4.0//",
    "-//Spyglass//DTD HTML 2.0 Extended//",
    "-//Sun Microsystems Corp.//DTD HotJava HTML//",
    "-//Sun Microsystems Corp.//DTD HotJava Strict HTML//",
    "-//W3C//DTD HTML 3 1995-03-24//",
    "-//W3C//DTD HTML 3.2 Draft//",
    "-//W3C//DTD HTML 3.2 Final//",
    "-//W3C//DTD HTML 3.2//",
    "-//W3C//DTD HTML 3.2S Draft//",
    "-//W3C//DTD HTML 4.0 Frameset//",
    "-//W3C//DTD HTML 4.0 Transitional//",
    "-//W3C//DTD HTML Experimental 19960712//",
    "-//W3C//DTD HTML Experimental 970421//",
    "-//W3C//DTD W3 HTML//",
    "-//W3O//DTD W3 HTML 3.0//",
    "-//WebTechs//DTD Mozilla HTML 2.0//",
    "-//WebTechs//DTD Mozilla HTML//",
];

/// Public identifier prefixes that set the document to quirks mode only
/// when the system identifier is missing or the empty string
/// (§13.2.6.4.1) — the same two prefixes instead set *limited*-quirks
/// mode when a system identifier is present, see
/// [`LIMITED_QUIRKS_PUBLIC_IDENTIFIER_PREFIXES_WITH_SYSTEM_IDENTIFIER`].
const QUIRKS_PUBLIC_IDENTIFIER_PREFIXES_WITHOUT_SYSTEM_IDENTIFIER: &[&str] = &[
    "-//W3C//DTD HTML 4.01 Frameset//",
    "-//W3C//DTD HTML 4.01 Transitional//",
];

/// Public identifier prefixes that set the document to limited-quirks
/// mode unconditionally (§13.2.6.4.1).
const LIMITED_QUIRKS_PUBLIC_IDENTIFIER_PREFIXES: &[&str] = &[
    "-//W3C//DTD XHTML 1.0 Frameset//",
    "-//W3C//DTD XHTML 1.0 Transitional//",
];

/// Public identifier prefixes that set the document to limited-quirks
/// mode when the system identifier is present (neither missing nor the
/// empty string) — see
/// [`QUIRKS_PUBLIC_IDENTIFIER_PREFIXES_WITHOUT_SYSTEM_IDENTIFIER`] for
/// the same two prefixes' quirks-mode counterpart.
const LIMITED_QUIRKS_PUBLIC_IDENTIFIER_PREFIXES_WITH_SYSTEM_IDENTIFIER: &[&str] = &[
    "-//W3C//DTD HTML 4.01 Frameset//",
    "-//W3C//DTD HTML 4.01 Transitional//",
];

fn starts_with_ignore_ascii_case(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

/// "Determine the quirks mode" from a `DOCTYPE` token, per the condition
/// checks §13.2.6.4.1's "initial" insertion mode runs before switching to
/// "before html". Comparisons are ASCII case-insensitive per spec; a
/// system identifier of `Some("")` counts as present (not missing) — only
/// `None` counts as missing.
///
/// Deliberately doesn't model the spec's "iframe `srcdoc` document" or
/// "parser cannot change the mode flag" exceptions: this crate parses
/// whole documents only, never `srcdoc` content or HTML fragments (see
/// plan/03-tree-construction.md's scope decision), so both are always in
/// their default/false state here.
pub(crate) fn determine_quirks_mode(doctype: &DoctypeToken) -> QuirksMode {
    let name = doctype.name.as_deref();
    let public_id = doctype.public_identifier.as_deref();
    let system_id = doctype.system_identifier.as_deref();
    let system_id_missing_or_empty = system_id.is_none_or(str::is_empty);

    let quirks = doctype.force_quirks
        || name != Some("html")
        || public_id.is_some_and(|id| {
            QUIRKS_PUBLIC_IDENTIFIER_EXACT
                .iter()
                .any(|exact| id.eq_ignore_ascii_case(exact))
        })
        || system_id.is_some_and(|id| id.eq_ignore_ascii_case(QUIRKS_SYSTEM_IDENTIFIER_EXACT))
        || public_id.is_some_and(|id| {
            QUIRKS_PUBLIC_IDENTIFIER_PREFIXES
                .iter()
                .any(|prefix| starts_with_ignore_ascii_case(id, prefix))
        })
        || (system_id_missing_or_empty
            && public_id.is_some_and(|id| {
                QUIRKS_PUBLIC_IDENTIFIER_PREFIXES_WITHOUT_SYSTEM_IDENTIFIER
                    .iter()
                    .any(|prefix| starts_with_ignore_ascii_case(id, prefix))
            }));
    if quirks {
        return QuirksMode::Quirks;
    }

    let limited_quirks = public_id.is_some_and(|id| {
        LIMITED_QUIRKS_PUBLIC_IDENTIFIER_PREFIXES
            .iter()
            .any(|prefix| starts_with_ignore_ascii_case(id, prefix))
    }) || (!system_id_missing_or_empty
        && public_id.is_some_and(|id| {
            LIMITED_QUIRKS_PUBLIC_IDENTIFIER_PREFIXES_WITH_SYSTEM_IDENTIFIER
                .iter()
                .any(|prefix| starts_with_ignore_ascii_case(id, prefix))
        }));
    if limited_quirks {
        QuirksMode::LimitedQuirks
    } else {
        QuirksMode::NoQuirks
    }
}

/// The insertion mode (§13.2.4.1): determines how the tree-construction
/// dispatcher processes each token. Only the modes this crate's scope
/// decision actually needs are listed (see plan/03-tree-construction.md)
/// — no `InFrameset`/`AfterFrameset`/`AfterAfterFrameset` (frameset
/// documents can never validate against `html-conform`'s schema, so
/// there's nothing to build them for) and no `InTemplate` (`<template>`
/// is treated as a plain element, not given its own inert-content
/// insertion mode). Each mode's actual token-processing rules are a
/// separate, not-yet-implemented step — this enum only tracks *which*
/// mode is active, needed by algorithms like this one (§13.2.6.2) that
/// read/switch it before any mode's rules exist yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum InsertionMode {
    #[default]
    Initial,
    BeforeHtml,
    BeforeHead,
    InHead,
    InHeadNoscript,
    AfterHead,
    InBody,
    Text,
    InTable,
    InTableText,
    InCaption,
    InColumnGroup,
    InTableBody,
    InRow,
    InCell,
    AfterBody,
    AfterAfterBody,
}

/// Which variant of §13.2.6.2's "generic raw text/RCDATA element parsing
/// algorithm" to run — the spec text is identical between the two except
/// for which tokenizer state they switch to. `Script` is not literally
/// part of §13.2.6.2 (a `<script>` start tag has its own, much longer,
/// dedicated algorithm in "in head", §13.2.6.4.4) — but once its
/// script-execution bookkeeping is stripped out (parser document,
/// force-async, already-started flags, `document.write()` re-entrancy —
/// none of it applicable, this crate never executes scripts), what's
/// left (create the element, insert it, push it, switch the tokenizer
/// to script data state, remember the original insertion mode, switch
/// to "text") is exactly this same shape, so it's folded in here rather
/// than duplicated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GenericTextElementKind {
    RawText,
    Rcdata,
    Script,
}

/// The result of running one insertion-mode handler on a token: either
/// it was fully handled (optionally requesting a tokenizer state
/// switch, same convention as
/// [`generic_text_element_parsing_algorithm`](TreeBuilder::generic_text_element_parsing_algorithm)),
/// or the spec's "reprocess the token" instruction applies — almost
/// always because the handler just switched `insertion_mode` and wants
/// the *new* mode's handler to see the same token next.
enum TokenOutcome {
    Consumed(Option<ExternalState>),
    Reprocess,
}

/// True for the five whitespace characters every insertion mode's
/// "A character token that is one of ..." case lists (tab, LF, FF, CR,
/// space). This tokenizer's input-stream preprocessing (§13.2.3.5)
/// normalizes CR/CRLF to LF before tokenization, so a literal `'\r'`
/// character token can never actually reach here — CR is included
/// anyway for fidelity to the spec's literal wording, the same stance
/// taken for other practically-unreachable-but-spec-mandated checks
/// elsewhere in this file.
fn is_whitespace(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0C' | '\r' | ' ')
}

/// Where a node should be inserted: as a child of `parent`, immediately
/// before `before` — or, if `before` is `None`, as the last child.
/// §13.2.6.1's "adjusted insertion location", minus the "inside a
/// `template` element" adjustment (this crate treats `<template>` as a
/// plain element, not a separate inert content fragment — see
/// plan/03-tree-construction.md's Template scope decision) and the
/// fragment-parsing "root insertion target" step (this crate doesn't
/// implement the HTML fragment parsing algorithm).
struct InsertionLocation {
    parent: NodeId,
    before: Option<NodeId>,
}

/// Drives tree construction: owns the [`Document`] being built and the
/// [`OpenElementsStack`], and implements the shared insertion algorithms
/// (§13.2.6.1) that every insertion mode (not yet implemented — see
/// plan/03-tree-construction.md) is built on.
pub(crate) struct TreeBuilder {
    document: Document,
    open_elements: OpenElementsStack,
    active_formatting_elements: ActiveFormattingElements,
    /// Whether foster parenting (§13.2.6.1) is currently enabled. Set by
    /// insertion-mode rules (not yet implemented) while processing
    /// certain tokens in table-related contexts.
    foster_parenting: bool,
    /// The current insertion mode (§13.2.4.1). Defaults to `Initial`,
    /// though nothing switches away from it yet — the "initial" mode's
    /// own token-processing rules aren't implemented (see
    /// plan/03-tree-construction.md).
    insertion_mode: InsertionMode,
    /// The insertion mode saved by algorithms (like this one, §13.2.6.2)
    /// that temporarily switch to another mode and need to restore it
    /// afterward — restoring it is "text" mode's own job (not yet
    /// implemented) once it sees the RCDATA/RAWTEXT element's end tag.
    original_insertion_mode: Option<InsertionMode>,
    /// The document's quirks-mode flag (§13.2.6.4.1). Defaults to
    /// `NoQuirks`; set by the "initial" insertion mode via
    /// [`determine_quirks_mode`].
    quirks_mode: QuirksMode,
    /// The "head element pointer" (§13.2.4.1) — the `head` element once
    /// "before head" has inserted one, used by "after head" to reopen it
    /// temporarily even after it's been popped off the stack of open
    /// elements.
    head_element_pointer: Option<NodeId>,
    /// The "form element pointer" (§13.2.4.1) — the most recently
    /// opened `form` element that isn't inside a `template`, used by
    /// "in body" (§13.2.6.4.7) to find the right element to close for a
    /// `</form>` end tag even if it isn't the current node.
    form_element_pointer: Option<NodeId>,
    /// The "frameset-ok flag" (§13.2.4.1). Defaults to "ok" (`true`);
    /// most of "in body"'s content-inserting rules set it to "not ok".
    /// Nothing reads it yet — this crate doesn't implement the
    /// `<frameset>`/"in frameset" path at all (see the scope decision
    /// in plan/03-tree-construction.md) — but it's tracked faithfully
    /// regardless, the same stance taken for other pieces of state built
    /// ahead of their first real consumer.
    frameset_ok: bool,
    /// Sentinel for "if the next token is a U+000A LINE FEED (LF)
    /// character token, then ignore that token" (§13.2.6.4.7's `pre`/
    /// `listing`/`textarea` start-tag rules — an authoring convenience
    /// that trims one leading newline). Set to `true` right after
    /// inserting one of those elements; the next character-token check
    /// consumes it unconditionally (suppressing output only if that
    /// character actually is `'\n'`).
    skip_next_line_feed: bool,
    /// The "pending table character tokens" list (§13.2.4.1), used by
    /// "in table text" (§13.2.6.4.10) to buffer character tokens until
    /// a non-character token decides whether they're plain whitespace
    /// (inserted normally) or need foster parenting (reprocessed via
    /// "in table"'s "anything else" rule). Each entry keeps its own
    /// original position, matching how the tokenizer would have
    /// produced these as separate character tokens.
    pending_table_character_tokens: Vec<(char, Position)>,
}

impl TreeBuilder {
    pub(crate) fn new() -> Self {
        TreeBuilder {
            document: Document::new(),
            open_elements: OpenElementsStack::new(),
            active_formatting_elements: ActiveFormattingElements::new(),
            foster_parenting: false,
            insertion_mode: InsertionMode::default(),
            original_insertion_mode: None,
            quirks_mode: QuirksMode::default(),
            head_element_pointer: None,
            form_element_pointer: None,
            frameset_ok: true,
            skip_next_line_feed: false,
            pending_table_character_tokens: Vec::new(),
        }
    }

    /// "The appropriate place for inserting a node" (§13.2.6.1), given an
    /// optional override target (defaults to the current node).
    fn appropriate_place_for_inserting_a_node(
        &self,
        override_target: Option<NodeId>,
    ) -> InsertionLocation {
        let target = override_target.unwrap_or_else(|| {
            self.open_elements
                .current_node()
                .expect("inserting a node requires a current node on the stack")
        });

        if self.foster_parenting && self.is_foster_parenting_target(target) {
            return self.foster_parenting_location();
        }

        InsertionLocation {
            parent: target,
            before: None,
        }
    }

    /// True for the element types whose presence as the insertion target
    /// (while foster parenting is enabled) triggers foster parenting
    /// itself: `table`, `tbody`, `tfoot`, `thead`, `tr` (§13.2.6.1).
    fn is_foster_parenting_target(&self, node: NodeId) -> bool {
        let NodeKind::Element {
            name, namespace, ..
        } = &self.document.node(node).kind
        else {
            return false;
        };
        namespace.as_deref() == Some(HTML_NAMESPACE)
            && matches!(name.as_str(), "table" | "tbody" | "tfoot" | "thead" | "tr")
    }

    /// §13.2.6.1's foster-parenting substeps.
    fn foster_parenting_location(&self) -> InsertionLocation {
        let last_template =
            self.open_elements
                .last_matching(&self.document, HTML_NAMESPACE, "template");
        let last_table = self
            .open_elements
            .last_matching(&self.document, HTML_NAMESPACE, "table");

        if let Some(last_template) = last_template {
            let use_template = match last_table {
                None => true,
                Some(last_table) => self.open_elements.is_lower(last_template, last_table),
            };
            if use_template {
                // Simplified: <template> has no separate "template
                // contents" fragment in this crate's scope (see
                // plan/03-tree-construction.md) — insert directly inside
                // the template element itself.
                return InsertionLocation {
                    parent: last_template,
                    before: None,
                };
            }
        }

        let Some(last_table) = last_table else {
            // No table at all: insert inside the first stack element
            // (the `html` element). Spec calls this "the fragment case",
            // but it's reachable in ordinary (non-fragment) parsing too,
            // wherever the algorithm's other branches don't apply.
            let html = self
                .open_elements
                .topmost()
                .expect("foster parenting requires at least the html element on the stack");
            return InsertionLocation {
                parent: html,
                before: None,
            };
        };

        if let Some(parent) = self.document.parent(last_table) {
            return InsertionLocation {
                parent,
                before: Some(last_table),
            };
        }

        // last_table has no parent: unreachable in this single-pass,
        // non-scripting parser (nothing ever removes a node from its
        // parent once inserted), but implemented for spec fidelity.
        let previous_element = self
            .open_elements
            .element_immediately_above(last_table)
            .expect("a table with no parent must still have something above it on the stack");
        InsertionLocation {
            parent: previous_element,
            before: None,
        }
    }

    /// "Create an element for a token" (§13.2.6.1), simplified: no custom
    /// elements, no speculative parsing, no form-association, no `xmlns`
    /// validation — none of those apply without script execution or a
    /// live DOM, which are outside this crate's Step-1 scope. What
    /// remains: build a [`NodeKind::Element`] from the tag's name,
    /// `namespace`, and attributes (converted via
    /// [`document::Attribute`](Attribute)'s `From<tokenizer::Attribute>`
    /// for HTML-namespace elements; via
    /// [`adjust_attributes_for_foreign_element`] for everything else —
    /// "adjust MathML attributes"/"adjust SVG attributes"/"adjust
    /// foreign attributes" (§13.2.6.1/.5), called by every real
    /// caller that inserts a non-HTML element: "in body"'s `<math>`/
    /// `<svg>` start-tag rules and foreign content's "any other start
    /// tag").
    fn create_element_for_token(
        &mut self,
        tag: &TagToken,
        namespace: &str,
        position: Option<Position>,
    ) -> NodeId {
        let attributes = if namespace == HTML_NAMESPACE {
            tag.attributes
                .iter()
                .cloned()
                .map(Attribute::from)
                .collect()
        } else {
            adjust_attributes_for_foreign_element(tag, namespace)
        };
        self.document.new_node(
            NodeKind::Element {
                name: tag.name.clone(),
                namespace: Some(namespace.to_owned()),
                attributes,
            },
            position,
        )
    }

    /// "Insert an element at the adjusted insertion location" (§13.2.6.1),
    /// simplified: no custom-element-reaction-queue bookkeeping (no
    /// custom elements without script execution). Keeps the one
    /// tree-shape-relevant guard: a second root element is never
    /// inserted as a child of the document node itself.
    fn insert_element_at_adjusted_insertion_location(
        &mut self,
        element: NodeId,
        location: InsertionLocation,
    ) {
        if location.parent == self.document.root()
            && self.document.children(location.parent).next().is_some()
        {
            return;
        }
        self.document
            .insert_before(location.parent, location.before, element);
    }

    /// "Insert a foreign element" (§13.2.6.1). `only_add_to_element_stack`
    /// corresponds to the spec parameter of the same name — used by
    /// insertion-mode rules that build an element without actually
    /// placing it in the tree (not yet needed by anything implemented so
    /// far, but part of the algorithm's real signature).
    fn insert_foreign_element(
        &mut self,
        tag: &TagToken,
        namespace: &str,
        only_add_to_element_stack: bool,
        position: Option<Position>,
    ) -> NodeId {
        let location = self.appropriate_place_for_inserting_a_node(None);
        let element = self.create_element_for_token(tag, namespace, position);
        if !only_add_to_element_stack {
            self.insert_element_at_adjusted_insertion_location(element, location);
        }
        self.open_elements.push(element);
        element
    }

    /// "Insert an HTML element" (§13.2.6.1): insert a foreign element in
    /// the HTML namespace, actually placing it in the tree.
    fn insert_html_element(&mut self, tag: &TagToken, position: Option<Position>) -> NodeId {
        self.insert_foreign_element(tag, HTML_NAMESPACE, false, position)
    }

    /// §13.2.6.2's "generic raw text element parsing algorithm"/"generic
    /// RCDATA element parsing algorithm" — always invoked for a start tag
    /// token (`<title>`/`<textarea>` → RCDATA, `<style>`/`<xmp>`/
    /// `<iframe>`/`<noembed>`/`<noframes>` → RAWTEXT). This crate's
    /// tree-builder never holds the tokenizer itself (kept decoupled —
    /// tree_builder.rs only depends on tokenizer's plain data types
    /// otherwise), so instead of switching it directly, this returns the
    /// [`ExternalState`] the caller must feed to
    /// [`Tokenizer::switch_to`](crate::tokenizer::Tokenizer::switch_to)
    /// right after this returns.
    fn generic_text_element_parsing_algorithm(
        &mut self,
        tag: &TagToken,
        kind: GenericTextElementKind,
        position: Option<Position>,
    ) -> (NodeId, ExternalState) {
        let element = self.insert_html_element(tag, position);
        let external_state = match kind {
            GenericTextElementKind::RawText => ExternalState::RawText,
            GenericTextElementKind::Rcdata => ExternalState::RcData,
            GenericTextElementKind::Script => ExternalState::ScriptData,
        };
        self.original_insertion_mode = Some(self.insertion_mode);
        self.insertion_mode = InsertionMode::Text;
        (element, external_state)
    }

    /// Rebuilds an equivalent `TagToken` from an already-inserted
    /// element's current (immutable-once-parsed) name and attributes —
    /// used by "reconstruct the active formatting elements" to recreate
    /// a formatting element via [`insert_html_element`](Self::insert_html_element).
    fn tag_token_for(&self, node: NodeId) -> TagToken {
        let NodeKind::Element {
            name, attributes, ..
        } = &self.document.node(node).kind
        else {
            panic!("tag_token_for called on a non-element node")
        };
        TagToken {
            name: name.clone(),
            self_closing: false,
            attributes: attributes
                .iter()
                .map(|attribute| crate::tokenizer::Attribute {
                    name: attribute.name.clone(),
                    value: attribute.value.clone(),
                })
                .collect(),
        }
    }

    /// "Reconstruct the active formatting elements" (§13.2.4.3): reopens
    /// whichever formatting elements (`a`/`b`/`i`/...) were implicitly
    /// closed since they were last active, by recreating each as a fresh
    /// element via [`insert_html_element`](Self::insert_html_element) —
    /// which is why this had to wait until that existed (see
    /// plan/03-tree-construction.md's reordering note). Recreated
    /// elements get `position: None`: they aren't literally parsed at
    /// this point in the source, this crate's own extension over the
    /// spec (which has no position concept at all) treats them like any
    /// other synthesized node.
    fn reconstruct_the_active_formatting_elements(&mut self) {
        let Some(&last) = self.active_formatting_elements.entries.last() else {
            return;
        };
        let is_already_active = match last {
            FormattingEntry::Marker => true,
            FormattingEntry::Element(node) => self.open_elements.contains(node),
        };
        if is_already_active {
            return;
        }

        let last_index = self.active_formatting_elements.entries.len() - 1;
        let mut index = last_index;
        let create_index = loop {
            if index == 0 {
                break 0;
            }
            index -= 1;
            let is_stop = match self.active_formatting_elements.entries[index] {
                FormattingEntry::Marker => true,
                FormattingEntry::Element(node) => self.open_elements.contains(node),
            };
            if is_stop {
                break index + 1;
            }
        };

        let mut index = create_index;
        loop {
            let FormattingEntry::Element(old_node) = self.active_formatting_elements.entries[index]
            else {
                unreachable!(
                    "rewinding stops at a marker or in-stack element, so the Create step's \
                     entry (one position later) is always a formatting element"
                )
            };
            let tag = self.tag_token_for(old_node);
            let new_node = self.insert_html_element(&tag, None);
            self.active_formatting_elements.entries[index] = FormattingEntry::Element(new_node);
            if index == last_index {
                break;
            }
            index += 1;
        }
    }

    /// "Any other end tag" (§13.2.6.4.7, "in body" insertion mode's
    /// default end-tag handling) — also the fallback the adoption agency
    /// algorithm itself runs (step 3) when no matching formatting
    /// element exists in the list of active formatting elements. Built
    /// now, ahead of "in body" itself (not yet implemented — see
    /// plan/03-tree-construction.md), because the adoption agency
    /// algorithm cannot be complete without it.
    fn any_other_end_tag_in_body(&mut self, tag: &TagToken) {
        let mut node = self
            .open_elements
            .current_node()
            .expect("any other end tag requires at least one open element");
        loop {
            let is_match = matches!(
                &self.document.node(node).kind,
                NodeKind::Element { name, namespace, .. }
                    if namespace.as_deref() == Some(HTML_NAMESPACE) && name == &tag.name
            );
            if is_match {
                self.open_elements
                    .generate_implied_end_tags(&self.document, Some(tag.name.as_str()));
                // "If node is not the current node, then this is a parse
                // error" — parse-error-only, no tree-shape effect.
                loop {
                    let popped = self.open_elements.pop();
                    if popped == Some(node) {
                        break;
                    }
                }
                return;
            }
            if is_special(&self.document, node) {
                return;
            }
            node = self.open_elements.element_immediately_above(node).expect(
                "the html element is always in SPECIAL_CATEGORY, so this loop always \
                 returns via the is_special check above before running out of stack \
                 entries to walk above",
            );
        }
    }

    /// The adoption agency algorithm (§13.2.6.4.7) — handles misnested
    /// formatting element end tags (the canonical `<b><i>x</b>y</i>`
    /// case). `tag` is the end tag token being processed. Implemented as
    /// its own isolated step per plan/03-tree-construction.md (notorious
    /// for subtle bugs in other implementations), ahead of "in body"
    /// itself (not yet implemented) actually calling it.
    fn adoption_agency_algorithm(&mut self, tag: &TagToken) {
        let subject = tag.name.as_str();

        // Step 2: the common, non-misnested case — the current node
        // already *is* the element being closed, and isn't itself an
        // active formatting element, so there's nothing to adopt.
        if let Some(current) = self.open_elements.current_node() {
            let is_subject = matches!(
                &self.document.node(current).kind,
                NodeKind::Element { name, namespace, .. }
                    if namespace.as_deref() == Some(HTML_NAMESPACE) && name == subject
            );
            let is_active_formatting_element =
                self.active_formatting_elements.entries.iter().any(
                    |entry| matches!(entry, FormattingEntry::Element(node) if *node == current),
                );
            if is_subject && !is_active_formatting_element {
                self.open_elements.pop();
                return;
            }
        }

        // Steps 3-4: the outer loop, capped at 8 iterations.
        for _ in 0..8 {
            // Step 4.3: the last formatting element with this tag name,
            // searching only entries after the last marker (or from the
            // start of the list, if there is none).
            let marker_boundary = self
                .active_formatting_elements
                .entries
                .iter()
                .rposition(|entry| matches!(entry, FormattingEntry::Marker))
                .map_or(0, |marker_index| marker_index + 1);
            let formatting_element = self.active_formatting_elements.entries[marker_boundary..]
                .iter()
                .rev()
                .find_map(|entry| match entry {
                    FormattingEntry::Element(node) => {
                        let NodeKind::Element { name, .. } = &self.document.node(*node).kind else {
                            unreachable!("active formatting elements are always elements")
                        };
                        (name == subject).then_some(*node)
                    }
                    FormattingEntry::Marker => None,
                });
            let Some(formatting_element) = formatting_element else {
                self.any_other_end_tag_in_body(tag);
                return;
            };

            // Step 4.4
            if !self.open_elements.contains(formatting_element) {
                let index = self
                    .active_formatting_elements
                    .entries
                    .iter()
                    .position(|entry| {
                        matches!(entry, FormattingEntry::Element(node) if *node == formatting_element)
                    })
                    .expect("formattingElement was just found in this list");
                self.active_formatting_elements.entries.remove(index);
                return;
            }

            // Step 4.5
            let NodeKind::Element {
                name: formatting_element_name,
                ..
            } = &self.document.node(formatting_element).kind
            else {
                unreachable!("formattingElement is always an element")
            };
            if !self
                .open_elements
                .has_element_in_scope(&self.document, formatting_element_name)
            {
                return;
            }

            // Step 4.6 is a parse-error-only observation; no tree-shape
            // effect, nothing to do.

            // Step 4.7
            let furthest_block = self
                .open_elements
                .topmost_special_element_below(&self.document, formatting_element);
            let Some(furthest_block) = furthest_block else {
                // Step 4.8: no furthest block — pop up to and including
                // formattingElement, drop it from the active formatting
                // elements list, and stop.
                loop {
                    let popped = self.open_elements.pop();
                    if popped == Some(formatting_element) {
                        break;
                    }
                }
                let index = self
                    .active_formatting_elements
                    .entries
                    .iter()
                    .position(|entry| {
                        matches!(entry, FormattingEntry::Element(node) if *node == formatting_element)
                    })
                    .expect("formattingElement was just found in this list");
                self.active_formatting_elements.entries.remove(index);
                return;
            };

            // Step 4.9
            let common_ancestor = self
                .open_elements
                .element_immediately_above(formatting_element)
                .expect("formattingElement always has an element above it (at minimum html)");

            // Step 4.10: the bookmark, tracked as a running index into
            // the active formatting elements list, kept in sync with
            // every removal/insertion this algorithm performs on that
            // list below.
            let mut bookmark = self
                .active_formatting_elements
                .entries
                .iter()
                .position(|entry| {
                    matches!(entry, FormattingEntry::Element(node) if *node == formatting_element)
                })
                .expect("formattingElement was just found in this list");

            // Steps 4.11-4.13: the inner loop.
            let mut node;
            let mut last_node = furthest_block;
            let mut node_above = self.open_elements.element_immediately_above(furthest_block);
            let mut inner_loop_counter = 0u32;
            loop {
                inner_loop_counter += 1;

                // Step 4.13.2: advance to the element above the previous
                // `node`, cached *before* this iteration's own possible
                // stack removal below (once removed, its neighbor can no
                // longer be looked up positionally).
                node = node_above.expect(
                    "walking upward from furthestBlock always reaches formattingElement, \
                     which sits strictly above it, before running out of stack entries",
                );
                node_above = self.open_elements.element_immediately_above(node);

                // Step 4.13.3
                if node == formatting_element {
                    break;
                }

                // Step 4.13.4
                if inner_loop_counter > 3
                    && let Some(index) = self.active_formatting_elements.entries.iter().position(
                        |entry| matches!(entry, FormattingEntry::Element(afe_node) if *afe_node == node),
                    )
                {
                    self.active_formatting_elements.entries.remove(index);
                    if index < bookmark {
                        bookmark -= 1;
                    }
                }

                // Step 4.13.5
                let node_afe_index = self.active_formatting_elements.entries.iter().position(
                    |entry| matches!(entry, FormattingEntry::Element(afe_node) if *afe_node == node),
                );
                let Some(node_afe_index) = node_afe_index else {
                    let stack_index = self
                        .open_elements
                        .entries
                        .iter()
                        .position(|&entry| entry == node)
                        .expect("node is currently on the stack of open elements");
                    self.open_elements.entries.remove(stack_index);
                    continue;
                };

                // Step 4.13.6
                let node_tag = self.tag_token_for(node);
                let new_node = self.create_element_for_token(&node_tag, HTML_NAMESPACE, None);
                self.active_formatting_elements.entries[node_afe_index] =
                    FormattingEntry::Element(new_node);
                let stack_index = self
                    .open_elements
                    .entries
                    .iter()
                    .position(|&entry| entry == node)
                    .expect("node is currently on the stack of open elements");
                self.open_elements.entries[stack_index] = new_node;

                // Step 4.13.7
                if last_node == furthest_block {
                    bookmark = node_afe_index + 1;
                }

                // Steps 4.13.8-4.13.9 — `append_child` detaches lastNode
                // from wherever it's currently attached first (see
                // `Document::insert_before`'s doc comment).
                self.document.append_child(new_node, last_node);
                last_node = new_node;
            }

            // Steps 4.14-4.17: "the adjusted insertion location given
            // commonAncestor" is exactly `appropriate_place_for_inserting_a_node`
            // (§13.2.6.1) — the same helper `insert_html_element` uses,
            // foster parenting included.
            let location = self.appropriate_place_for_inserting_a_node(Some(common_ancestor));

            // Step 4.18
            self.document.remove(last_node);

            // Step 4.19: all four conditions transcribed literally, even
            // though in this single-pass parser `target` is never the
            // Document node and `lastNode` can't practically end up an
            // ancestor of `target` — matching this crate's established
            // fidelity-over-pruning stance for spec-mandated but
            // practically unreachable guards (see foster parenting's
            // "table has no parent" fallback).
            let last_node_parent_is_null = self.document.parent(last_node).is_none();
            let last_node_is_not_ancestor_of_target = !self
                .document
                .is_inclusive_ancestor(last_node, location.parent);
            let target_is_not_document_with_element_child =
                !(matches!(self.document.node(location.parent).kind, NodeKind::Document)
                    && self.document.children(location.parent).any(|child| {
                        matches!(self.document.node(child).kind, NodeKind::Element { .. })
                    }));
            let ref_node_is_null_or_its_parent_is_target = match location.before {
                Some(reference) => self.document.parent(reference) == Some(location.parent),
                None => true,
            };
            if last_node_parent_is_null
                && last_node_is_not_ancestor_of_target
                && target_is_not_document_with_element_child
                && ref_node_is_null_or_its_parent_is_target
            {
                self.document
                    .insert_before(location.parent, location.before, last_node);
            }

            // Step 4.20
            let formatting_element_tag = self.tag_token_for(formatting_element);
            let new_formatting_element =
                self.create_element_for_token(&formatting_element_tag, HTML_NAMESPACE, None);

            // Step 4.21
            let furthest_block_children: Vec<_> = self.document.children(furthest_block).collect();
            for child in furthest_block_children {
                self.document.append_child(new_formatting_element, child);
            }

            // Step 4.22
            self.document
                .append_child(furthest_block, new_formatting_element);

            // Step 4.23
            let formatting_element_afe_index = self
                .active_formatting_elements
                .entries
                .iter()
                .position(|entry| {
                    matches!(entry, FormattingEntry::Element(node) if *node == formatting_element)
                })
                .expect("formattingElement's own entry is never touched by the inner loop");
            self.active_formatting_elements
                .entries
                .remove(formatting_element_afe_index);
            if formatting_element_afe_index < bookmark {
                bookmark -= 1;
            }
            self.active_formatting_elements
                .entries
                .insert(bookmark, FormattingEntry::Element(new_formatting_element));

            // Step 4.24
            let formatting_element_stack_index = self
                .open_elements
                .entries
                .iter()
                .position(|&entry| entry == formatting_element)
                .expect("formattingElement is still on the stack of open elements");
            self.open_elements
                .entries
                .remove(formatting_element_stack_index);
            let furthest_block_stack_index = self
                .open_elements
                .entries
                .iter()
                .position(|&entry| entry == furthest_block)
                .expect("furthestBlock is still on the stack of open elements");
            self.open_elements
                .entries
                .insert(furthest_block_stack_index + 1, new_formatting_element);
        }
    }

    /// "Insert a character" (§13.2.6.1): appends to the last child if
    /// it's already a text node, otherwise creates a new one. Also
    /// implements the "insertion location is inside a Document node"
    /// guard (character data is silently dropped there — the DOM never
    /// allows Text children of the Document node).
    fn insert_character(&mut self, c: char, position: Option<Position>) {
        let location = self.appropriate_place_for_inserting_a_node(None);
        if location.parent == self.document.root() {
            return;
        }
        // "If there is a Text node immediately before insertionLocation" —
        // that's the previous sibling of `before` (append case: `before`
        // is None, so it's the parent's last child instead).
        let node_immediately_before = match location.before {
            Some(before) => self.document.prev_sibling(before),
            None => self.document.last_child(location.parent),
        };
        if let Some(node) = node_immediately_before
            && let NodeKind::Text { content } = &mut self.document.node_mut(node).kind
        {
            content.push(c);
            return;
        }
        let text = self.document.new_node(
            NodeKind::Text {
                content: c.to_string(),
            },
            position,
        );
        self.document
            .insert_before(location.parent, location.before, text);
    }

    /// "Insert a comment" (§13.2.6.1).
    fn insert_comment(&mut self, data: String, position: Option<Position>) {
        let location = self.appropriate_place_for_inserting_a_node(None);
        let comment = self
            .document
            .new_node(NodeKind::Comment { content: data }, position);
        self.document
            .insert_before(location.parent, location.before, comment);
    }

    /// "Insert a processing instruction" (§13.2.6.1).
    fn insert_processing_instruction(
        &mut self,
        target: String,
        data: String,
        position: Option<Position>,
    ) {
        let location = self.appropriate_place_for_inserting_a_node(None);
        let pi = self
            .document
            .new_node(NodeKind::ProcessingInstruction { target, data }, position);
        self.document
            .insert_before(location.parent, location.before, pi);
    }

    /// "Insert a comment as the last child of the Document object"
    /// (used by the "initial" and "before html" insertion modes'
    /// comment-token rules) — distinct from the generic
    /// [`insert_comment`](Self::insert_comment) (§13.2.6.1), which
    /// targets the *current node* instead. Both modes run before any
    /// element (hence no current node) exists on the stack of open
    /// elements, so the generic algorithm doesn't apply here.
    fn append_comment_to_document(&mut self, data: String, position: Position) {
        let comment = self
            .document
            .new_node(NodeKind::Comment { content: data }, Some(position));
        let root = self.document.root();
        self.document.append_child(root, comment);
    }

    /// "Insert a processing instruction as the last child of the
    /// Document object" — the PI-token counterpart to
    /// [`append_comment_to_document`](Self::append_comment_to_document).
    fn append_processing_instruction_to_document(
        &mut self,
        target: String,
        data: String,
        position: Position,
    ) {
        let pi = self.document.new_node(
            NodeKind::ProcessingInstruction { target, data },
            Some(position),
        );
        let root = self.document.root();
        self.document.append_child(root, pi);
    }

    /// "Insert a comment as the last child of the first element in the
    /// stack of open elements (the html element)" — "after body"'s
    /// (§13.2.6.4.17) comment-token rule.
    fn append_comment_to_html_element(&mut self, data: String, position: Position) {
        let html = self
            .open_elements
            .topmost()
            .expect("\"after body\" always runs with html on the stack of open elements");
        let comment = self
            .document
            .new_node(NodeKind::Comment { content: data }, Some(position));
        self.document.append_child(html, comment);
    }

    /// The PI-token counterpart to
    /// [`append_comment_to_html_element`](Self::append_comment_to_html_element).
    fn append_processing_instruction_to_html_element(
        &mut self,
        target: String,
        data: String,
        position: Position,
    ) {
        let html = self
            .open_elements
            .topmost()
            .expect("\"after body\" always runs with html on the stack of open elements");
        let pi = self.document.new_node(
            NodeKind::ProcessingInstruction { target, data },
            Some(position),
        );
        self.document.append_child(html, pi);
    }

    /// "In body"'s rule for a start tag whose tag name is "html"
    /// (§13.2.6.4.7): merge any attributes not already present onto the
    /// existing (topmost) `html` element, unless a `template` element
    /// is currently open. Several other insertion modes ("before head",
    /// "after head", and others not yet implemented) delegate `<html>`
    /// start tags to "in body" wholesale — but a token that *is* an
    /// html start tag can only ever hit this one "in body" rule, so
    /// it's implemented standalone here rather than waiting for the
    /// rest of "in body" to exist.
    fn merge_attributes_onto_html_element(&mut self, tag: &TagToken) {
        if self.has_template_on_stack() {
            return;
        }
        let html = self
            .open_elements
            .topmost()
            .expect("this rule never runs before the html element has been pushed");
        self.merge_new_attributes_onto(html, tag);
    }

    /// Adds each of `tag`'s attributes to `node` that isn't already
    /// present there by name — the "for each attribute on the token,
    /// check to see if the attribute is already present [...], if it is
    /// not, add [it]" pattern shared by "in body"'s `<html>` (via
    /// [`merge_attributes_onto_html_element`](Self::merge_attributes_onto_html_element))
    /// and `<body>` start-tag rules (§13.2.6.4.7).
    fn merge_new_attributes_onto(&mut self, node: NodeId, tag: &TagToken) {
        let NodeKind::Element { attributes, .. } = &mut self.document.node_mut(node).kind else {
            unreachable!("merge_new_attributes_onto is always called with an element node")
        };
        for attribute in &tag.attributes {
            if !attributes
                .iter()
                .any(|existing| existing.name == attribute.name)
            {
                attributes.push(Attribute::from(attribute.clone()));
            }
        }
    }

    /// The tree-construction dispatcher (§13.2.6): for each token,
    /// first decides — via [`should_process_as_foreign_content`](Self::should_process_as_foreign_content) —
    /// whether it's processed "in HTML content" (routed to the handler
    /// for the current insertion mode) or "in foreign content"
    /// (§13.2.6.5), looping to reprocess it whenever a handler switches
    /// modes/context without otherwise consuming the token — the spec's
    /// "reprocess the token" instruction, which appears throughout both.
    /// Returns the tokenizer state switch (if any) the caller must
    /// apply via `Tokenizer::switch_to`, same convention as
    /// [`generic_text_element_parsing_algorithm`](Self::generic_text_element_parsing_algorithm).
    pub(crate) fn process_token(
        &mut self,
        kind: &TokenKind,
        position: Position,
    ) -> Option<ExternalState> {
        loop {
            let outcome = if self.should_process_as_foreign_content(kind) {
                self.process_token_foreign_content(kind, position)
            } else {
                match self.insertion_mode {
                    InsertionMode::Initial => self.process_token_initial(kind, position),
                    InsertionMode::BeforeHtml => self.process_token_before_html(kind, position),
                    InsertionMode::BeforeHead => self.process_token_before_head(kind, position),
                    InsertionMode::InHead => self.process_token_in_head(kind, position),
                    InsertionMode::InHeadNoscript => {
                        self.process_token_in_head_noscript(kind, position)
                    }
                    InsertionMode::AfterHead => self.process_token_after_head(kind, position),
                    InsertionMode::InBody => self.process_token_in_body(kind, position),
                    InsertionMode::Text => self.process_token_text(kind, position),
                    InsertionMode::InTable => self.process_token_in_table(kind, position),
                    InsertionMode::InTableText => self.process_token_in_table_text(kind, position),
                    InsertionMode::InCaption => self.process_token_in_caption(kind, position),
                    InsertionMode::InColumnGroup => {
                        self.process_token_in_column_group(kind, position)
                    }
                    InsertionMode::InTableBody => self.process_token_in_table_body(kind, position),
                    InsertionMode::InRow => self.process_token_in_row(kind, position),
                    InsertionMode::InCell => self.process_token_in_cell(kind, position),
                    InsertionMode::AfterBody => self.process_token_after_body(kind, position),
                    InsertionMode::AfterAfterBody => {
                        self.process_token_after_after_body(kind, position)
                    }
                }
            };
            match outcome {
                TokenOutcome::Consumed(state) => return state,
                TokenOutcome::Reprocess => continue,
            }
        }
    }

    /// The tree construction dispatcher's own test (§13.2.6): true if
    /// `kind` should be processed "in foreign content" (§13.2.6.5)
    /// rather than by the current insertion mode's HTML-content rules.
    /// The "stack of open elements is empty" case returns `false`
    /// (HTML content) implicitly, since [`adjusted_current_node`](Self::adjusted_current_node)
    /// is `None` then.
    fn should_process_as_foreign_content(&self, kind: &TokenKind) -> bool {
        let Some(node) = self.adjusted_current_node() else {
            return false;
        };
        if self.is_html_namespace_element(node) {
            return false;
        }
        if is_mathml_text_integration_point(&self.document, node) {
            match kind {
                TokenKind::StartTag(tag)
                    if !matches!(tag.name.as_str(), "mglyph" | "malignmark") =>
                {
                    return false;
                }
                TokenKind::Character(_) => return false,
                _ => {}
            }
        }
        if self.node_has_namespace_and_name(node, MATHML_NAMESPACE, "annotation-xml")
            && matches!(kind, TokenKind::StartTag(tag) if tag.name == "svg")
        {
            return false;
        }
        if is_html_integration_point(&self.document, node)
            && matches!(kind, TokenKind::StartTag(_) | TokenKind::Character(_))
        {
            return false;
        }
        if matches!(kind, TokenKind::Eof) {
            return false;
        }
        true
    }

    /// Pops elements off the stack of open elements while the current
    /// node is foreign content that isn't an integration point — the
    /// shared "pop back out of foreign content" step used by both of
    /// foreign content's implicit-close-and-reprocess cases (the block-
    /// element start-tag group and the `br`/`p` end tags, §13.2.6.5).
    fn pop_out_of_foreign_content(&mut self) {
        while let Some(current) = self.open_elements.current_node() {
            if is_mathml_text_integration_point(&self.document, current)
                || is_html_integration_point(&self.document, current)
                || self.is_html_namespace_element(current)
            {
                break;
            }
            self.open_elements.pop();
        }
    }

    /// True if the current node is an SVG `script` element — used by
    /// the `</script>` special case (§13.2.6.5).
    fn current_node_is_svg_script(&self) -> bool {
        self.open_elements
            .current_node()
            .is_some_and(|node| self.node_has_namespace_and_name(node, SVG_NAMESPACE, "script"))
    }

    /// Pops elements off the stack of open elements until `target` has
    /// been popped, by identity.
    fn pop_until_node_popped(&mut self, target: NodeId) {
        loop {
            let popped = self.open_elements.pop();
            if popped == Some(target) {
                break;
            }
        }
    }

    /// "The rules for parsing tokens in foreign content" (§13.2.6.5).
    /// Script-execution machinery (the speculative parser, script
    /// nesting level, insertion-point save/restore, actually executing
    /// SVG `<script>`) is entirely out of scope, same as everywhere
    /// else in this crate — the `</script>` special case (whose real
    /// purpose is running the script at exactly the right moment)
    /// reduces to a plain pop once that's stripped out, same as the
    /// generic "any other end tag" fallback right below it.
    fn process_token_foreign_content(
        &mut self,
        kind: &TokenKind,
        position: Position,
    ) -> TokenOutcome {
        match kind {
            TokenKind::Character(c) if *c == '\0' => {
                self.insert_character('\u{FFFD}', Some(position));
                TokenOutcome::Consumed(None)
            }
            TokenKind::Character(c) if is_whitespace(*c) => {
                self.insert_character(*c, Some(position));
                TokenOutcome::Consumed(None)
            }
            TokenKind::Character(c) => {
                self.insert_character(*c, Some(position));
                self.frameset_ok = false;
                TokenOutcome::Consumed(None)
            }
            TokenKind::Comment(data) => {
                self.insert_comment(data.clone(), Some(position));
                TokenOutcome::Consumed(None)
            }
            TokenKind::ProcessingInstruction(pi) => {
                self.insert_processing_instruction(
                    pi.target.clone(),
                    pi.data.clone(),
                    Some(position),
                );
                TokenOutcome::Consumed(None)
            }
            TokenKind::Doctype(_) => TokenOutcome::Consumed(None),
            TokenKind::StartTag(tag)
                if matches!(
                    tag.name.as_str(),
                    "b" | "big"
                        | "blockquote"
                        | "body"
                        | "br"
                        | "center"
                        | "code"
                        | "dd"
                        | "div"
                        | "dl"
                        | "dt"
                        | "em"
                        | "embed"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                        | "head"
                        | "hr"
                        | "i"
                        | "img"
                        | "li"
                        | "listing"
                        | "menu"
                        | "meta"
                        | "nobr"
                        | "ol"
                        | "p"
                        | "pre"
                        | "ruby"
                        | "s"
                        | "small"
                        | "span"
                        | "strong"
                        | "strike"
                        | "sub"
                        | "sup"
                        | "table"
                        | "tt"
                        | "u"
                        | "ul"
                        | "var"
                ) || (tag.name == "font"
                    && tag.attributes.iter().any(|attribute| {
                        matches!(attribute.name.as_str(), "color" | "face" | "size")
                    })) =>
            {
                self.pop_out_of_foreign_content();
                TokenOutcome::Reprocess
            }
            TokenKind::EndTag(tag) if matches!(tag.name.as_str(), "br" | "p") => {
                self.pop_out_of_foreign_content();
                TokenOutcome::Reprocess
            }
            TokenKind::StartTag(tag) => {
                let namespace = self
                    .adjusted_current_node()
                    .and_then(|node| self.namespace_of(node))
                    .expect("foreign content dispatch always has a foreign adjusted current node");
                let adjusted_tag = if namespace == SVG_NAMESPACE {
                    TagToken {
                        name: adjust_svg_tag_name(&tag.name).to_owned(),
                        ..tag.clone()
                    }
                } else {
                    tag.clone()
                };
                self.insert_foreign_element(&adjusted_tag, &namespace, false, Some(position));
                if adjusted_tag.self_closing {
                    // Both self-closing branches (SVG <script>, acting
                    // as its own end tag below, vs. every other
                    // element) reduce to the same pop once script
                    // execution is stripped out.
                    self.open_elements.pop();
                }
                TokenOutcome::Consumed(None)
            }
            TokenKind::EndTag(tag) if tag.name == "script" && self.current_node_is_svg_script() => {
                self.open_elements.pop();
                TokenOutcome::Consumed(None)
            }
            TokenKind::EndTag(tag) => {
                let mut node = self.open_elements.current_node();
                loop {
                    let current = node.expect(
                        "the stack of open elements always contains at least html by the time \
                         this runs",
                    );
                    if self.open_elements.topmost() == Some(current) {
                        return TokenOutcome::Consumed(None);
                    }
                    if self.element_name_matches_ignore_case(current, &tag.name) {
                        self.pop_until_node_popped(current);
                        return TokenOutcome::Consumed(None);
                    }
                    let next = self
                        .open_elements
                        .element_immediately_above(current)
                        .expect("current is not topmost, so an element always sits above it");
                    if self.is_html_namespace_element(next) {
                        return TokenOutcome::Reprocess;
                    }
                    node = Some(next);
                }
            }
            TokenKind::Eof => unreachable!(
                "should_process_as_foreign_content always returns false for an EOF token, \
                 so process_token never dispatches one here"
            ),
        }
    }

    /// True if `node`'s tag name matches `tag_name` under ASCII
    /// case-insensitive comparison — foreign content's "any other end
    /// tag" rule (§13.2.6.5) explicitly lowercases the (possibly
    /// mixed-case, e.g. SVG) node name before comparing it against the
    /// token's tag name (which always arrives all-lowercase from the
    /// tokenizer).
    fn element_name_matches_ignore_case(&self, node: NodeId, tag_name: &str) -> bool {
        matches!(
            &self.document.node(node).kind,
            NodeKind::Element { name, .. } if name.eq_ignore_ascii_case(tag_name)
        )
    }

    /// The "initial" insertion mode (§13.2.6.4.1).
    fn process_token_initial(&mut self, kind: &TokenKind, position: Position) -> TokenOutcome {
        match kind {
            TokenKind::Character(c) if is_whitespace(*c) => TokenOutcome::Consumed(None),
            TokenKind::Comment(data) => {
                self.append_comment_to_document(data.clone(), position);
                TokenOutcome::Consumed(None)
            }
            TokenKind::ProcessingInstruction(pi) => {
                self.append_processing_instruction_to_document(
                    pi.target.clone(),
                    pi.data.clone(),
                    position,
                );
                TokenOutcome::Consumed(None)
            }
            TokenKind::Doctype(doctype) => {
                // The parse-error checks (wrong name/public-identifier/
                // system-identifier shape) have no tree-shape effect —
                // omitted.
                let root = self.document.root();
                let has_doctype_or_element_child = self.document.children(root).any(|child| {
                    matches!(
                        self.document.node(child).kind,
                        NodeKind::Doctype { .. } | NodeKind::Element { .. }
                    )
                });
                if !has_doctype_or_element_child {
                    let doctype_node = self.document.new_node(
                        NodeKind::Doctype {
                            name: Some(doctype.name.clone().unwrap_or_default()),
                            public_identifier: Some(
                                doctype.public_identifier.clone().unwrap_or_default(),
                            ),
                            system_identifier: Some(
                                doctype.system_identifier.clone().unwrap_or_default(),
                            ),
                        },
                        Some(position),
                    );
                    self.document.append_child(root, doctype_node);
                }
                self.quirks_mode = determine_quirks_mode(doctype);
                self.insertion_mode = InsertionMode::BeforeHtml;
                TokenOutcome::Consumed(None)
            }
            _ => {
                // "Anything else": not an iframe srcdoc document (this
                // crate never parses srcdoc content), so set quirks
                // mode unconditionally, then switch and reprocess.
                self.quirks_mode = QuirksMode::Quirks;
                self.insertion_mode = InsertionMode::BeforeHtml;
                TokenOutcome::Reprocess
            }
        }
    }

    /// The "before html" insertion mode (§13.2.6.4.2).
    fn process_token_before_html(&mut self, kind: &TokenKind, position: Position) -> TokenOutcome {
        match kind {
            TokenKind::Doctype(_) => TokenOutcome::Consumed(None),
            TokenKind::Comment(data) => {
                self.append_comment_to_document(data.clone(), position);
                TokenOutcome::Consumed(None)
            }
            TokenKind::ProcessingInstruction(pi) => {
                self.append_processing_instruction_to_document(
                    pi.target.clone(),
                    pi.data.clone(),
                    position,
                );
                TokenOutcome::Consumed(None)
            }
            TokenKind::Character(c) if is_whitespace(*c) => TokenOutcome::Consumed(None),
            TokenKind::StartTag(tag) if tag.name == "html" => {
                let element = self.create_element_for_token(tag, HTML_NAMESPACE, Some(position));
                let root = self.document.root();
                self.document.append_child(root, element);
                self.open_elements.push(element);
                self.insertion_mode = InsertionMode::BeforeHead;
                TokenOutcome::Consumed(None)
            }
            TokenKind::EndTag(tag)
                if matches!(tag.name.as_str(), "head" | "body" | "html" | "br") =>
            {
                self.before_html_anything_else()
            }
            TokenKind::EndTag(_) => TokenOutcome::Consumed(None),
            _ => self.before_html_anything_else(),
        }
    }

    /// "Before html"'s "anything else" case: synthesize an `html`
    /// element directly (bypassing the generic insertion algorithm,
    /// which requires a current node that doesn't exist yet), push it,
    /// switch to "before head", and reprocess.
    fn before_html_anything_else(&mut self) -> TokenOutcome {
        let html = self.document.new_node(
            NodeKind::Element {
                name: "html".to_owned(),
                namespace: Some(HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            },
            None,
        );
        let root = self.document.root();
        self.document.append_child(root, html);
        self.open_elements.push(html);
        self.insertion_mode = InsertionMode::BeforeHead;
        TokenOutcome::Reprocess
    }

    /// The "before head" insertion mode (§13.2.6.4.3).
    fn process_token_before_head(&mut self, kind: &TokenKind, position: Position) -> TokenOutcome {
        match kind {
            TokenKind::Character(c) if is_whitespace(*c) => TokenOutcome::Consumed(None),
            TokenKind::Comment(data) => {
                self.insert_comment(data.clone(), Some(position));
                TokenOutcome::Consumed(None)
            }
            TokenKind::ProcessingInstruction(pi) => {
                self.insert_processing_instruction(
                    pi.target.clone(),
                    pi.data.clone(),
                    Some(position),
                );
                TokenOutcome::Consumed(None)
            }
            TokenKind::Doctype(_) => TokenOutcome::Consumed(None),
            TokenKind::StartTag(tag) if tag.name == "html" => {
                self.merge_attributes_onto_html_element(tag);
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag) if tag.name == "head" => {
                let head = self.insert_html_element(tag, Some(position));
                self.head_element_pointer = Some(head);
                self.insertion_mode = InsertionMode::InHead;
                TokenOutcome::Consumed(None)
            }
            TokenKind::EndTag(tag)
                if matches!(tag.name.as_str(), "head" | "body" | "html" | "br") =>
            {
                self.before_head_anything_else()
            }
            TokenKind::EndTag(_) => TokenOutcome::Consumed(None),
            _ => self.before_head_anything_else(),
        }
    }

    /// "Before head"'s "anything else" case: insert a synthesized
    /// `head` start tag (no attributes), remember it via the head
    /// element pointer, switch to "in head", and reprocess.
    fn before_head_anything_else(&mut self) -> TokenOutcome {
        let head_tag = TagToken {
            name: "head".to_owned(),
            self_closing: false,
            attributes: vec![],
        };
        let head = self.insert_html_element(&head_tag, None);
        self.head_element_pointer = Some(head);
        self.insertion_mode = InsertionMode::InHead;
        TokenOutcome::Reprocess
    }

    /// True if `node` is an HTML-namespace element with tag name `name`
    /// — the "node is a(n) X element" check that recurs throughout
    /// §13.2.6.4.7 ("in body") and elsewhere.
    fn node_has_html_name(&self, node: NodeId, name: &str) -> bool {
        self.node_has_namespace_and_name(node, HTML_NAMESPACE, name)
    }

    /// True if `node` is an element in `namespace` with tag name `name`
    /// — the namespace-generalized form of
    /// [`node_has_html_name`](Self::node_has_html_name), needed for
    /// foreign-content checks (§13.2.6.5) that care about MathML/SVG
    /// elements specifically.
    fn node_has_namespace_and_name(&self, node: NodeId, namespace: &str, name: &str) -> bool {
        matches!(
            &self.document.node(node).kind,
            NodeKind::Element { name: node_name, namespace: node_namespace, .. }
                if node_namespace.as_deref() == Some(namespace) && node_name == name
        )
    }

    /// True if `node` is an element in the HTML namespace, regardless
    /// of tag name — the tree-construction dispatcher's (§13.2.6)
    /// primary foreign-vs-HTML-content test.
    fn is_html_namespace_element(&self, node: NodeId) -> bool {
        matches!(
            &self.document.node(node).kind,
            NodeKind::Element { namespace, .. } if namespace.as_deref() == Some(HTML_NAMESPACE)
        )
    }

    /// `node`'s namespace, if it's an element — owned rather than
    /// borrowed so callers can use it across a subsequent `&mut self`
    /// call (e.g. inserting a new element into that same namespace).
    fn namespace_of(&self, node: NodeId) -> Option<String> {
        match &self.document.node(node).kind {
            NodeKind::Element { namespace, .. } => namespace.clone(),
            _ => None,
        }
    }

    /// The "adjusted current node" (§13.2.6): the context element
    /// substitution for the HTML fragment parsing algorithm never
    /// applies (this crate never does fragment parsing), so this is
    /// always just the current node.
    fn adjusted_current_node(&self) -> Option<NodeId> {
        self.open_elements.current_node()
    }

    /// Whether `Tokenizer::set_in_foreign_content` should currently be
    /// `true` — the adjusted current node exists and isn't an HTML-
    /// namespace element (matches that flag's documented contract
    /// exactly: "whether it is currently in a non-HTML namespace",
    /// nothing about integration points). The driver loop
    /// (`lib.rs::parse`) calls this after every token and forwards the
    /// result, since this crate's `TreeBuilder` never holds the
    /// `Tokenizer` itself to call it directly.
    pub(crate) fn is_in_foreign_content(&self) -> bool {
        self.adjusted_current_node()
            .is_some_and(|node| !self.is_html_namespace_element(node))
    }

    /// Consumes the builder and returns the finished [`Document`] — the
    /// driver loop's (`lib.rs::parse`) final step once the tokenizer
    /// has no more tokens.
    pub(crate) fn into_document(self) -> Document {
        self.document
    }

    /// True if a `template` element is currently on the stack of open
    /// elements — used by the (simplified) `<template>` handling in "in
    /// head" and by [`merge_attributes_onto_html_element`](Self::merge_attributes_onto_html_element).
    fn has_template_on_stack(&self) -> bool {
        self.open_elements
            .entries
            .iter()
            .any(|&node| self.node_has_html_name(node, "template"))
    }

    /// Pops elements off the stack of open elements until one whose tag
    /// name is in `names` (HTML namespace) has been popped — the "pop
    /// elements from the stack of open elements until an X element has
    /// been popped from the stack" pattern that recurs throughout "in
    /// body" (§13.2.6.4.7). Callers only invoke this once they've
    /// already confirmed (via a scope check) that a matching element
    /// genuinely exists on the stack, exactly mirroring every one of
    /// "in body"'s own end-tag rules.
    fn pop_until_one_of_popped(&mut self, names: &[&str]) {
        loop {
            let popped = self.open_elements.pop();
            let is_match = popped.is_some_and(|node| {
                names
                    .iter()
                    .any(|&name| self.node_has_html_name(node, name))
            });
            if is_match {
                break;
            }
        }
    }

    /// Removes `node` from the stack of open elements, wherever it
    /// currently is — not necessarily the current node. Used where the
    /// spec says "remove node from the stack of open elements" rather
    /// than "pop".
    fn remove_node_from_open_elements(&mut self, node: NodeId) {
        if let Some(index) = self.open_elements.entries.iter().position(|&n| n == node) {
            self.open_elements.entries.remove(index);
        }
    }

    /// Removes `node`'s entry from the list of active formatting
    /// elements, if it has one.
    fn remove_node_from_active_formatting_elements(&mut self, node: NodeId) {
        if let Some(index) = self.active_formatting_elements.entries.iter().position(
            |entry| matches!(entry, FormattingEntry::Element(afe_node) if *afe_node == node),
        ) {
            self.active_formatting_elements.entries.remove(index);
        }
    }

    /// "Close a p element" (§13.2.6.4.7): generate implied end tags
    /// except for `p` elements, then pop until a `p` element has been
    /// popped. Callers are expected to have already checked
    /// `has_element_in_button_scope("p")` first, per every one of this
    /// helper's actual call sites in the spec.
    fn close_a_p_element(&mut self) {
        self.open_elements
            .generate_implied_end_tags(&self.document, Some("p"));
        self.pop_until_one_of_popped(&["p"]);
    }

    /// True if `node` — a *specific* element, by identity rather than
    /// by tag name — is reachable from the current node without
    /// crossing a default-scope boundary. Needed by "in body"'s
    /// `</form>` rule (§13.2.6.4.7), which tracks a specific `form`
    /// element via the form element pointer rather than searching by
    /// name (there's normally at most one open `form` at a time, but
    /// the spec phrases the check node-identity-first regardless).
    fn has_node_in_scope(&self, node: NodeId) -> bool {
        for &entry in self.open_elements.entries.iter().rev() {
            if entry == node {
                return true;
            }
            let NodeKind::Element {
                name, namespace, ..
            } = &self.document.node(entry).kind
            else {
                continue;
            };
            let namespace = namespace.as_deref().unwrap_or("");
            if element_type_matches(DEFAULT_SCOPE, namespace, name) {
                return false;
            }
        }
        false
    }

    /// The void-element group's shared shape (§13.2.6.4.7:
    /// `area`/`br`/`embed`/`img`/`keygen`/`wbr`, and the `<br>`
    /// end-tag-treated-as-start-tag rewrite): reconstruct the active
    /// formatting elements, insert an HTML element for the token,
    /// immediately pop it back off, and mark the frameset-ok flag "not
    /// ok". Self-closing-flag acknowledgment is a parse-error-
    /// suppression detail this crate has no diagnostics to suppress, so
    /// it's omitted, as elsewhere.
    fn insert_void_element(&mut self, tag: &TagToken, position: Option<Position>) {
        self.reconstruct_the_active_formatting_elements();
        self.insert_html_element(tag, position);
        self.open_elements.pop();
        self.frameset_ok = false;
    }

    /// "Clear the stack back to a table context" (§13.2.6.4.9) and its
    /// "table body"/"table row" siblings (§13.2.6.4.13/.14) all share
    /// this exact shape: pop while the current node's tag name isn't in
    /// `names`. Each of the three real callers passes its own fixed
    /// boundary list.
    fn clear_stack_back_to_context(&mut self, names: &[&str]) {
        while let Some(current) = self.open_elements.current_node() {
            if names
                .iter()
                .any(|&name| self.node_has_html_name(current, name))
            {
                break;
            }
            self.open_elements.pop();
        }
    }

    /// "Clear the stack back to a table context" (§13.2.6.4.9).
    fn clear_stack_back_to_a_table_context(&mut self) {
        self.clear_stack_back_to_context(&["table", "template", "html"]);
    }

    /// "Clear the stack back to a table body context" (§13.2.6.4.13).
    fn clear_stack_back_to_a_table_body_context(&mut self) {
        self.clear_stack_back_to_context(&["tbody", "tfoot", "thead", "template", "html"]);
    }

    /// "Clear the stack back to a table row context" (§13.2.6.4.14).
    fn clear_stack_back_to_a_table_row_context(&mut self) {
        self.clear_stack_back_to_context(&["tr", "template", "html"]);
    }

    /// "Reset the insertion mode appropriately" (§13.2.4.1): walks the
    /// stack of open elements from the current node upward, switching
    /// to whichever insertion mode that node's tag name implies. Used
    /// after popping back out of table-related structure (e.g. a
    /// misnested `<table>` end tag) to figure out where parsing
    /// continues.
    ///
    /// Two branches are deliberately omitted, both per this crate's
    /// scope decisions: the "node is a template element" case (no
    /// template insertion modes stack exists — `<template>` is a plain
    /// element, see plan/03-tree-construction.md — so this just falls
    /// through and keeps walking past it, same as any other
    /// unrecognized element) and the "node is a frameset element" case
    /// (this crate never creates a frameset element at all, so it can
    /// never match). The "fragment case" context-element substitution
    /// never applies either — this crate never does HTML fragment
    /// parsing.
    fn reset_the_insertion_mode_appropriately(&mut self) {
        let mut last = false;
        let mut node = self.open_elements.current_node();
        loop {
            let current = node.expect(
                "the stack of open elements always contains at least html by the time this runs",
            );
            if self.open_elements.topmost() == Some(current) {
                last = true;
            }
            if !last
                && (self.node_has_html_name(current, "td")
                    || self.node_has_html_name(current, "th"))
            {
                self.insertion_mode = InsertionMode::InCell;
                return;
            }
            if self.node_has_html_name(current, "tr") {
                self.insertion_mode = InsertionMode::InRow;
                return;
            }
            if ["tbody", "thead", "tfoot"]
                .iter()
                .any(|&name| self.node_has_html_name(current, name))
            {
                self.insertion_mode = InsertionMode::InTableBody;
                return;
            }
            if self.node_has_html_name(current, "caption") {
                self.insertion_mode = InsertionMode::InCaption;
                return;
            }
            if self.node_has_html_name(current, "colgroup") {
                self.insertion_mode = InsertionMode::InColumnGroup;
                return;
            }
            if self.node_has_html_name(current, "table") {
                self.insertion_mode = InsertionMode::InTable;
                return;
            }
            if !last && self.node_has_html_name(current, "head") {
                self.insertion_mode = InsertionMode::InHead;
                return;
            }
            if self.node_has_html_name(current, "body") {
                self.insertion_mode = InsertionMode::InBody;
                return;
            }
            if self.node_has_html_name(current, "html") {
                self.insertion_mode = if self.head_element_pointer.is_none() {
                    InsertionMode::BeforeHead
                } else {
                    InsertionMode::AfterHead
                };
                return;
            }
            if last {
                self.insertion_mode = InsertionMode::InBody;
                return;
            }
            node = self.open_elements.element_immediately_above(current);
        }
    }

    /// The "in head" insertion mode (§13.2.6.4.4). `<template>` is
    /// deliberately simplified per this crate's scope decision
    /// (plan/03-tree-construction.md): treated as an ordinary element,
    /// not a separate inert content fragment — no active-formatting-
    /// elements marker on open, no template insertion modes stack, no
    /// shadow-root handling. Its end tag still pops through to the
    /// nearest open `template`, generating implied end tags thoroughly
    /// first, but skips clearing the active formatting elements list
    /// (that clear is tied to the marker this simplification never
    /// pushes — clearing without it would wipe out unrelated formatting
    /// state from an enclosing context). Character-encoding sniffing
    /// (`<meta charset>`) and script-execution bookkeeping (parser
    /// document, force-async, already-started, `document.write()`
    /// re-entrancy) are entirely out of scope — this crate never
    /// decodes bytes itself or executes scripts.
    fn process_token_in_head(&mut self, kind: &TokenKind, position: Position) -> TokenOutcome {
        match kind {
            TokenKind::Character(c) if is_whitespace(*c) => {
                self.insert_character(*c, Some(position));
                TokenOutcome::Consumed(None)
            }
            TokenKind::Comment(data) => {
                self.insert_comment(data.clone(), Some(position));
                TokenOutcome::Consumed(None)
            }
            TokenKind::ProcessingInstruction(pi) => {
                self.insert_processing_instruction(
                    pi.target.clone(),
                    pi.data.clone(),
                    Some(position),
                );
                TokenOutcome::Consumed(None)
            }
            TokenKind::Doctype(_) => TokenOutcome::Consumed(None),
            TokenKind::StartTag(tag) if tag.name == "html" => {
                self.merge_attributes_onto_html_element(tag);
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag)
                if matches!(
                    tag.name.as_str(),
                    "base" | "basefont" | "bgsound" | "link" | "meta"
                ) =>
            {
                self.insert_html_element(tag, Some(position));
                self.open_elements.pop();
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag) if tag.name == "title" => {
                let (_, state) = self.generic_text_element_parsing_algorithm(
                    tag,
                    GenericTextElementKind::Rcdata,
                    Some(position),
                );
                TokenOutcome::Consumed(Some(state))
            }
            TokenKind::StartTag(tag) if tag.name == "noscript" => {
                self.insert_html_element(tag, Some(position));
                self.insertion_mode = InsertionMode::InHeadNoscript;
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag) if matches!(tag.name.as_str(), "noframes" | "style") => {
                let (_, state) = self.generic_text_element_parsing_algorithm(
                    tag,
                    GenericTextElementKind::RawText,
                    Some(position),
                );
                TokenOutcome::Consumed(Some(state))
            }
            TokenKind::StartTag(tag) if tag.name == "script" => {
                let (_, state) = self.generic_text_element_parsing_algorithm(
                    tag,
                    GenericTextElementKind::Script,
                    Some(position),
                );
                TokenOutcome::Consumed(Some(state))
            }
            TokenKind::EndTag(tag) if tag.name == "head" => {
                self.open_elements.pop();
                self.insertion_mode = InsertionMode::AfterHead;
                TokenOutcome::Consumed(None)
            }
            TokenKind::EndTag(tag) if matches!(tag.name.as_str(), "body" | "html" | "br") => {
                self.in_head_anything_else()
            }
            TokenKind::StartTag(tag) if tag.name == "template" => {
                self.insert_html_element(tag, Some(position));
                TokenOutcome::Consumed(None)
            }
            TokenKind::EndTag(tag) if tag.name == "template" => {
                if self.has_template_on_stack() {
                    self.open_elements
                        .generate_implied_end_tags_thoroughly(&self.document);
                    self.pop_until_one_of_popped(&["template"]);
                }
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag) if tag.name == "head" => TokenOutcome::Consumed(None),
            TokenKind::EndTag(_) => TokenOutcome::Consumed(None),
            _ => self.in_head_anything_else(),
        }
    }

    /// "In head"'s "anything else" case: pop the head element, switch
    /// to "after head", and reprocess.
    fn in_head_anything_else(&mut self) -> TokenOutcome {
        self.open_elements.pop();
        self.insertion_mode = InsertionMode::AfterHead;
        TokenOutcome::Reprocess
    }

    /// The "in head noscript" insertion mode (§13.2.6.4.5). Only
    /// reachable via "in head"'s `<noscript>` handling, which this
    /// crate always takes with scripting modeled as disabled (this
    /// crate never executes scripts) — matching typical validator
    /// behavior (e.g. `vnu`), the same posture `html-conform` documents
    /// elsewhere as its default.
    fn process_token_in_head_noscript(
        &mut self,
        kind: &TokenKind,
        position: Position,
    ) -> TokenOutcome {
        match kind {
            TokenKind::Doctype(_) => TokenOutcome::Consumed(None),
            TokenKind::StartTag(tag) if tag.name == "html" => {
                self.merge_attributes_onto_html_element(tag);
                TokenOutcome::Consumed(None)
            }
            TokenKind::EndTag(tag) if tag.name == "noscript" => {
                self.open_elements.pop();
                self.insertion_mode = InsertionMode::InHead;
                TokenOutcome::Consumed(None)
            }
            TokenKind::Character(c) if is_whitespace(*c) => {
                self.process_token_in_head(kind, position)
            }
            TokenKind::Comment(_) | TokenKind::ProcessingInstruction(_) => {
                self.process_token_in_head(kind, position)
            }
            TokenKind::StartTag(tag)
                if matches!(
                    tag.name.as_str(),
                    "basefont" | "bgsound" | "link" | "meta" | "noframes" | "style"
                ) =>
            {
                self.process_token_in_head(kind, position)
            }
            TokenKind::EndTag(tag) if tag.name == "br" => self.in_head_noscript_anything_else(),
            TokenKind::StartTag(tag) if matches!(tag.name.as_str(), "head" | "noscript") => {
                TokenOutcome::Consumed(None)
            }
            TokenKind::EndTag(_) => TokenOutcome::Consumed(None),
            _ => self.in_head_noscript_anything_else(),
        }
    }

    /// "In head noscript"'s "anything else" case (and its explicit
    /// `</br>` entry, which acts the same way): pop the `noscript`
    /// element, switch back to "in head", and reprocess.
    fn in_head_noscript_anything_else(&mut self) -> TokenOutcome {
        self.open_elements.pop();
        self.insertion_mode = InsertionMode::InHead;
        TokenOutcome::Reprocess
    }

    /// The "after head" insertion mode (§13.2.6.4.6). The `frameset`
    /// start tag case is intentionally absent: frameset documents are
    /// out of this crate's scope entirely (see the scope decision in
    /// plan/03-tree-construction.md) — falls through to "anything
    /// else" instead of the (unimplemented) "in frameset" mode.
    fn process_token_after_head(&mut self, kind: &TokenKind, position: Position) -> TokenOutcome {
        match kind {
            TokenKind::Character(c) if is_whitespace(*c) => {
                self.insert_character(*c, Some(position));
                TokenOutcome::Consumed(None)
            }
            TokenKind::Comment(data) => {
                self.insert_comment(data.clone(), Some(position));
                TokenOutcome::Consumed(None)
            }
            TokenKind::ProcessingInstruction(pi) => {
                self.insert_processing_instruction(
                    pi.target.clone(),
                    pi.data.clone(),
                    Some(position),
                );
                TokenOutcome::Consumed(None)
            }
            TokenKind::Doctype(_) => TokenOutcome::Consumed(None),
            TokenKind::StartTag(tag) if tag.name == "html" => {
                self.merge_attributes_onto_html_element(tag);
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag) if tag.name == "body" => {
                self.insert_html_element(tag, Some(position));
                self.insertion_mode = InsertionMode::InBody;
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag)
                if matches!(
                    tag.name.as_str(),
                    "base"
                        | "basefont"
                        | "bgsound"
                        | "link"
                        | "meta"
                        | "noframes"
                        | "script"
                        | "style"
                        | "template"
                        | "title"
                ) =>
            {
                let Some(head) = self.head_element_pointer else {
                    unreachable!(
                        "the head element pointer is always set by the time \"after head\" runs"
                    )
                };
                self.open_elements.push(head);
                let outcome = self.process_token_in_head(kind, position);
                let head_index = self
                    .open_elements
                    .entries
                    .iter()
                    .position(|&node| node == head);
                if let Some(head_index) = head_index {
                    self.open_elements.entries.remove(head_index);
                }
                outcome
            }
            TokenKind::EndTag(tag) if tag.name == "template" => {
                self.process_token_in_head(kind, position)
            }
            TokenKind::EndTag(tag) if matches!(tag.name.as_str(), "body" | "html" | "br") => {
                self.after_head_anything_else()
            }
            TokenKind::StartTag(tag) if tag.name == "head" => TokenOutcome::Consumed(None),
            TokenKind::EndTag(_) => TokenOutcome::Consumed(None),
            _ => self.after_head_anything_else(),
        }
    }

    /// "After head"'s "anything else" case: insert a synthesized `body`
    /// start tag (no attributes), switch to "in body", and reprocess.
    fn after_head_anything_else(&mut self) -> TokenOutcome {
        let body_tag = TagToken {
            name: "body".to_owned(),
            self_closing: false,
            attributes: vec![],
        };
        self.insert_html_element(&body_tag, None);
        self.insertion_mode = InsertionMode::InBody;
        TokenOutcome::Reprocess
    }

    /// The "in body" insertion mode (§13.2.6.4.7) — by far the largest
    /// rule set in the whole spec. Transcribed arm by arm from the raw
    /// spec markup (parsed into its actual nested-list structure, not
    /// the flattened prose — flattening lost at least one real
    /// conditional boundary during transcription, e.g. "option"'s
    /// "reconstruct the active formatting elements"/"insert an HTML
    /// element" steps are unconditional, not nested under its
    /// `if`/`else`).
    ///
    /// A few pieces are genuinely out of this crate's scope or deferred
    /// to a separate step, each noted at its arm:
    /// - `<frameset>` never creates a frameset element (frameset
    ///   documents are excluded entirely, see
    ///   plan/03-tree-construction.md's scope decision).
    /// - An end-of-file token's "stop parsing" is a driver-loop concern
    ///   (not yet built).
    /// - `<math>`/`<svg>` insert the foreign-namespaced root element,
    ///   but skip the MathML/SVG/foreign attribute-adjustment tables —
    ///   that's §13.2.6.5's job (Foreign-Content-Dispatch, a separate
    ///   plan/03-tree-construction.md step), as is actually dispatching
    ///   *content inside* a foreign element through different rules.
    /// - `<noscript>` is absent from the raw-text group here (unlike
    ///   "in head"): it only belongs there "if scripting mode is not
    ///   Disabled", and this crate always models scripting as disabled
    ///   (see "in head noscript"'s doc comment) — so a stray
    ///   `<noscript>` in body position falls through to "any other
    ///   start tag" instead, exactly as intended.
    fn process_token_in_body(&mut self, kind: &TokenKind, position: Position) -> TokenOutcome {
        match kind {
            TokenKind::Character(c) => {
                if self.skip_next_line_feed {
                    self.skip_next_line_feed = false;
                    if *c == '\n' {
                        return TokenOutcome::Consumed(None);
                    }
                }
                if *c == '\0' {
                    return TokenOutcome::Consumed(None);
                }
                self.reconstruct_the_active_formatting_elements();
                self.insert_character(*c, Some(position));
                if !is_whitespace(*c) {
                    self.frameset_ok = false;
                }
                TokenOutcome::Consumed(None)
            }
            TokenKind::Comment(data) => {
                self.insert_comment(data.clone(), Some(position));
                TokenOutcome::Consumed(None)
            }
            TokenKind::ProcessingInstruction(pi) => {
                self.insert_processing_instruction(
                    pi.target.clone(),
                    pi.data.clone(),
                    Some(position),
                );
                TokenOutcome::Consumed(None)
            }
            TokenKind::Doctype(_) => TokenOutcome::Consumed(None),
            TokenKind::StartTag(tag) if tag.name == "html" => {
                self.merge_attributes_onto_html_element(tag);
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag)
                if matches!(
                    tag.name.as_str(),
                    "base"
                        | "basefont"
                        | "bgsound"
                        | "link"
                        | "meta"
                        | "noframes"
                        | "script"
                        | "style"
                        | "template"
                        | "title"
                ) =>
            {
                self.process_token_in_head(kind, position)
            }
            TokenKind::EndTag(tag) if tag.name == "template" => {
                self.process_token_in_head(kind, position)
            }
            TokenKind::StartTag(tag) if tag.name == "body" => {
                let second = self.open_elements.entries.get(1).copied();
                let ignore = self.open_elements.entries.len() == 1
                    || second.is_none_or(|node| !self.node_has_html_name(node, "body"))
                    || self.has_template_on_stack();
                if !ignore {
                    self.frameset_ok = false;
                    self.merge_new_attributes_onto(second.expect("checked by `ignore` above"), tag);
                }
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag) if tag.name == "frameset" => {
                // Frameset documents are entirely out of scope (see
                // plan/03-tree-construction.md) — html-conform's schema
                // can never validate one. Rather than half-implement
                // "in frameset" mode, this never creates a frameset
                // element, same as this rule's own several real
                // ignore-conditions would in a full implementation.
                let _ = tag;
                TokenOutcome::Consumed(None)
            }
            TokenKind::Eof => {
                // The "stack of template insertion modes" branch never
                // applies (no such stack — <template> is a plain
                // element, see the scope decision). The parse-error
                // check has no tree-shape effect. "Stop parsing" itself
                // is a driver-loop concern, not yet built.
                TokenOutcome::Consumed(None)
            }
            TokenKind::EndTag(tag) if tag.name == "body" => {
                if !self
                    .open_elements
                    .has_element_in_scope(&self.document, "body")
                {
                    return TokenOutcome::Consumed(None);
                }
                self.insertion_mode = InsertionMode::AfterBody;
                TokenOutcome::Consumed(None)
            }
            TokenKind::EndTag(tag) if tag.name == "html" => {
                if !self
                    .open_elements
                    .has_element_in_scope(&self.document, "body")
                {
                    return TokenOutcome::Consumed(None);
                }
                self.insertion_mode = InsertionMode::AfterBody;
                TokenOutcome::Reprocess
            }
            TokenKind::StartTag(tag)
                if matches!(
                    tag.name.as_str(),
                    "address"
                        | "article"
                        | "aside"
                        | "blockquote"
                        | "center"
                        | "details"
                        | "dialog"
                        | "dir"
                        | "div"
                        | "dl"
                        | "fieldset"
                        | "figcaption"
                        | "figure"
                        | "footer"
                        | "header"
                        | "hgroup"
                        | "main"
                        | "menu"
                        | "nav"
                        | "ol"
                        | "p"
                        | "search"
                        | "section"
                        | "summary"
                        | "ul"
                ) =>
            {
                if self
                    .open_elements
                    .has_element_in_button_scope(&self.document, "p")
                {
                    self.close_a_p_element();
                }
                self.insert_html_element(tag, Some(position));
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag)
                if matches!(tag.name.as_str(), "h1" | "h2" | "h3" | "h4" | "h5" | "h6") =>
            {
                if self
                    .open_elements
                    .has_element_in_button_scope(&self.document, "p")
                {
                    self.close_a_p_element();
                }
                let current_is_heading = self.open_elements.current_node().is_some_and(|node| {
                    ["h1", "h2", "h3", "h4", "h5", "h6"]
                        .iter()
                        .any(|&heading| self.node_has_html_name(node, heading))
                });
                if current_is_heading {
                    self.open_elements.pop();
                }
                self.insert_html_element(tag, Some(position));
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag) if matches!(tag.name.as_str(), "pre" | "listing") => {
                if self
                    .open_elements
                    .has_element_in_button_scope(&self.document, "p")
                {
                    self.close_a_p_element();
                }
                self.insert_html_element(tag, Some(position));
                self.skip_next_line_feed = true;
                self.frameset_ok = false;
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag) if tag.name == "form" => {
                if self.form_element_pointer.is_some() && !self.has_template_on_stack() {
                    return TokenOutcome::Consumed(None);
                }
                if self
                    .open_elements
                    .has_element_in_button_scope(&self.document, "p")
                {
                    self.close_a_p_element();
                }
                let form = self.insert_html_element(tag, Some(position));
                if !self.has_template_on_stack() {
                    self.form_element_pointer = Some(form);
                }
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag) if tag.name == "li" => {
                self.frameset_ok = false;
                let mut node = self.open_elements.current_node();
                while let Some(current) = node {
                    if self.node_has_html_name(current, "li") {
                        self.open_elements
                            .generate_implied_end_tags(&self.document, Some("li"));
                        self.pop_until_one_of_popped(&["li"]);
                        break;
                    }
                    if is_special(&self.document, current)
                        && !["address", "div", "p"]
                            .iter()
                            .any(|&name| self.node_has_html_name(current, name))
                    {
                        break;
                    }
                    node = self.open_elements.element_immediately_above(current);
                }
                if self
                    .open_elements
                    .has_element_in_button_scope(&self.document, "p")
                {
                    self.close_a_p_element();
                }
                self.insert_html_element(tag, Some(position));
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag) if matches!(tag.name.as_str(), "dd" | "dt") => {
                self.frameset_ok = false;
                let mut node = self.open_elements.current_node();
                while let Some(current) = node {
                    if self.node_has_html_name(current, "dd") {
                        self.open_elements
                            .generate_implied_end_tags(&self.document, Some("dd"));
                        self.pop_until_one_of_popped(&["dd"]);
                        break;
                    }
                    if self.node_has_html_name(current, "dt") {
                        self.open_elements
                            .generate_implied_end_tags(&self.document, Some("dt"));
                        self.pop_until_one_of_popped(&["dt"]);
                        break;
                    }
                    if is_special(&self.document, current)
                        && !["address", "div", "p"]
                            .iter()
                            .any(|&name| self.node_has_html_name(current, name))
                    {
                        break;
                    }
                    node = self.open_elements.element_immediately_above(current);
                }
                if self
                    .open_elements
                    .has_element_in_button_scope(&self.document, "p")
                {
                    self.close_a_p_element();
                }
                self.insert_html_element(tag, Some(position));
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag) if tag.name == "plaintext" => {
                if self
                    .open_elements
                    .has_element_in_button_scope(&self.document, "p")
                {
                    self.close_a_p_element();
                }
                self.insert_html_element(tag, Some(position));
                TokenOutcome::Consumed(Some(ExternalState::PlainText))
            }
            TokenKind::StartTag(tag) if tag.name == "button" => {
                if self
                    .open_elements
                    .has_element_in_scope(&self.document, "button")
                {
                    self.open_elements
                        .generate_implied_end_tags(&self.document, None);
                    self.pop_until_one_of_popped(&["button"]);
                }
                self.reconstruct_the_active_formatting_elements();
                self.insert_html_element(tag, Some(position));
                self.frameset_ok = false;
                TokenOutcome::Consumed(None)
            }
            TokenKind::EndTag(tag)
                if matches!(
                    tag.name.as_str(),
                    "address"
                        | "article"
                        | "aside"
                        | "blockquote"
                        | "button"
                        | "center"
                        | "details"
                        | "dialog"
                        | "dir"
                        | "div"
                        | "dl"
                        | "fieldset"
                        | "figcaption"
                        | "figure"
                        | "footer"
                        | "header"
                        | "hgroup"
                        | "listing"
                        | "main"
                        | "menu"
                        | "nav"
                        | "ol"
                        | "pre"
                        | "search"
                        | "section"
                        | "select"
                        | "summary"
                        | "ul"
                ) =>
            {
                if !self
                    .open_elements
                    .has_element_in_scope(&self.document, &tag.name)
                {
                    return TokenOutcome::Consumed(None);
                }
                self.open_elements
                    .generate_implied_end_tags(&self.document, None);
                self.pop_until_one_of_popped(&[tag.name.as_str()]);
                TokenOutcome::Consumed(None)
            }
            TokenKind::EndTag(tag) if tag.name == "form" => {
                if self.has_template_on_stack() {
                    if !self
                        .open_elements
                        .has_element_in_scope(&self.document, "form")
                    {
                        return TokenOutcome::Consumed(None);
                    }
                    self.open_elements
                        .generate_implied_end_tags(&self.document, None);
                    self.pop_until_one_of_popped(&["form"]);
                } else {
                    let Some(node) = self.form_element_pointer.take() else {
                        return TokenOutcome::Consumed(None);
                    };
                    if !self.has_node_in_scope(node) {
                        return TokenOutcome::Consumed(None);
                    }
                    self.open_elements
                        .generate_implied_end_tags(&self.document, None);
                    self.remove_node_from_open_elements(node);
                }
                TokenOutcome::Consumed(None)
            }
            TokenKind::EndTag(tag) if tag.name == "p" => {
                if !self
                    .open_elements
                    .has_element_in_button_scope(&self.document, "p")
                {
                    let p_tag = TagToken {
                        name: "p".to_owned(),
                        self_closing: false,
                        attributes: vec![],
                    };
                    self.insert_html_element(&p_tag, None);
                }
                self.close_a_p_element();
                let _ = tag;
                TokenOutcome::Consumed(None)
            }
            TokenKind::EndTag(tag) if tag.name == "li" => {
                if !self
                    .open_elements
                    .has_element_in_list_item_scope(&self.document, "li")
                {
                    return TokenOutcome::Consumed(None);
                }
                self.open_elements
                    .generate_implied_end_tags(&self.document, Some("li"));
                self.pop_until_one_of_popped(&["li"]);
                TokenOutcome::Consumed(None)
            }
            TokenKind::EndTag(tag) if matches!(tag.name.as_str(), "dd" | "dt") => {
                if !self
                    .open_elements
                    .has_element_in_scope(&self.document, &tag.name)
                {
                    return TokenOutcome::Consumed(None);
                }
                self.open_elements
                    .generate_implied_end_tags(&self.document, Some(tag.name.as_str()));
                self.pop_until_one_of_popped(&[tag.name.as_str()]);
                TokenOutcome::Consumed(None)
            }
            TokenKind::EndTag(tag)
                if matches!(tag.name.as_str(), "h1" | "h2" | "h3" | "h4" | "h5" | "h6") =>
            {
                let headings = ["h1", "h2", "h3", "h4", "h5", "h6"];
                if !headings.iter().any(|&heading| {
                    self.open_elements
                        .has_element_in_scope(&self.document, heading)
                }) {
                    return TokenOutcome::Consumed(None);
                }
                self.open_elements
                    .generate_implied_end_tags(&self.document, None);
                self.pop_until_one_of_popped(&headings);
                let _ = tag;
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag) if tag.name == "a" => {
                let marker_boundary = self
                    .active_formatting_elements
                    .entries
                    .iter()
                    .rposition(|entry| matches!(entry, FormattingEntry::Marker))
                    .map_or(0, |marker_index| marker_index + 1);
                let existing_a = self.active_formatting_elements.entries[marker_boundary..]
                    .iter()
                    .find_map(|entry| match entry {
                        FormattingEntry::Element(node) if self.node_has_html_name(*node, "a") => {
                            Some(*node)
                        }
                        _ => None,
                    });
                if let Some(existing_a) = existing_a {
                    self.adoption_agency_algorithm(tag);
                    self.remove_node_from_active_formatting_elements(existing_a);
                    self.remove_node_from_open_elements(existing_a);
                }
                self.reconstruct_the_active_formatting_elements();
                let element = self.insert_html_element(tag, Some(position));
                self.active_formatting_elements
                    .push(&self.document, element);
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag)
                if matches!(
                    tag.name.as_str(),
                    "b" | "big"
                        | "code"
                        | "em"
                        | "font"
                        | "i"
                        | "s"
                        | "small"
                        | "strike"
                        | "strong"
                        | "tt"
                        | "u"
                ) =>
            {
                self.reconstruct_the_active_formatting_elements();
                let element = self.insert_html_element(tag, Some(position));
                self.active_formatting_elements
                    .push(&self.document, element);
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag) if tag.name == "nobr" => {
                self.reconstruct_the_active_formatting_elements();
                if self
                    .open_elements
                    .has_element_in_scope(&self.document, "nobr")
                {
                    self.adoption_agency_algorithm(tag);
                    self.reconstruct_the_active_formatting_elements();
                }
                let element = self.insert_html_element(tag, Some(position));
                self.active_formatting_elements
                    .push(&self.document, element);
                TokenOutcome::Consumed(None)
            }
            TokenKind::EndTag(tag)
                if matches!(
                    tag.name.as_str(),
                    "a" | "b"
                        | "big"
                        | "code"
                        | "em"
                        | "font"
                        | "i"
                        | "nobr"
                        | "s"
                        | "small"
                        | "strike"
                        | "strong"
                        | "tt"
                        | "u"
                ) =>
            {
                self.adoption_agency_algorithm(tag);
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag)
                if matches!(tag.name.as_str(), "applet" | "marquee" | "object") =>
            {
                self.reconstruct_the_active_formatting_elements();
                self.insert_html_element(tag, Some(position));
                self.active_formatting_elements.push_marker();
                self.frameset_ok = false;
                TokenOutcome::Consumed(None)
            }
            TokenKind::EndTag(tag)
                if matches!(tag.name.as_str(), "applet" | "marquee" | "object") =>
            {
                if !self
                    .open_elements
                    .has_element_in_scope(&self.document, &tag.name)
                {
                    return TokenOutcome::Consumed(None);
                }
                self.open_elements
                    .generate_implied_end_tags(&self.document, None);
                self.pop_until_one_of_popped(&[tag.name.as_str()]);
                self.active_formatting_elements.clear_up_to_last_marker();
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag) if tag.name == "table" => {
                if self.quirks_mode != QuirksMode::Quirks
                    && self
                        .open_elements
                        .has_element_in_button_scope(&self.document, "p")
                {
                    self.close_a_p_element();
                }
                self.insert_html_element(tag, Some(position));
                self.frameset_ok = false;
                self.insertion_mode = InsertionMode::InTable;
                TokenOutcome::Consumed(None)
            }
            TokenKind::EndTag(tag) if tag.name == "br" => {
                let br_tag = TagToken {
                    name: "br".to_owned(),
                    self_closing: false,
                    attributes: vec![],
                };
                self.insert_void_element(&br_tag, Some(position));
                let _ = tag;
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag)
                if matches!(
                    tag.name.as_str(),
                    "area" | "br" | "embed" | "img" | "keygen" | "wbr"
                ) =>
            {
                self.insert_void_element(tag, Some(position));
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag) if tag.name == "input" => {
                // The fragment-case check never applies — this crate
                // never does HTML fragment parsing.
                if self
                    .open_elements
                    .has_element_in_scope(&self.document, "select")
                {
                    self.pop_until_one_of_popped(&["select"]);
                }
                self.reconstruct_the_active_formatting_elements();
                self.insert_html_element(tag, Some(position));
                self.open_elements.pop();
                let is_hidden_type = tag
                    .attributes
                    .iter()
                    .find(|attribute| attribute.name == "type")
                    .is_some_and(|attribute| attribute.value.eq_ignore_ascii_case("hidden"));
                if !is_hidden_type {
                    self.frameset_ok = false;
                }
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag)
                if matches!(tag.name.as_str(), "param" | "source" | "track") =>
            {
                self.insert_html_element(tag, Some(position));
                self.open_elements.pop();
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag) if tag.name == "hr" => {
                if self
                    .open_elements
                    .has_element_in_button_scope(&self.document, "p")
                {
                    self.close_a_p_element();
                }
                if self
                    .open_elements
                    .has_element_in_scope(&self.document, "select")
                {
                    self.open_elements
                        .generate_implied_end_tags(&self.document, None);
                    // parse-error-only checks (option/optgroup in
                    // scope) omitted, no tree-shape effect.
                }
                self.insert_html_element(tag, Some(position));
                self.open_elements.pop();
                self.frameset_ok = false;
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag) if tag.name == "image" => {
                let img_tag = TagToken {
                    name: "img".to_owned(),
                    ..tag.clone()
                };
                self.process_token_in_body(&TokenKind::StartTag(img_tag), position)
            }
            TokenKind::StartTag(tag) if tag.name == "textarea" => {
                let (_, state) = self.generic_text_element_parsing_algorithm(
                    tag,
                    GenericTextElementKind::Rcdata,
                    Some(position),
                );
                self.skip_next_line_feed = true;
                self.frameset_ok = false;
                TokenOutcome::Consumed(Some(state))
            }
            TokenKind::StartTag(tag) if tag.name == "xmp" => {
                if self
                    .open_elements
                    .has_element_in_button_scope(&self.document, "p")
                {
                    self.close_a_p_element();
                }
                self.reconstruct_the_active_formatting_elements();
                self.frameset_ok = false;
                let (_, state) = self.generic_text_element_parsing_algorithm(
                    tag,
                    GenericTextElementKind::RawText,
                    Some(position),
                );
                TokenOutcome::Consumed(Some(state))
            }
            TokenKind::StartTag(tag) if tag.name == "iframe" => {
                self.frameset_ok = false;
                let (_, state) = self.generic_text_element_parsing_algorithm(
                    tag,
                    GenericTextElementKind::RawText,
                    Some(position),
                );
                TokenOutcome::Consumed(Some(state))
            }
            TokenKind::StartTag(tag) if tag.name == "noembed" => {
                let (_, state) = self.generic_text_element_parsing_algorithm(
                    tag,
                    GenericTextElementKind::RawText,
                    Some(position),
                );
                TokenOutcome::Consumed(Some(state))
            }
            TokenKind::StartTag(tag) if tag.name == "select" => {
                // The fragment-case check never applies.
                if self
                    .open_elements
                    .has_element_in_scope(&self.document, "select")
                {
                    self.pop_until_one_of_popped(&["select"]);
                } else {
                    self.reconstruct_the_active_formatting_elements();
                    self.insert_html_element(tag, Some(position));
                    self.frameset_ok = false;
                }
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag) if tag.name == "option" => {
                if self
                    .open_elements
                    .has_element_in_scope(&self.document, "select")
                {
                    self.open_elements
                        .generate_implied_end_tags(&self.document, Some("optgroup"));
                } else if self
                    .open_elements
                    .current_node()
                    .is_some_and(|node| self.node_has_html_name(node, "option"))
                {
                    self.open_elements.pop();
                }
                self.reconstruct_the_active_formatting_elements();
                self.insert_html_element(tag, Some(position));
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag) if tag.name == "optgroup" => {
                if self
                    .open_elements
                    .has_element_in_scope(&self.document, "select")
                {
                    self.open_elements
                        .generate_implied_end_tags(&self.document, None);
                } else if self
                    .open_elements
                    .current_node()
                    .is_some_and(|node| self.node_has_html_name(node, "option"))
                {
                    self.open_elements.pop();
                }
                self.reconstruct_the_active_formatting_elements();
                self.insert_html_element(tag, Some(position));
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag) if matches!(tag.name.as_str(), "rb" | "rtc") => {
                if self
                    .open_elements
                    .has_element_in_scope(&self.document, "ruby")
                {
                    self.open_elements
                        .generate_implied_end_tags(&self.document, None);
                }
                self.insert_html_element(tag, Some(position));
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag) if matches!(tag.name.as_str(), "rp" | "rt") => {
                if self
                    .open_elements
                    .has_element_in_scope(&self.document, "ruby")
                {
                    self.open_elements
                        .generate_implied_end_tags(&self.document, Some("rtc"));
                }
                self.insert_html_element(tag, Some(position));
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag) if tag.name == "math" => {
                self.reconstruct_the_active_formatting_elements();
                self.insert_foreign_element(tag, MATHML_NAMESPACE, false, Some(position));
                if tag.self_closing {
                    self.open_elements.pop();
                }
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag) if tag.name == "svg" => {
                self.reconstruct_the_active_formatting_elements();
                self.insert_foreign_element(tag, SVG_NAMESPACE, false, Some(position));
                if tag.self_closing {
                    self.open_elements.pop();
                }
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag)
                if matches!(
                    tag.name.as_str(),
                    "caption"
                        | "col"
                        | "colgroup"
                        | "frame"
                        | "head"
                        | "tbody"
                        | "td"
                        | "tfoot"
                        | "th"
                        | "thead"
                        | "tr"
                ) =>
            {
                let _ = tag;
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag) => {
                self.reconstruct_the_active_formatting_elements();
                self.insert_html_element(tag, Some(position));
                TokenOutcome::Consumed(None)
            }
            TokenKind::EndTag(tag) => {
                self.any_other_end_tag_in_body(tag);
                TokenOutcome::Consumed(None)
            }
        }
    }

    /// The "text" insertion mode (§13.2.6.4.8) — the counterpart to
    /// [`generic_text_element_parsing_algorithm`](Self::generic_text_element_parsing_algorithm),
    /// which is the only way this crate ever enters it (RCDATA/RAWTEXT/
    /// script data; `<plaintext>` deliberately never switches to this
    /// mode at all, see "in body"'s own `plaintext` arm).
    fn process_token_text(&mut self, kind: &TokenKind, position: Position) -> TokenOutcome {
        match kind {
            TokenKind::Character(c) => {
                self.insert_character(*c, Some(position));
                TokenOutcome::Consumed(None)
            }
            TokenKind::Eof => {
                // The parse-error check and "if current node is a
                // script element, set its already started flag" have
                // no tree-shape effect (no diagnostics, no script
                // execution in this crate).
                self.open_elements.pop();
                self.insertion_mode = self.original_insertion_mode.take().expect(
                    "\"text\" mode is only entered via \
                     generic_text_element_parsing_algorithm, which always records \
                     original_insertion_mode first",
                );
                TokenOutcome::Reprocess
            }
            TokenKind::EndTag(_) => {
                // Both "an end tag whose tag name is 'script'" and "any
                // other end tag" reduce to the same tree-relevant steps
                // once script-execution machinery (microtask
                // checkpoints, the speculative parser, document.write()
                // re-entrancy, script nesting level) is stripped out —
                // this crate never executes scripts.
                self.open_elements.pop();
                self.insertion_mode = self.original_insertion_mode.take().expect(
                    "\"text\" mode is only entered via \
                     generic_text_element_parsing_algorithm, which always records \
                     original_insertion_mode first",
                );
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(_)
            | TokenKind::Comment(_)
            | TokenKind::ProcessingInstruction(_)
            | TokenKind::Doctype(_) => unreachable!(
                "the tokenizer's RCDATA/RAWTEXT/script-data states only ever produce \
                 character and end-tag tokens (plus EOF) — never start tags, comments, \
                 processing instructions, or DOCTYPEs"
            ),
        }
    }

    /// "In table"'s "anything else" case (§13.2.6.4.9): enable foster
    /// parenting, process the token using "in body"'s rules, then
    /// disable foster parenting again.
    fn in_table_anything_else(&mut self, kind: &TokenKind, position: Position) -> TokenOutcome {
        self.foster_parenting = true;
        let outcome = self.process_token_in_body(kind, position);
        self.foster_parenting = false;
        outcome
    }

    /// The "in table" insertion mode (§13.2.6.4.9).
    fn process_token_in_table(&mut self, kind: &TokenKind, position: Position) -> TokenOutcome {
        match kind {
            TokenKind::Character(_)
                if self.open_elements.current_node().is_some_and(|node| {
                    ["table", "tbody", "template", "tfoot", "thead", "tr"]
                        .iter()
                        .any(|&name| self.node_has_html_name(node, name))
                }) =>
            {
                self.pending_table_character_tokens.clear();
                self.original_insertion_mode = Some(self.insertion_mode);
                self.insertion_mode = InsertionMode::InTableText;
                TokenOutcome::Reprocess
            }
            TokenKind::Comment(data) => {
                self.insert_comment(data.clone(), Some(position));
                TokenOutcome::Consumed(None)
            }
            TokenKind::ProcessingInstruction(pi) => {
                self.insert_processing_instruction(
                    pi.target.clone(),
                    pi.data.clone(),
                    Some(position),
                );
                TokenOutcome::Consumed(None)
            }
            TokenKind::Doctype(_) => TokenOutcome::Consumed(None),
            TokenKind::StartTag(tag) if tag.name == "caption" => {
                self.clear_stack_back_to_a_table_context();
                self.active_formatting_elements.push_marker();
                self.insert_html_element(tag, Some(position));
                self.insertion_mode = InsertionMode::InCaption;
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag) if tag.name == "colgroup" => {
                self.clear_stack_back_to_a_table_context();
                self.insert_html_element(tag, Some(position));
                self.insertion_mode = InsertionMode::InColumnGroup;
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag) if tag.name == "col" => {
                self.clear_stack_back_to_a_table_context();
                let colgroup_tag = TagToken {
                    name: "colgroup".to_owned(),
                    self_closing: false,
                    attributes: vec![],
                };
                self.insert_html_element(&colgroup_tag, None);
                self.insertion_mode = InsertionMode::InColumnGroup;
                TokenOutcome::Reprocess
            }
            TokenKind::StartTag(tag)
                if matches!(tag.name.as_str(), "tbody" | "tfoot" | "thead") =>
            {
                self.clear_stack_back_to_a_table_context();
                self.insert_html_element(tag, Some(position));
                self.insertion_mode = InsertionMode::InTableBody;
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag) if matches!(tag.name.as_str(), "td" | "th" | "tr") => {
                self.clear_stack_back_to_a_table_context();
                let tbody_tag = TagToken {
                    name: "tbody".to_owned(),
                    self_closing: false,
                    attributes: vec![],
                };
                self.insert_html_element(&tbody_tag, None);
                self.insertion_mode = InsertionMode::InTableBody;
                TokenOutcome::Reprocess
            }
            TokenKind::StartTag(tag) if tag.name == "table" => {
                if !self
                    .open_elements
                    .has_element_in_table_scope(&self.document, "table")
                {
                    return TokenOutcome::Consumed(None);
                }
                self.pop_until_one_of_popped(&["table"]);
                self.reset_the_insertion_mode_appropriately();
                TokenOutcome::Reprocess
            }
            TokenKind::EndTag(tag) if tag.name == "table" => {
                if !self
                    .open_elements
                    .has_element_in_table_scope(&self.document, "table")
                {
                    return TokenOutcome::Consumed(None);
                }
                self.pop_until_one_of_popped(&["table"]);
                self.reset_the_insertion_mode_appropriately();
                TokenOutcome::Consumed(None)
            }
            TokenKind::EndTag(tag)
                if matches!(
                    tag.name.as_str(),
                    "body"
                        | "caption"
                        | "col"
                        | "colgroup"
                        | "html"
                        | "tbody"
                        | "td"
                        | "tfoot"
                        | "th"
                        | "thead"
                        | "tr"
                ) =>
            {
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag)
                if matches!(tag.name.as_str(), "style" | "script" | "template") =>
            {
                self.process_token_in_head(kind, position)
            }
            TokenKind::EndTag(tag) if tag.name == "template" => {
                self.process_token_in_head(kind, position)
            }
            TokenKind::StartTag(tag) if tag.name == "input" => {
                let is_hidden_type = tag
                    .attributes
                    .iter()
                    .find(|attribute| attribute.name == "type")
                    .is_some_and(|attribute| attribute.value.eq_ignore_ascii_case("hidden"));
                if !is_hidden_type {
                    return self.in_table_anything_else(kind, position);
                }
                self.insert_html_element(tag, Some(position));
                self.open_elements.pop();
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag) if tag.name == "form" => {
                if self.has_template_on_stack() || self.form_element_pointer.is_some() {
                    return TokenOutcome::Consumed(None);
                }
                let form = self.insert_html_element(tag, Some(position));
                self.form_element_pointer = Some(form);
                self.open_elements.pop();
                TokenOutcome::Consumed(None)
            }
            TokenKind::Eof => self.process_token_in_body(kind, position),
            _ => self.in_table_anything_else(kind, position),
        }
    }

    /// The "in table text" insertion mode (§13.2.6.4.10) — only ever
    /// entered from "in table"'s own character-token rule, buffering
    /// characters until a non-character token arrives to decide whether
    /// they're plain whitespace or need foster parenting.
    fn process_token_in_table_text(
        &mut self,
        kind: &TokenKind,
        position: Position,
    ) -> TokenOutcome {
        match kind {
            TokenKind::Character(c) if *c == '\0' => TokenOutcome::Consumed(None),
            TokenKind::Character(c) => {
                self.pending_table_character_tokens.push((*c, position));
                TokenOutcome::Consumed(None)
            }
            _ => {
                let tokens = std::mem::take(&mut self.pending_table_character_tokens);
                let has_non_whitespace = tokens.iter().any(|&(c, _)| !is_whitespace(c));
                if has_non_whitespace {
                    for &(c, char_position) in &tokens {
                        self.in_table_anything_else(&TokenKind::Character(c), char_position);
                    }
                } else {
                    for &(c, char_position) in &tokens {
                        self.insert_character(c, Some(char_position));
                    }
                }
                self.insertion_mode = self.original_insertion_mode.take().expect(
                    "\"in table text\" is only entered from \"in table\", which always \
                     records original_insertion_mode first",
                );
                TokenOutcome::Reprocess
            }
        }
    }

    /// "Close the caption" — not a named spec algorithm, but the four
    /// steps shared by "in caption"'s (§13.2.6.4.11) `</caption>` rule
    /// and its "close it implicitly and reprocess" group (the start-tag
    /// table-structure group and `</table>`). Returns whether it
    /// actually closed one (false only when `caption` isn't in table
    /// scope — every one of this helper's real call sites treats that
    /// the same way: ignore the token, don't reprocess).
    fn close_caption(&mut self) -> bool {
        if !self
            .open_elements
            .has_element_in_table_scope(&self.document, "caption")
        {
            return false;
        }
        self.open_elements
            .generate_implied_end_tags(&self.document, None);
        self.pop_until_one_of_popped(&["caption"]);
        self.active_formatting_elements.clear_up_to_last_marker();
        self.insertion_mode = InsertionMode::InTable;
        true
    }

    /// The "in caption" insertion mode (§13.2.6.4.11).
    fn process_token_in_caption(&mut self, kind: &TokenKind, position: Position) -> TokenOutcome {
        match kind {
            TokenKind::EndTag(tag) if tag.name == "caption" => {
                self.close_caption();
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag)
                if matches!(
                    tag.name.as_str(),
                    "caption"
                        | "col"
                        | "colgroup"
                        | "tbody"
                        | "td"
                        | "tfoot"
                        | "th"
                        | "thead"
                        | "tr"
                ) =>
            {
                if self.close_caption() {
                    TokenOutcome::Reprocess
                } else {
                    TokenOutcome::Consumed(None)
                }
            }
            TokenKind::EndTag(tag) if tag.name == "table" => {
                if self.close_caption() {
                    TokenOutcome::Reprocess
                } else {
                    TokenOutcome::Consumed(None)
                }
            }
            TokenKind::EndTag(tag)
                if matches!(
                    tag.name.as_str(),
                    "body"
                        | "col"
                        | "colgroup"
                        | "html"
                        | "tbody"
                        | "td"
                        | "tfoot"
                        | "th"
                        | "thead"
                        | "tr"
                ) =>
            {
                TokenOutcome::Consumed(None)
            }
            _ => self.process_token_in_body(kind, position),
        }
    }

    /// The "in column group" insertion mode (§13.2.6.4.12).
    fn process_token_in_column_group(
        &mut self,
        kind: &TokenKind,
        position: Position,
    ) -> TokenOutcome {
        match kind {
            TokenKind::Character(c) if is_whitespace(*c) => {
                self.insert_character(*c, Some(position));
                TokenOutcome::Consumed(None)
            }
            TokenKind::Comment(data) => {
                self.insert_comment(data.clone(), Some(position));
                TokenOutcome::Consumed(None)
            }
            TokenKind::ProcessingInstruction(pi) => {
                self.insert_processing_instruction(
                    pi.target.clone(),
                    pi.data.clone(),
                    Some(position),
                );
                TokenOutcome::Consumed(None)
            }
            TokenKind::Doctype(_) => TokenOutcome::Consumed(None),
            TokenKind::StartTag(tag) if tag.name == "html" => {
                self.process_token_in_body(kind, position)
            }
            TokenKind::StartTag(tag) if tag.name == "col" => {
                self.insert_html_element(tag, Some(position));
                self.open_elements.pop();
                TokenOutcome::Consumed(None)
            }
            TokenKind::EndTag(tag) if tag.name == "colgroup" => {
                if !self
                    .open_elements
                    .current_node()
                    .is_some_and(|node| self.node_has_html_name(node, "colgroup"))
                {
                    return TokenOutcome::Consumed(None);
                }
                self.open_elements.pop();
                self.insertion_mode = InsertionMode::InTable;
                TokenOutcome::Consumed(None)
            }
            TokenKind::EndTag(tag) if tag.name == "col" => TokenOutcome::Consumed(None),
            TokenKind::StartTag(tag) if tag.name == "template" => {
                self.process_token_in_head(kind, position)
            }
            TokenKind::EndTag(tag) if tag.name == "template" => {
                self.process_token_in_head(kind, position)
            }
            TokenKind::Eof => self.process_token_in_body(kind, position),
            _ => {
                if !self
                    .open_elements
                    .current_node()
                    .is_some_and(|node| self.node_has_html_name(node, "colgroup"))
                {
                    return TokenOutcome::Consumed(None);
                }
                self.open_elements.pop();
                self.insertion_mode = InsertionMode::InTable;
                TokenOutcome::Reprocess
            }
        }
    }

    /// "Close the table body" — not a named spec algorithm, but the
    /// steps shared by "in table body"'s (§13.2.6.4.13) `</tbody>`/
    /// `</thead>`/`</tfoot>` rule and its "close it implicitly and
    /// reprocess" group. Returns whether it actually closed one (false
    /// only when none of tbody/thead/tfoot is in table scope).
    fn close_table_body_and_reprocess(&mut self) -> TokenOutcome {
        let in_scope = ["tbody", "thead", "tfoot"].iter().any(|&name| {
            self.open_elements
                .has_element_in_table_scope(&self.document, name)
        });
        if !in_scope {
            return TokenOutcome::Consumed(None);
        }
        self.clear_stack_back_to_a_table_body_context();
        self.open_elements.pop();
        self.insertion_mode = InsertionMode::InTable;
        TokenOutcome::Reprocess
    }

    /// The "in table body" insertion mode (§13.2.6.4.13).
    fn process_token_in_table_body(
        &mut self,
        kind: &TokenKind,
        position: Position,
    ) -> TokenOutcome {
        match kind {
            TokenKind::StartTag(tag) if tag.name == "tr" => {
                self.clear_stack_back_to_a_table_body_context();
                self.insert_html_element(tag, Some(position));
                self.insertion_mode = InsertionMode::InRow;
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag) if matches!(tag.name.as_str(), "th" | "td") => {
                self.clear_stack_back_to_a_table_body_context();
                let tr_tag = TagToken {
                    name: "tr".to_owned(),
                    self_closing: false,
                    attributes: vec![],
                };
                self.insert_html_element(&tr_tag, None);
                self.insertion_mode = InsertionMode::InRow;
                TokenOutcome::Reprocess
            }
            TokenKind::EndTag(tag) if matches!(tag.name.as_str(), "tbody" | "tfoot" | "thead") => {
                if !self
                    .open_elements
                    .has_element_in_table_scope(&self.document, &tag.name)
                {
                    return TokenOutcome::Consumed(None);
                }
                self.clear_stack_back_to_a_table_body_context();
                self.open_elements.pop();
                self.insertion_mode = InsertionMode::InTable;
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag)
                if matches!(
                    tag.name.as_str(),
                    "caption" | "col" | "colgroup" | "tbody" | "tfoot" | "thead"
                ) =>
            {
                self.close_table_body_and_reprocess()
            }
            TokenKind::EndTag(tag) if tag.name == "table" => self.close_table_body_and_reprocess(),
            TokenKind::EndTag(tag)
                if matches!(
                    tag.name.as_str(),
                    "body" | "caption" | "col" | "colgroup" | "html" | "td" | "th" | "tr"
                ) =>
            {
                TokenOutcome::Consumed(None)
            }
            _ => self.process_token_in_table(kind, position),
        }
    }

    /// "Close the row" — not a named spec algorithm, but the steps
    /// shared by "in row"'s (§13.2.6.4.14) `</tr>` rule and its "close
    /// it implicitly and reprocess" group. Returns whether it actually
    /// closed one (false only when `tr` isn't in table scope).
    fn close_row_and_reprocess(&mut self) -> TokenOutcome {
        if !self
            .open_elements
            .has_element_in_table_scope(&self.document, "tr")
        {
            return TokenOutcome::Consumed(None);
        }
        self.clear_stack_back_to_a_table_row_context();
        self.open_elements.pop();
        self.insertion_mode = InsertionMode::InTableBody;
        TokenOutcome::Reprocess
    }

    /// The "in row" insertion mode (§13.2.6.4.14).
    fn process_token_in_row(&mut self, kind: &TokenKind, position: Position) -> TokenOutcome {
        match kind {
            TokenKind::StartTag(tag) if matches!(tag.name.as_str(), "th" | "td") => {
                self.clear_stack_back_to_a_table_row_context();
                self.insert_html_element(tag, Some(position));
                self.insertion_mode = InsertionMode::InCell;
                self.active_formatting_elements.push_marker();
                TokenOutcome::Consumed(None)
            }
            TokenKind::EndTag(tag) if tag.name == "tr" => {
                if !self
                    .open_elements
                    .has_element_in_table_scope(&self.document, "tr")
                {
                    return TokenOutcome::Consumed(None);
                }
                self.clear_stack_back_to_a_table_row_context();
                self.open_elements.pop();
                self.insertion_mode = InsertionMode::InTableBody;
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag)
                if matches!(
                    tag.name.as_str(),
                    "caption" | "col" | "colgroup" | "tbody" | "tfoot" | "thead" | "tr"
                ) =>
            {
                self.close_row_and_reprocess()
            }
            TokenKind::EndTag(tag) if tag.name == "table" => self.close_row_and_reprocess(),
            TokenKind::EndTag(tag) if matches!(tag.name.as_str(), "tbody" | "tfoot" | "thead") => {
                if !self
                    .open_elements
                    .has_element_in_table_scope(&self.document, &tag.name)
                {
                    return TokenOutcome::Consumed(None);
                }
                self.close_row_and_reprocess()
            }
            TokenKind::EndTag(tag)
                if matches!(
                    tag.name.as_str(),
                    "body" | "caption" | "col" | "colgroup" | "html" | "td" | "th"
                ) =>
            {
                TokenOutcome::Consumed(None)
            }
            _ => self.process_token_in_table(kind, position),
        }
    }

    /// "Close the cell" (§13.2.6.4.15).
    fn close_the_cell(&mut self) {
        self.open_elements
            .generate_implied_end_tags(&self.document, None);
        self.pop_until_one_of_popped(&["td", "th"]);
        self.active_formatting_elements.clear_up_to_last_marker();
        self.insertion_mode = InsertionMode::InRow;
    }

    /// The "in cell" insertion mode (§13.2.6.4.15).
    fn process_token_in_cell(&mut self, kind: &TokenKind, position: Position) -> TokenOutcome {
        match kind {
            TokenKind::EndTag(tag) if matches!(tag.name.as_str(), "td" | "th") => {
                if !self
                    .open_elements
                    .has_element_in_table_scope(&self.document, &tag.name)
                {
                    return TokenOutcome::Consumed(None);
                }
                self.open_elements
                    .generate_implied_end_tags(&self.document, None);
                self.pop_until_one_of_popped(&[tag.name.as_str()]);
                self.active_formatting_elements.clear_up_to_last_marker();
                self.insertion_mode = InsertionMode::InRow;
                TokenOutcome::Consumed(None)
            }
            TokenKind::StartTag(tag)
                if matches!(
                    tag.name.as_str(),
                    "caption"
                        | "col"
                        | "colgroup"
                        | "tbody"
                        | "td"
                        | "tfoot"
                        | "th"
                        | "thead"
                        | "tr"
                ) =>
            {
                self.close_the_cell();
                TokenOutcome::Reprocess
            }
            TokenKind::EndTag(tag)
                if matches!(
                    tag.name.as_str(),
                    "body" | "caption" | "col" | "colgroup" | "html"
                ) =>
            {
                TokenOutcome::Consumed(None)
            }
            TokenKind::EndTag(tag)
                if matches!(
                    tag.name.as_str(),
                    "table" | "tbody" | "tfoot" | "thead" | "tr"
                ) =>
            {
                if !self
                    .open_elements
                    .has_element_in_table_scope(&self.document, &tag.name)
                {
                    return TokenOutcome::Consumed(None);
                }
                self.close_the_cell();
                TokenOutcome::Reprocess
            }
            _ => self.process_token_in_body(kind, position),
        }
    }

    /// The "after body" insertion mode (§13.2.6.4.17).
    fn process_token_after_body(&mut self, kind: &TokenKind, position: Position) -> TokenOutcome {
        match kind {
            TokenKind::Character(c) if is_whitespace(*c) => {
                self.process_token_in_body(kind, position)
            }
            TokenKind::Comment(data) => {
                self.append_comment_to_html_element(data.clone(), position);
                TokenOutcome::Consumed(None)
            }
            TokenKind::ProcessingInstruction(pi) => {
                self.append_processing_instruction_to_html_element(
                    pi.target.clone(),
                    pi.data.clone(),
                    position,
                );
                TokenOutcome::Consumed(None)
            }
            TokenKind::Doctype(_) => TokenOutcome::Consumed(None),
            TokenKind::StartTag(tag) if tag.name == "html" => {
                self.process_token_in_body(kind, position)
            }
            TokenKind::EndTag(tag) if tag.name == "html" => {
                // The fragment-case check never applies — this crate
                // never does HTML fragment parsing.
                let _ = tag;
                self.insertion_mode = InsertionMode::AfterAfterBody;
                TokenOutcome::Consumed(None)
            }
            TokenKind::Eof => {
                // "Stop parsing" is a driver-loop concern, not yet
                // built — see plan/03-tree-construction.md.
                TokenOutcome::Consumed(None)
            }
            _ => {
                self.insertion_mode = InsertionMode::InBody;
                TokenOutcome::Reprocess
            }
        }
    }

    /// The "after after body" insertion mode (§13.2.6.4.20) — the very
    /// last insertion mode this crate implements (see the scope
    /// decision excluding the frameset-related modes,
    /// plan/03-tree-construction.md).
    fn process_token_after_after_body(
        &mut self,
        kind: &TokenKind,
        position: Position,
    ) -> TokenOutcome {
        match kind {
            TokenKind::Comment(data) => {
                self.append_comment_to_document(data.clone(), position);
                TokenOutcome::Consumed(None)
            }
            TokenKind::ProcessingInstruction(pi) => {
                self.append_processing_instruction_to_document(
                    pi.target.clone(),
                    pi.data.clone(),
                    position,
                );
                TokenOutcome::Consumed(None)
            }
            TokenKind::Doctype(_) => self.process_token_in_body(kind, position),
            TokenKind::Character(c) if is_whitespace(*c) => {
                self.process_token_in_body(kind, position)
            }
            TokenKind::StartTag(tag) if tag.name == "html" => {
                self.process_token_in_body(kind, position)
            }
            TokenKind::Eof => TokenOutcome::Consumed(None),
            _ => {
                self.insertion_mode = InsertionMode::InBody;
                TokenOutcome::Reprocess
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{HTML_NAMESPACE, OpenElementsStack};
    use crate::document::{Document, NodeKind};

    /// Builds a stack containing one HTML-namespace element per given
    /// name, pushed in order (so the last name is the current node).
    fn stack_of(document: &mut Document, names: &[&str]) -> OpenElementsStack {
        let mut stack = OpenElementsStack::new();
        for &name in names {
            let node = document.new_node(
                NodeKind::Element {
                    name: name.to_owned(),
                    namespace: Some(HTML_NAMESPACE.to_owned()),
                    attributes: vec![],
                },
                None,
            );
            stack.push(node);
        }
        stack
    }

    #[test]
    fn empty_stack_has_no_current_node() {
        let stack = OpenElementsStack::new();
        assert_eq!(stack.current_node(), None);
    }

    #[test]
    fn push_and_pop_and_current_node() {
        let mut document = Document::new();
        let mut stack = stack_of(&mut document, &["html", "body"]);
        let body = stack.current_node().unwrap();
        assert_eq!(
            document.node(body).kind,
            NodeKind::Element {
                name: "body".to_owned(),
                namespace: Some(HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
        stack.pop();
        let html = stack.current_node().unwrap();
        assert_eq!(
            document.node(html).kind,
            NodeKind::Element {
                name: "html".to_owned(),
                namespace: Some(HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
    }

    #[test]
    fn has_element_in_scope_finds_a_matching_ancestor() {
        let mut document = Document::new();
        let stack = stack_of(&mut document, &["html", "body", "div", "p"]);
        assert!(stack.has_element_in_scope(&document, "div"));
        assert!(stack.has_element_in_scope(&document, "p"));
        assert!(!stack.has_element_in_scope(&document, "span"));
    }

    #[test]
    fn has_element_in_scope_stops_at_a_boundary_element() {
        // A `p` above a `table` is out of scope from inside the table,
        // per the default scope's boundary list including `table` — and
        // `td` itself is *also* a boundary element, so the search for
        // `table` from inside `td` stops at `td` before ever reaching it.
        let mut document = Document::new();
        let stack = stack_of(&mut document, &["html", "body", "p", "table", "tr", "td"]);
        assert!(!stack.has_element_in_scope(&document, "p"));
        assert!(!stack.has_element_in_scope(&document, "table"));
        // `td` matches immediately since it's the current node itself —
        // a node matching the target is checked *before* the boundary
        // check, per spec step order.
        assert!(stack.has_element_in_scope(&document, "td"));
    }

    #[test]
    fn has_element_in_list_item_scope_stops_at_ul_but_default_scope_does_not() {
        // `body` sits above a `ul`; list item scope's boundary list
        // includes `ul`/`ol` (on top of the default scope list), default
        // scope's does not.
        let mut document = Document::new();
        let stack = stack_of(&mut document, &["html", "body", "ul", "li", "span"]);
        assert!(!stack.has_element_in_list_item_scope(&document, "body"));
        assert!(stack.has_element_in_scope(&document, "body"));
    }

    #[test]
    fn has_element_in_button_scope_stops_at_button() {
        let mut document = Document::new();
        let stack = stack_of(&mut document, &["html", "body", "p", "button"]);
        assert!(!stack.has_element_in_button_scope(&document, "p"));
        assert!(stack.has_element_in_scope(&document, "p"));
    }

    #[test]
    fn has_element_in_table_scope_uses_its_own_short_boundary_list() {
        // `td` is a boundary element for default scope but *not* for
        // table scope (table scope's boundary list is just
        // html/table/template) — so table scope can see past it to the
        // enclosing `table`, while default scope cannot.
        let mut document = Document::new();
        let stack = stack_of(&mut document, &["html", "table", "td", "div"]);
        assert!(stack.has_element_in_table_scope(&document, "table"));
        assert!(!stack.has_element_in_scope(&document, "table"));
    }

    #[test]
    fn generate_implied_end_tags_pops_while_current_node_is_in_the_list() {
        let mut document = Document::new();
        let mut stack = stack_of(&mut document, &["html", "body", "ul", "li", "p"]);
        stack.generate_implied_end_tags(&document, None);
        // `p` and `li` are both in the list, `ul` is not: popping stops
        // there and `ul` remains the current node.
        let current = stack.current_node().unwrap();
        assert_eq!(
            document.node(current).kind,
            NodeKind::Element {
                name: "ul".to_owned(),
                namespace: Some(HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
    }

    #[test]
    fn generate_implied_end_tags_excludes_the_given_element() {
        let mut document = Document::new();
        let mut stack = stack_of(&mut document, &["html", "body", "dl", "dt", "li"]);
        // `li` is in the list but excluded, so popping stops immediately
        // without popping anything.
        stack.generate_implied_end_tags(&document, Some("li"));
        let current = stack.current_node().unwrap();
        assert_eq!(
            document.node(current).kind,
            NodeKind::Element {
                name: "li".to_owned(),
                namespace: Some(HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
    }

    #[test]
    fn generate_implied_end_tags_thoroughly_pops_table_elements_too() {
        let mut document = Document::new();
        let mut stack = stack_of(&mut document, &["html", "table", "tbody", "tr", "td"]);
        // `td` is only in the "thoroughly" list, not the plain list.
        stack.generate_implied_end_tags(&document, None);
        assert_eq!(stack.entries.len(), 5);
        stack.generate_implied_end_tags_thoroughly(&document);
        let current = stack.current_node().unwrap();
        assert_eq!(
            document.node(current).kind,
            NodeKind::Element {
                name: "table".to_owned(),
                namespace: Some(HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
    }
}

#[cfg(test)]
mod quirks_mode_tests {
    use super::{QuirksMode, determine_quirks_mode};
    use crate::tokenizer::DoctypeToken;

    fn doctype(
        name: Option<&str>,
        public_identifier: Option<&str>,
        system_identifier: Option<&str>,
        force_quirks: bool,
    ) -> DoctypeToken {
        DoctypeToken {
            name: name.map(str::to_owned),
            public_identifier: public_identifier.map(str::to_owned),
            system_identifier: system_identifier.map(str::to_owned),
            force_quirks,
        }
    }

    #[test]
    fn html5_doctype_is_no_quirks() {
        let token = doctype(Some("html"), None, None, false);
        assert_eq!(determine_quirks_mode(&token), QuirksMode::NoQuirks);
    }

    #[test]
    fn force_quirks_flag_wins_regardless_of_everything_else() {
        let token = doctype(Some("html"), None, None, true);
        assert_eq!(determine_quirks_mode(&token), QuirksMode::Quirks);
    }

    #[test]
    fn missing_doctype_name_is_quirks() {
        let token = doctype(None, None, None, false);
        assert_eq!(determine_quirks_mode(&token), QuirksMode::Quirks);
    }

    #[test]
    fn exact_public_identifier_match_is_quirks_case_insensitively() {
        let token = doctype(Some("html"), Some("html"), None, false);
        assert_eq!(determine_quirks_mode(&token), QuirksMode::Quirks);
    }

    #[test]
    fn html4_transitional_without_system_identifier_is_quirks() {
        let token = doctype(
            Some("html"),
            Some("-//W3C//DTD HTML 4.01 Transitional//EN"),
            None,
            false,
        );
        assert_eq!(determine_quirks_mode(&token), QuirksMode::Quirks);
    }

    #[test]
    fn html4_transitional_with_system_identifier_is_limited_quirks() {
        let token = doctype(
            Some("html"),
            Some("-//W3C//DTD HTML 4.01 Transitional//EN"),
            Some("http://www.w3.org/TR/html4/loose.dtd"),
            false,
        );
        assert_eq!(determine_quirks_mode(&token), QuirksMode::LimitedQuirks);
    }

    #[test]
    fn xhtml_transitional_is_always_limited_quirks() {
        let token = doctype(
            Some("html"),
            Some("-//W3C//DTD XHTML 1.0 Transitional//EN"),
            Some("http://www.w3.org/TR/xhtml1/DTD/xhtml1-transitional.dtd"),
            false,
        );
        assert_eq!(determine_quirks_mode(&token), QuirksMode::LimitedQuirks);
    }

    #[test]
    fn empty_system_identifier_counts_as_missing_for_the_html4_split() {
        let token = doctype(
            Some("html"),
            Some("-//W3C//DTD HTML 4.01 Frameset//EN"),
            Some(""),
            false,
        );
        assert_eq!(determine_quirks_mode(&token), QuirksMode::Quirks);
    }

    #[test]
    fn ibm_system_identifier_is_quirks() {
        let token = doctype(
            Some("html"),
            None,
            Some("http://www.ibm.com/data/dtd/v11/ibmxhtml1-transitional.dtd"),
            false,
        );
        assert_eq!(determine_quirks_mode(&token), QuirksMode::Quirks);
    }
}

#[cfg(test)]
mod insertion_algorithm_tests {
    use super::{HTML_NAMESPACE, InsertionLocation, TreeBuilder};
    use crate::document::{NodeId, NodeKind};
    use crate::tokenizer::TagToken;

    fn tag(name: &str) -> TagToken {
        TagToken {
            name: name.to_owned(),
            self_closing: false,
            attributes: vec![],
        }
    }

    /// Mirrors what the "before html" insertion mode (not yet
    /// implemented — see plan/03-tree-construction.md) does for the very
    /// first `<html>` element: append it directly to the Document node
    /// and push it onto the stack, bypassing the generic insertion
    /// algorithm under test here (which requires a current node to
    /// already exist, since "the appropriate place for inserting a node"
    /// defaults its target to the current node).
    fn bootstrap_html(builder: &mut TreeBuilder) -> NodeId {
        let root = builder.document.root();
        let html = builder.document.new_node(
            NodeKind::Element {
                name: "html".to_owned(),
                namespace: Some(HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            },
            None,
        );
        builder.document.append_child(root, html);
        builder.open_elements.push(html);
        html
    }

    fn only_child_kind(builder: &TreeBuilder, parent: NodeId) -> NodeKind {
        let children: Vec<_> = builder.document.children(parent).collect();
        assert_eq!(children.len(), 1, "expected exactly one child");
        builder.document.node(children[0]).kind.clone()
    }

    #[test]
    fn insert_html_element_nests_under_the_current_node() {
        let mut builder = TreeBuilder::new();
        let html = bootstrap_html(&mut builder);
        let body = builder.insert_html_element(&tag("body"), None);

        assert_eq!(
            builder.document.children(html).collect::<Vec<_>>(),
            vec![body]
        );
        assert_eq!(builder.open_elements.current_node(), Some(body));
        assert_eq!(
            builder.document.node(body).kind,
            NodeKind::Element {
                name: "body".to_owned(),
                namespace: Some(HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
    }

    #[test]
    fn generic_rcdata_element_parsing_algorithm() {
        let mut builder = TreeBuilder::new();
        bootstrap_html(&mut builder);
        let body = builder.insert_html_element(&tag("body"), None);
        builder.insertion_mode = super::InsertionMode::InBody;

        let (title, external_state) = builder.generic_text_element_parsing_algorithm(
            &tag("title"),
            super::GenericTextElementKind::Rcdata,
            None,
        );

        assert_eq!(external_state, crate::tokenizer::ExternalState::RcData);
        assert_eq!(
            builder.document.children(body).collect::<Vec<_>>(),
            vec![title]
        );
        assert_eq!(builder.open_elements.current_node(), Some(title));
        assert_eq!(
            builder.original_insertion_mode,
            Some(super::InsertionMode::InBody)
        );
        assert_eq!(builder.insertion_mode, super::InsertionMode::Text);
    }

    #[test]
    fn generic_rawtext_element_parsing_algorithm() {
        let mut builder = TreeBuilder::new();
        bootstrap_html(&mut builder);
        builder.insert_html_element(&tag("head"), None);
        builder.insertion_mode = super::InsertionMode::InHead;

        let (style, external_state) = builder.generic_text_element_parsing_algorithm(
            &tag("style"),
            super::GenericTextElementKind::RawText,
            None,
        );

        assert_eq!(external_state, crate::tokenizer::ExternalState::RawText);
        assert_eq!(builder.open_elements.current_node(), Some(style));
        assert_eq!(
            builder.original_insertion_mode,
            Some(super::InsertionMode::InHead)
        );
        assert_eq!(builder.insertion_mode, super::InsertionMode::Text);
    }

    #[test]
    fn a_second_element_is_never_appended_directly_to_the_document_root() {
        // "insert an element at the adjusted insertion location"'s guard:
        // a Document node with an element child already must not get a
        // second one. Exercised directly against the private method,
        // since none of the insertion-mode rules that would ever trigger
        // this (not yet implemented) exist yet — normal calls through
        // `insert_html_element` always target the current node, never the
        // document root itself.
        let mut builder = TreeBuilder::new();
        let root = builder.document.root();
        bootstrap_html(&mut builder);
        let second_html = builder.document.new_node(
            NodeKind::Element {
                name: "html".to_owned(),
                namespace: Some(HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            },
            None,
        );

        builder.insert_element_at_adjusted_insertion_location(
            second_html,
            InsertionLocation {
                parent: root,
                before: None,
            },
        );

        assert_eq!(builder.document.children(root).count(), 1);
    }

    #[test]
    fn insert_character_merges_into_an_adjacent_text_node() {
        let mut builder = TreeBuilder::new();
        let html = bootstrap_html(&mut builder);
        builder.insert_character('a', None);
        builder.insert_character('b', None);

        assert_eq!(
            only_child_kind(&builder, html),
            NodeKind::Text {
                content: "ab".to_owned()
            }
        );
    }

    #[test]
    fn insert_character_after_an_element_creates_a_new_text_node() {
        let mut builder = TreeBuilder::new();
        let html = bootstrap_html(&mut builder);
        builder.insert_html_element(&tag("body"), None);
        builder.open_elements.pop();
        builder.insert_character('x', None);

        let children: Vec<_> = builder.document.children(html).collect();
        assert_eq!(children.len(), 2);
        assert_eq!(
            builder.document.node(children[1]).kind,
            NodeKind::Text {
                content: "x".to_owned()
            }
        );
    }

    #[test]
    fn insert_comment_and_processing_instruction() {
        let mut builder = TreeBuilder::new();
        let html = bootstrap_html(&mut builder);
        builder.insert_comment("hi".to_owned(), None);
        builder.insert_processing_instruction("target".to_owned(), "data".to_owned(), None);

        let children: Vec<_> = builder.document.children(html).collect();
        assert_eq!(
            builder.document.node(children[0]).kind,
            NodeKind::Comment {
                content: "hi".to_owned()
            }
        );
        assert_eq!(
            builder.document.node(children[1]).kind,
            NodeKind::ProcessingInstruction {
                target: "target".to_owned(),
                data: "data".to_owned(),
            }
        );
    }

    #[test]
    fn foster_parenting_inserts_before_the_table_not_inside_it() {
        let mut builder = TreeBuilder::new();
        let html = bootstrap_html(&mut builder);
        let table = builder.insert_html_element(&tag("table"), None);
        builder.foster_parenting = true;
        // A character token processed while the current node is `table`
        // (a foster-parenting target) must land *before* the table, as a
        // sibling under `html` — not as a child of the table.
        builder.insert_character('x', None);

        assert_eq!(builder.document.children(table).count(), 0);
        let html_children: Vec<_> = builder.document.children(html).collect();
        assert_eq!(html_children.len(), 2);
        assert_eq!(html_children[1], table);
        assert_eq!(
            builder.document.node(html_children[0]).kind,
            NodeKind::Text {
                content: "x".to_owned()
            }
        );
    }

    #[test]
    fn foster_parenting_with_no_table_falls_back_to_the_html_element() {
        let mut builder = TreeBuilder::new();
        let html = bootstrap_html(&mut builder);
        // No table on the stack at all, but foster parenting is somehow
        // enabled and the current node is a foster-parenting target
        // (`tbody`) — falls back to inserting inside the topmost stack
        // element.
        builder.insert_html_element(&tag("tbody"), None);
        builder.foster_parenting = true;
        builder.insert_character('x', None);

        let html_children: Vec<_> = builder.document.children(html).collect();
        assert_eq!(
            builder.document.node(html_children[1]).kind,
            NodeKind::Text {
                content: "x".to_owned()
            }
        );
    }
}

#[cfg(test)]
mod active_formatting_elements_tests {
    use super::{ActiveFormattingElements, FormattingEntry, HTML_NAMESPACE, TreeBuilder};
    use crate::document::{Attribute, Document, NodeId, NodeKind};
    use crate::tokenizer::TagToken;

    fn font_with_color(document: &mut Document, color: &str) -> NodeId {
        document.new_node(
            NodeKind::Element {
                name: "font".to_owned(),
                namespace: Some(HTML_NAMESPACE.to_owned()),
                attributes: vec![Attribute {
                    name: "color".to_owned(),
                    value: color.to_owned(),
                    namespace: None,
                }],
            },
            None,
        )
    }

    fn elements(afe: &ActiveFormattingElements) -> Vec<NodeId> {
        afe.entries
            .iter()
            .filter_map(|entry| match entry {
                FormattingEntry::Element(node) => Some(*node),
                FormattingEntry::Marker => None,
            })
            .collect()
    }

    #[test]
    fn noahs_ark_clause_removes_the_earliest_of_four_identical_elements() {
        let mut document = Document::new();
        let mut afe = ActiveFormattingElements::new();
        let nodes: Vec<_> = (0..4)
            .map(|_| font_with_color(&mut document, "red"))
            .collect();
        for &node in &nodes {
            afe.push(&document, node);
        }

        assert_eq!(elements(&afe), vec![nodes[1], nodes[2], nodes[3]]);
    }

    #[test]
    fn noahs_ark_clause_ignores_elements_with_different_attributes() {
        let mut document = Document::new();
        let mut afe = ActiveFormattingElements::new();
        let red: Vec<_> = (0..3)
            .map(|_| font_with_color(&mut document, "red"))
            .collect();
        let blue = font_with_color(&mut document, "blue");
        for &node in &red {
            afe.push(&document, node);
        }
        afe.push(&document, blue);

        // Four entries total: the three "red" ones don't collide with the
        // differently-attributed "blue" one, so nothing is removed.
        assert_eq!(elements(&afe).len(), 4);
    }

    #[test]
    fn noahs_ark_clause_only_counts_entries_after_the_last_marker() {
        let mut document = Document::new();
        let mut afe = ActiveFormattingElements::new();
        for _ in 0..3 {
            let node = font_with_color(&mut document, "red");
            afe.push(&document, node);
        }
        afe.push_marker();
        let node = font_with_color(&mut document, "red");
        afe.push(&document, node);

        // The marker resets the Noah's Ark count — the 3 pre-marker
        // entries plus the marker plus the 1 post-marker entry, none
        // removed.
        assert_eq!(afe.entries.len(), 5);
    }

    #[test]
    fn clear_up_to_last_marker_removes_the_marker_and_everything_after_it() {
        let mut document = Document::new();
        let mut afe = ActiveFormattingElements::new();
        let before = font_with_color(&mut document, "red");
        afe.push(&document, before);
        afe.push_marker();
        let after = font_with_color(&mut document, "blue");
        afe.push(&document, after);

        afe.clear_up_to_last_marker();

        assert_eq!(elements(&afe), vec![before]);
    }

    fn tag(name: &str) -> TagToken {
        TagToken {
            name: name.to_owned(),
            self_closing: false,
            attributes: vec![],
        }
    }

    fn bootstrap_html(builder: &mut TreeBuilder) -> NodeId {
        let root = builder.document.root();
        let html = builder.document.new_node(
            NodeKind::Element {
                name: "html".to_owned(),
                namespace: Some(HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            },
            None,
        );
        builder.document.append_child(root, html);
        builder.open_elements.push(html);
        html
    }

    #[test]
    fn reconstruct_does_nothing_when_the_list_is_empty() {
        let mut builder = TreeBuilder::new();
        bootstrap_html(&mut builder);
        builder.insert_html_element(&tag("body"), None);
        let stack_before = builder.open_elements.current_node();

        builder.reconstruct_the_active_formatting_elements();

        assert_eq!(builder.open_elements.current_node(), stack_before);
    }

    #[test]
    fn reconstruct_does_nothing_when_the_last_entry_is_still_open() {
        let mut builder = TreeBuilder::new();
        bootstrap_html(&mut builder);
        builder.insert_html_element(&tag("body"), None);
        let b = builder.insert_html_element(&tag("b"), None);
        builder
            .active_formatting_elements
            .push(&builder.document, b);

        builder.reconstruct_the_active_formatting_elements();

        // `b` is still on the stack, so there's nothing to reconstruct —
        // no new node created.
        assert_eq!(builder.open_elements.current_node(), Some(b));
    }

    #[test]
    fn reconstruct_reopens_a_single_implicitly_closed_formatting_element() {
        let mut builder = TreeBuilder::new();
        bootstrap_html(&mut builder);
        let body = builder.insert_html_element(&tag("body"), None);
        let b = builder.insert_html_element(&tag("b"), None);
        builder
            .active_formatting_elements
            .push(&builder.document, b);
        // Simulate `b` having been implicitly popped off the stack (e.g.
        // by a misnested close) while staying in the formatting-elements
        // list.
        builder.open_elements.pop();

        builder.reconstruct_the_active_formatting_elements();

        let new_b = builder
            .open_elements
            .current_node()
            .expect("reconstruct should have pushed a new element");
        assert_ne!(new_b, b, "must be a freshly created node, not the old one");
        // The original `b` stays in the tree exactly where it was parsed
        // — reconstruction never removes/moves anything, it only adds a
        // new sibling to carry the formatting forward.
        assert_eq!(
            builder.document.children(body).collect::<Vec<_>>(),
            vec![b, new_b]
        );
        assert_eq!(
            builder.document.node(new_b).kind,
            NodeKind::Element {
                name: "b".to_owned(),
                namespace: Some(HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
        assert_eq!(
            super::FormattingEntry::Element(new_b),
            *builder.active_formatting_elements.entries.last().unwrap()
        );
    }

    #[test]
    fn reconstruct_reopens_nested_formatting_elements_in_the_original_order() {
        let mut builder = TreeBuilder::new();
        bootstrap_html(&mut builder);
        let body = builder.insert_html_element(&tag("body"), None);
        let b = builder.insert_html_element(&tag("b"), None);
        builder
            .active_formatting_elements
            .push(&builder.document, b);
        let i = builder.insert_html_element(&tag("i"), None);
        builder
            .active_formatting_elements
            .push(&builder.document, i);
        // Both implicitly closed, in order.
        builder.open_elements.pop();
        builder.open_elements.pop();

        builder.reconstruct_the_active_formatting_elements();

        // The original b>i subtree stays exactly where it was parsed...
        let body_children: Vec<_> = builder.document.children(body).collect();
        assert_eq!(body_children.len(), 2);
        assert_eq!(body_children[0], b);
        assert_eq!(builder.document.children(b).collect::<Vec<_>>(), vec![i]);
        // ...and reconstruction adds a fresh, equally-nested b>i sibling
        // structure to carry both formattings forward.
        let new_b = body_children[1];
        assert_eq!(
            builder.document.node(new_b).kind,
            NodeKind::Element {
                name: "b".to_owned(),
                namespace: Some(HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
        let new_b_children: Vec<_> = builder.document.children(new_b).collect();
        assert_eq!(new_b_children.len(), 1);
        assert_eq!(
            builder.document.node(new_b_children[0]).kind,
            NodeKind::Element {
                name: "i".to_owned(),
                namespace: Some(HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
        assert_eq!(
            builder.open_elements.current_node(),
            Some(new_b_children[0])
        );
    }
}

#[cfg(test)]
mod adoption_agency_tests {
    use super::TreeBuilder;
    use crate::document::{NodeId, NodeKind};
    use crate::tokenizer::TagToken;

    fn tag(name: &str) -> TagToken {
        TagToken {
            name: name.to_owned(),
            self_closing: false,
            attributes: vec![],
        }
    }

    fn bootstrap_html(builder: &mut TreeBuilder) -> NodeId {
        let root = builder.document.root();
        let html = builder.document.new_node(
            NodeKind::Element {
                name: "html".to_owned(),
                namespace: Some(super::HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            },
            None,
        );
        builder.document.append_child(root, html);
        builder.open_elements.push(html);
        html
    }

    #[test]
    fn current_node_matching_and_not_active_is_just_popped() {
        // Step 2's fast path: `<b>` was inserted but never registered as
        // an active formatting element (e.g. it was already dropped from
        // that list by an earlier Noah's Ark/marker-clearing step) — so
        // there's nothing to reconstruct, just pop it like any other end
        // tag would.
        let mut builder = TreeBuilder::new();
        bootstrap_html(&mut builder);
        let body = builder.insert_html_element(&tag("body"), None);
        let b = builder.insert_html_element(&tag("b"), None);

        builder.adoption_agency_algorithm(&tag("b"));

        assert_eq!(builder.open_elements.current_node(), Some(body));
        // The element itself is untouched in the tree, just off the
        // stack.
        assert_eq!(builder.document.children(body).collect::<Vec<_>>(), vec![b]);
    }

    #[test]
    fn no_furthest_block_pops_up_to_and_including_the_formatting_element() {
        // `<p><b>bold</p>` (no misnesting content between `<b>` and the
        // current node) — step 8's simple case.
        let mut builder = TreeBuilder::new();
        bootstrap_html(&mut builder);
        let body = builder.insert_html_element(&tag("body"), None);
        let p = builder.insert_html_element(&tag("p"), None);
        let b = builder.insert_html_element(&tag("b"), None);
        builder
            .active_formatting_elements
            .push(&builder.document, b);

        builder.adoption_agency_algorithm(&tag("b"));

        assert_eq!(builder.open_elements.current_node(), Some(p));
        assert!(!builder.open_elements.contains(b));
        assert!(builder.active_formatting_elements.entries.is_empty());
        // The tree shape itself is untouched — `b` remains `p`'s child.
        assert_eq!(builder.document.children(p).collect::<Vec<_>>(), vec![b]);
        assert_eq!(builder.document.children(body).collect::<Vec<_>>(), vec![p]);
    }

    #[test]
    fn not_in_scope_returns_without_changing_the_tree() {
        // `formattingElement` (`b`) is still on the stack, but a `table`
        // — a default-scope boundary — sits between it and the current
        // node, so `has_element_in_scope` stops at `table` before ever
        // reaching `b`. Step 5 bails out entirely, parse error only.
        let mut builder = TreeBuilder::new();
        bootstrap_html(&mut builder);
        builder.insert_html_element(&tag("body"), None);
        let b = builder.insert_html_element(&tag("b"), None);
        builder
            .active_formatting_elements
            .push(&builder.document, b);
        let table = builder.insert_html_element(&tag("table"), None);

        builder.adoption_agency_algorithm(&tag("b"));

        // Nothing changed: no pop, no tree mutation.
        assert_eq!(builder.open_elements.current_node(), Some(table));
        assert!(builder.open_elements.contains(b));
        assert_eq!(
            builder.document.children(b).collect::<Vec<_>>(),
            vec![table]
        );
    }

    #[test]
    fn relocates_special_content_out_of_the_misnested_formatting_element() {
        // The canonical case: `<b><div>inner</div></b>` — `div` (a
        // special-category element) is still open when `</b>` arrives,
        // so it becomes the "furthest block" that gets relocated: `b`
        // ends up split into an empty original and a fresh clone that
        // wraps `div`'s former content, nested *inside* `div`.
        let mut builder = TreeBuilder::new();
        bootstrap_html(&mut builder);
        let body = builder.insert_html_element(&tag("body"), None);
        let b = builder.insert_html_element(&tag("b"), None);
        builder
            .active_formatting_elements
            .push(&builder.document, b);
        let div = builder.insert_html_element(&tag("div"), None);
        let inner_text = builder.document.new_node(
            NodeKind::Text {
                content: "inner".to_owned(),
            },
            None,
        );
        builder.document.append_child(div, inner_text);

        builder.adoption_agency_algorithm(&tag("b"));

        // Stack: html, body, div — both `b` (the original) and its
        // clone are fully popped off by the end (the clone itself hits
        // the outer loop's own no-furthest-block fast path on its
        // second pass).
        assert_eq!(builder.open_elements.current_node(), Some(div));
        assert!(!builder.open_elements.contains(b));
        assert!(builder.active_formatting_elements.entries.is_empty());

        let body_children: Vec<_> = builder.document.children(body).collect();
        assert_eq!(body_children, vec![b, div]);
        assert_eq!(builder.document.children(b).count(), 0);

        let div_children: Vec<_> = builder.document.children(div).collect();
        assert_eq!(div_children.len(), 1);
        let new_b = div_children[0];
        assert_ne!(new_b, b);
        assert_eq!(
            builder.document.node(new_b).kind,
            NodeKind::Element {
                name: "b".to_owned(),
                namespace: Some(super::HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
        assert_eq!(
            builder.document.children(new_b).collect::<Vec<_>>(),
            vec![inner_text]
        );
    }

    #[test]
    fn any_other_end_tag_pops_up_to_a_matching_ancestor() {
        let mut builder = TreeBuilder::new();
        bootstrap_html(&mut builder);
        let body = builder.insert_html_element(&tag("body"), None);
        let div = builder.insert_html_element(&tag("div"), None);
        builder.insert_html_element(&tag("span"), None);

        builder.any_other_end_tag_in_body(&tag("div"));

        assert_eq!(builder.open_elements.current_node(), Some(body));
        assert!(!builder.open_elements.contains(div));
    }

    #[test]
    fn any_other_end_tag_gives_up_at_the_first_special_element() {
        let mut builder = TreeBuilder::new();
        bootstrap_html(&mut builder);
        builder.insert_html_element(&tag("body"), None);
        builder.insert_html_element(&tag("div"), None); // special: blocks the search
        let span = builder.insert_html_element(&tag("span"), None);

        // No `x` element exists anywhere on the stack.
        builder.any_other_end_tag_in_body(&tag("x"));

        // Nothing was popped: the search hit `div` (special) before
        // finding a match and gave up.
        assert_eq!(builder.open_elements.current_node(), Some(span));
    }
}

#[cfg(test)]
mod insertion_mode_tests {
    use super::{InsertionMode, QuirksMode, TreeBuilder};
    use crate::document::{NodeId, NodeKind};
    use crate::tokenizer::{Attribute, DoctypeToken, ExternalState, Position, TagToken, TokenKind};

    fn pos() -> Position {
        Position {
            line: 1,
            column: 1,
            byte_offset: 0,
        }
    }

    fn doctype(
        name: Option<&str>,
        public_identifier: Option<&str>,
        system_identifier: Option<&str>,
    ) -> TokenKind {
        TokenKind::Doctype(DoctypeToken {
            name: name.map(str::to_owned),
            public_identifier: public_identifier.map(str::to_owned),
            system_identifier: system_identifier.map(str::to_owned),
            force_quirks: false,
        })
    }

    fn start_tag(name: &str) -> TokenKind {
        TokenKind::StartTag(TagToken {
            name: name.to_owned(),
            self_closing: false,
            attributes: vec![],
        })
    }

    fn start_tag_with_attrs(name: &str, attrs: &[(&str, &str)]) -> TokenKind {
        TokenKind::StartTag(TagToken {
            name: name.to_owned(),
            self_closing: false,
            attributes: attrs
                .iter()
                .map(|&(name, value)| Attribute {
                    name: name.to_owned(),
                    value: value.to_owned(),
                })
                .collect(),
        })
    }

    fn end_tag(name: &str) -> TokenKind {
        TokenKind::EndTag(TagToken {
            name: name.to_owned(),
            self_closing: false,
            attributes: vec![],
        })
    }

    #[test]
    fn initial_mode_ignores_whitespace() {
        let mut builder = TreeBuilder::new();
        builder.process_token(&TokenKind::Character(' '), pos());

        assert_eq!(builder.insertion_mode, InsertionMode::Initial);
        assert_eq!(
            builder.document.children(builder.document.root()).count(),
            0
        );
    }

    #[test]
    fn initial_mode_inserts_comment_as_a_document_child() {
        let mut builder = TreeBuilder::new();
        builder.process_token(&TokenKind::Comment("hi".to_owned()), pos());

        let root = builder.document.root();
        let children: Vec<_> = builder.document.children(root).collect();
        assert_eq!(
            builder.document.node(children[0]).kind,
            NodeKind::Comment {
                content: "hi".to_owned()
            }
        );
        assert_eq!(builder.insertion_mode, InsertionMode::Initial);
    }

    #[test]
    fn initial_mode_doctype_inserts_a_doctype_node_and_switches_mode() {
        let mut builder = TreeBuilder::new();
        builder.process_token(&doctype(Some("html"), None, None), pos());

        let root = builder.document.root();
        let children: Vec<_> = builder.document.children(root).collect();
        assert_eq!(
            builder.document.node(children[0]).kind,
            NodeKind::Doctype {
                name: Some("html".to_owned()),
                public_identifier: Some(String::new()),
                system_identifier: Some(String::new()),
            }
        );
        assert_eq!(builder.quirks_mode, QuirksMode::NoQuirks);
        assert_eq!(builder.insertion_mode, InsertionMode::BeforeHtml);
    }

    #[test]
    fn initial_mode_doctype_records_quirks_mode() {
        let mut builder = TreeBuilder::new();
        builder.process_token(&doctype(Some("html"), Some("HTML"), None), pos());

        assert_eq!(builder.quirks_mode, QuirksMode::Quirks);
    }

    #[test]
    fn end_to_end_doctype_then_html_then_head_reaches_in_head() {
        let mut builder = TreeBuilder::new();
        builder.process_token(&doctype(Some("html"), None, None), pos());
        builder.process_token(&start_tag_with_attrs("html", &[("lang", "en")]), pos());
        builder.process_token(&start_tag("head"), pos());

        assert_eq!(builder.quirks_mode, QuirksMode::NoQuirks);
        assert_eq!(builder.insertion_mode, InsertionMode::InHead);

        let root = builder.document.root();
        let root_children: Vec<_> = builder.document.children(root).collect();
        // [doctype, html]
        assert_eq!(root_children.len(), 2);
        let html = root_children[1];
        assert_eq!(
            builder.document.node(html).kind,
            NodeKind::Element {
                name: "html".to_owned(),
                namespace: Some(super::HTML_NAMESPACE.to_owned()),
                attributes: vec![super::Attribute {
                    name: "lang".to_owned(),
                    value: "en".to_owned(),
                    namespace: None,
                }],
            }
        );
        let head = builder.document.children(html).next().unwrap();
        assert_eq!(
            builder.document.node(head).kind,
            NodeKind::Element {
                name: "head".to_owned(),
                namespace: Some(super::HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
        assert_eq!(builder.head_element_pointer, Some(head));
        assert_eq!(builder.open_elements.current_node(), Some(head));
    }

    #[test]
    fn end_to_end_without_a_doctype_sets_quirks_mode_and_synthesizes_html_and_head() {
        // No DOCTYPE token at all: a single `process_token` call cascades
        // Initial -> BeforeHtml -> BeforeHead (all via "anything
        // else"/reprocess) before finally being consumed by BeforeHead's
        // own "start tag head" rule.
        let mut builder = TreeBuilder::new();
        builder.process_token(&start_tag("head"), pos());

        assert_eq!(builder.quirks_mode, QuirksMode::Quirks);
        assert_eq!(builder.insertion_mode, InsertionMode::InHead);

        let root = builder.document.root();
        let html = builder.document.children(root).next().unwrap();
        assert_eq!(
            builder.document.node(html).kind,
            NodeKind::Element {
                name: "html".to_owned(),
                namespace: Some(super::HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
        let head = builder.document.children(html).next().unwrap();
        assert_eq!(builder.head_element_pointer, Some(head));
        assert!(builder.open_elements.contains(html));
    }

    #[test]
    fn before_html_mode_anything_else_synthesizes_html_and_reprocesses() {
        let mut builder = TreeBuilder::new();
        builder.insertion_mode = InsertionMode::BeforeHtml;

        let outcome = builder.process_token_before_html(&end_tag("br"), pos());

        assert!(matches!(outcome, super::TokenOutcome::Reprocess));
        assert_eq!(builder.insertion_mode, InsertionMode::BeforeHead);
        let root = builder.document.root();
        let html = builder.document.children(root).next().unwrap();
        assert_eq!(
            builder.document.node(html).kind,
            NodeKind::Element {
                name: "html".to_owned(),
                namespace: Some(super::HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
        assert_eq!(builder.open_elements.current_node(), Some(html));
    }

    #[test]
    fn before_head_mode_html_start_tag_merges_new_attributes_only() {
        let mut builder = TreeBuilder::new();
        builder.insertion_mode = InsertionMode::BeforeHtml;
        builder.process_token(&start_tag_with_attrs("html", &[("lang", "en")]), pos());
        assert_eq!(builder.insertion_mode, InsertionMode::BeforeHead);

        builder.process_token_before_head(
            &start_tag_with_attrs("html", &[("lang", "de"), ("dir", "ltr")]),
            pos(),
        );

        let root = builder.document.root();
        let html = builder.document.children(root).next().unwrap();
        let NodeKind::Element { attributes, .. } = &builder.document.node(html).kind else {
            unreachable!()
        };
        // "lang" already existed (value untouched), "dir" is new.
        assert_eq!(attributes.len(), 2);
        assert!(
            attributes
                .iter()
                .any(|a| a.name == "lang" && a.value == "en")
        );
        assert!(
            attributes
                .iter()
                .any(|a| a.name == "dir" && a.value == "ltr")
        );
    }

    /// Drives a fresh builder through Initial -> BeforeHtml -> BeforeHead
    /// -> InHead by processing a `<head>` start tag (already exercised
    /// end-to-end above) and returns `(html, head)`.
    fn bootstrap_in_head(builder: &mut TreeBuilder) -> (NodeId, NodeId) {
        builder.process_token(&start_tag("head"), pos());
        let root = builder.document.root();
        let html = builder.document.children(root).next().unwrap();
        let head = builder.open_elements.current_node().unwrap();
        (html, head)
    }

    #[test]
    fn in_head_inserts_whitespace_characters() {
        let mut builder = TreeBuilder::new();
        let (_, head) = bootstrap_in_head(&mut builder);

        builder.process_token(&TokenKind::Character(' '), pos());

        let children: Vec<_> = builder.document.children(head).collect();
        assert_eq!(
            builder.document.node(children[0]).kind,
            NodeKind::Text {
                content: " ".to_owned()
            }
        );
    }

    #[test]
    fn in_head_void_like_elements_are_inserted_then_immediately_popped() {
        let mut builder = TreeBuilder::new();
        let (_, head) = bootstrap_in_head(&mut builder);

        builder.process_token(&start_tag("meta"), pos());

        let children: Vec<_> = builder.document.children(head).collect();
        assert_eq!(children.len(), 1);
        assert_eq!(
            builder.document.node(children[0]).kind,
            NodeKind::Element {
                name: "meta".to_owned(),
                namespace: Some(super::HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
        assert_eq!(builder.open_elements.current_node(), Some(head));
    }

    #[test]
    fn in_head_title_switches_to_text_mode_with_rcdata() {
        let mut builder = TreeBuilder::new();
        let (_, head) = bootstrap_in_head(&mut builder);

        let state = builder.process_token(&start_tag("title"), pos());

        assert_eq!(state, Some(ExternalState::RcData));
        assert_eq!(builder.insertion_mode, InsertionMode::Text);
        assert_eq!(builder.original_insertion_mode, Some(InsertionMode::InHead));
        let title = builder.open_elements.current_node().unwrap();
        assert_eq!(
            builder.document.children(head).collect::<Vec<_>>(),
            vec![title]
        );
    }

    #[test]
    fn in_head_script_switches_to_text_mode_with_script_data() {
        let mut builder = TreeBuilder::new();
        bootstrap_in_head(&mut builder);

        let state = builder.process_token(&start_tag("script"), pos());

        assert_eq!(state, Some(ExternalState::ScriptData));
        assert_eq!(builder.insertion_mode, InsertionMode::Text);
    }

    #[test]
    fn in_head_noscript_start_tag_switches_mode() {
        let mut builder = TreeBuilder::new();
        bootstrap_in_head(&mut builder);

        builder.process_token(&start_tag("noscript"), pos());

        assert_eq!(builder.insertion_mode, InsertionMode::InHeadNoscript);
    }

    #[test]
    fn in_head_end_tag_head_pops_and_switches_to_after_head() {
        let mut builder = TreeBuilder::new();
        let (html, head) = bootstrap_in_head(&mut builder);

        builder.process_token(&end_tag("head"), pos());

        assert_eq!(builder.insertion_mode, InsertionMode::AfterHead);
        assert_eq!(builder.open_elements.current_node(), Some(html));
        assert!(!builder.open_elements.contains(head));
    }

    #[test]
    fn in_head_template_round_trips_through_the_stack() {
        let mut builder = TreeBuilder::new();
        let (_, head) = bootstrap_in_head(&mut builder);

        builder.process_token(&start_tag("template"), pos());
        let template = builder.open_elements.current_node().unwrap();
        assert_ne!(template, head);

        builder.process_token(&end_tag("template"), pos());

        assert_eq!(builder.open_elements.current_node(), Some(head));
        assert!(!builder.open_elements.contains(template));
        assert_eq!(
            builder.document.children(head).collect::<Vec<_>>(),
            vec![template]
        );
    }

    #[test]
    fn in_head_noscript_delegates_link_to_in_head() {
        let mut builder = TreeBuilder::new();
        bootstrap_in_head(&mut builder);
        builder.process_token(&start_tag("noscript"), pos());
        let noscript = builder.open_elements.current_node().unwrap();

        builder.process_token(&start_tag("link"), pos());

        // `link` is inserted+popped as a child of `noscript` (the current
        // node while in "in head noscript") — delegation runs "in
        // head"'s rules verbatim, including its own insertion target.
        assert_eq!(builder.open_elements.current_node(), Some(noscript));
        let children: Vec<_> = builder.document.children(noscript).collect();
        assert_eq!(children.len(), 1);
        assert_eq!(
            builder.document.node(children[0]).kind,
            NodeKind::Element {
                name: "link".to_owned(),
                namespace: Some(super::HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
    }

    #[test]
    fn in_head_noscript_end_tag_noscript_returns_to_in_head() {
        let mut builder = TreeBuilder::new();
        let (_, head) = bootstrap_in_head(&mut builder);
        builder.process_token(&start_tag("noscript"), pos());

        builder.process_token(&end_tag("noscript"), pos());

        assert_eq!(builder.insertion_mode, InsertionMode::InHead);
        assert_eq!(builder.open_elements.current_node(), Some(head));
    }

    #[test]
    fn after_head_body_start_tag_switches_to_in_body() {
        let mut builder = TreeBuilder::new();
        let (html, head) = bootstrap_in_head(&mut builder);
        builder.process_token(&end_tag("head"), pos());
        assert_eq!(builder.insertion_mode, InsertionMode::AfterHead);

        builder.process_token(&start_tag("body"), pos());

        assert_eq!(builder.insertion_mode, InsertionMode::InBody);
        let body = builder.open_elements.current_node().unwrap();
        assert_eq!(
            builder.document.children(html).collect::<Vec<_>>(),
            vec![head, body]
        );
    }

    #[test]
    fn after_head_delegates_link_by_temporarily_repushing_head() {
        let mut builder = TreeBuilder::new();
        let (_, head) = bootstrap_in_head(&mut builder);
        builder.process_token(&end_tag("head"), pos());
        assert_eq!(builder.insertion_mode, InsertionMode::AfterHead);

        builder.process_token(&start_tag("link"), pos());

        // `head` is not left on the stack afterward ("it might not be
        // the current node at this point" — here it's not on the stack
        // *at all* anymore, matching before this call too)...
        assert!(!builder.open_elements.contains(head));
        // ...but the link element really did get inserted as its child.
        let children: Vec<_> = builder.document.children(head).collect();
        assert_eq!(children.len(), 1);
        assert_eq!(
            builder.document.node(children[0]).kind,
            NodeKind::Element {
                name: "link".to_owned(),
                namespace: Some(super::HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
        assert_eq!(builder.insertion_mode, InsertionMode::AfterHead);
    }

    #[test]
    fn after_head_anything_else_synthesizes_body_and_switches_mode() {
        let mut builder = TreeBuilder::new();
        let (html, head) = bootstrap_in_head(&mut builder);
        builder.process_token(&end_tag("head"), pos());

        // Called directly (not via the public `process_token` loop):
        // "Reprocess" would otherwise hand the same token to "in body",
        // not yet implemented.
        let outcome = builder.process_token_after_head(&TokenKind::Character('x'), pos());

        assert!(matches!(outcome, super::TokenOutcome::Reprocess));
        assert_eq!(builder.insertion_mode, InsertionMode::InBody);
        let body = builder.open_elements.current_node().unwrap();
        assert_eq!(
            builder.document.node(body).kind,
            NodeKind::Element {
                name: "body".to_owned(),
                namespace: Some(super::HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
        assert_eq!(
            builder.document.children(html).collect::<Vec<_>>(),
            vec![head, body]
        );
    }

    /// Drives a fresh builder all the way into "in body" via a single
    /// `<body>` start tag (cascading Initial -> BeforeHtml -> BeforeHead
    /// -> InHead -> AfterHead, ending at AfterHead's own explicit
    /// "start tag body" rule — already exercised end-to-end above).
    /// Returns `(html, body)`.
    fn bootstrap_in_body(builder: &mut TreeBuilder) -> (NodeId, NodeId) {
        builder.process_token(&start_tag("body"), pos());
        let root = builder.document.root();
        let html = builder.document.children(root).next().unwrap();
        let body = builder.open_elements.current_node().unwrap();
        (html, body)
    }

    #[test]
    fn in_body_character_sets_frameset_ok_and_ignores_nul() {
        let mut builder = TreeBuilder::new();
        let (_, body) = bootstrap_in_body(&mut builder);
        assert!(builder.frameset_ok);

        builder.process_token(&TokenKind::Character('\0'), pos());
        assert_eq!(builder.document.children(body).count(), 0);

        builder.process_token(&TokenKind::Character('x'), pos());
        assert!(!builder.frameset_ok);
        let children: Vec<_> = builder.document.children(body).collect();
        assert_eq!(
            builder.document.node(children[0]).kind,
            NodeKind::Text {
                content: "x".to_owned()
            }
        );
    }

    #[test]
    fn in_body_div_closes_an_open_p_in_button_scope() {
        let mut builder = TreeBuilder::new();
        let (_, body) = bootstrap_in_body(&mut builder);
        builder.process_token(&start_tag("p"), pos());
        let p = builder.open_elements.current_node().unwrap();

        builder.process_token(&start_tag("div"), pos());

        assert!(!builder.open_elements.contains(p));
        let div = builder.open_elements.current_node().unwrap();
        assert_eq!(
            builder.document.children(body).collect::<Vec<_>>(),
            vec![p, div]
        );
    }

    #[test]
    fn in_body_heading_pops_a_currently_open_heading() {
        let mut builder = TreeBuilder::new();
        let (_, body) = bootstrap_in_body(&mut builder);
        builder.process_token(&start_tag("h1"), pos());
        let h1 = builder.open_elements.current_node().unwrap();

        builder.process_token(&start_tag("h2"), pos());

        assert!(!builder.open_elements.contains(h1));
        let h2 = builder.open_elements.current_node().unwrap();
        assert_eq!(
            builder.document.children(body).collect::<Vec<_>>(),
            vec![h1, h2]
        );
    }

    #[test]
    fn in_body_formatting_start_tag_pushes_onto_active_formatting_elements() {
        let mut builder = TreeBuilder::new();
        bootstrap_in_body(&mut builder);

        builder.process_token(&start_tag("b"), pos());

        let b = builder.open_elements.current_node().unwrap();
        assert!(matches!(
            builder.active_formatting_elements.entries.last(),
            Some(super::FormattingEntry::Element(node)) if *node == b
        ));
    }

    #[test]
    fn in_body_end_tag_b_runs_the_adoption_agency_algorithm() {
        let mut builder = TreeBuilder::new();
        let (_, body) = bootstrap_in_body(&mut builder);
        builder.process_token(&start_tag("b"), pos());
        let b = builder.open_elements.current_node().unwrap();

        builder.process_token(&end_tag("b"), pos());

        // Simple case (no misnesting): adoption agency's step 2 fast
        // path just pops it.
        assert!(!builder.open_elements.contains(b));
        assert_eq!(builder.open_elements.current_node(), Some(body));
    }

    #[test]
    fn in_body_second_a_start_tag_closes_the_first_via_adoption_agency() {
        let mut builder = TreeBuilder::new();
        bootstrap_in_body(&mut builder);
        builder.process_token(&start_tag("a"), pos());
        let a1 = builder.open_elements.current_node().unwrap();

        builder.process_token(&start_tag("a"), pos());

        let a2 = builder.open_elements.current_node().unwrap();
        assert_ne!(a1, a2);
        assert!(!builder.open_elements.contains(a1));
        assert!(matches!(
            builder.active_formatting_elements.entries.last(),
            Some(super::FormattingEntry::Element(node)) if *node == a2
        ));
    }

    #[test]
    fn in_body_void_element_is_inserted_then_immediately_popped() {
        let mut builder = TreeBuilder::new();
        let (_, body) = bootstrap_in_body(&mut builder);

        builder.process_token(&start_tag("br"), pos());

        let children: Vec<_> = builder.document.children(body).collect();
        assert_eq!(children.len(), 1);
        assert_eq!(
            builder.document.node(children[0]).kind,
            NodeKind::Element {
                name: "br".to_owned(),
                namespace: Some(super::HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
        assert_eq!(builder.open_elements.current_node(), Some(body));
        assert!(!builder.frameset_ok);
    }

    #[test]
    fn in_body_br_end_tag_is_treated_as_a_start_tag() {
        let mut builder = TreeBuilder::new();
        let (_, body) = bootstrap_in_body(&mut builder);

        builder.process_token(&end_tag("br"), pos());

        let children: Vec<_> = builder.document.children(body).collect();
        assert_eq!(children.len(), 1);
        assert_eq!(
            builder.document.node(children[0]).kind,
            NodeKind::Element {
                name: "br".to_owned(),
                namespace: Some(super::HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
    }

    #[test]
    fn in_body_li_closes_a_previous_open_li() {
        let mut builder = TreeBuilder::new();
        let (_, body) = bootstrap_in_body(&mut builder);
        builder.process_token(&start_tag("ul"), pos());
        builder.process_token(&start_tag("li"), pos());
        let li1 = builder.open_elements.current_node().unwrap();

        builder.process_token(&start_tag("li"), pos());

        assert!(!builder.open_elements.contains(li1));
        let ul = builder.document.children(body).next().unwrap();
        let ul_children: Vec<_> = builder.document.children(ul).collect();
        assert_eq!(ul_children.len(), 2);
        assert_eq!(ul_children[0], li1);
    }

    #[test]
    fn in_body_form_sets_pointer_and_ignores_a_second_form() {
        let mut builder = TreeBuilder::new();
        bootstrap_in_body(&mut builder);
        builder.process_token(&start_tag("form"), pos());
        let form1 = builder.open_elements.current_node().unwrap();
        assert_eq!(builder.form_element_pointer, Some(form1));

        builder.process_token(&start_tag("form"), pos());

        // Second form is ignored entirely: no new element, pointer
        // unchanged, current node unchanged.
        assert_eq!(builder.open_elements.current_node(), Some(form1));
        assert_eq!(builder.form_element_pointer, Some(form1));
    }

    #[test]
    fn in_body_table_switches_insertion_mode() {
        let mut builder = TreeBuilder::new();
        bootstrap_in_body(&mut builder);

        builder.process_token(&start_tag("table"), pos());

        assert_eq!(builder.insertion_mode, InsertionMode::InTable);
        assert!(!builder.frameset_ok);
    }

    #[test]
    fn in_body_plaintext_returns_plaintext_state_without_switching_mode() {
        let mut builder = TreeBuilder::new();
        bootstrap_in_body(&mut builder);

        let state = builder.process_token(&start_tag("plaintext"), pos());

        assert_eq!(state, Some(ExternalState::PlainText));
        assert_eq!(builder.insertion_mode, InsertionMode::InBody);
    }

    #[test]
    fn in_body_textarea_switches_to_text_mode_and_skips_next_lf() {
        let mut builder = TreeBuilder::new();
        bootstrap_in_body(&mut builder);

        let state = builder.process_token(&start_tag("textarea"), pos());

        assert_eq!(state, Some(ExternalState::RcData));
        assert_eq!(builder.insertion_mode, InsertionMode::Text);
        assert!(builder.skip_next_line_feed);
        assert!(!builder.frameset_ok);
    }

    #[test]
    fn in_body_pre_ignores_one_leading_line_feed() {
        let mut builder = TreeBuilder::new();
        bootstrap_in_body(&mut builder);
        builder.process_token(&start_tag("pre"), pos());
        let pre = builder.open_elements.current_node().unwrap();
        assert!(builder.skip_next_line_feed);

        builder.process_token(&TokenKind::Character('\n'), pos());
        assert_eq!(builder.document.children(pre).count(), 0);
        assert!(!builder.skip_next_line_feed);

        builder.process_token(&TokenKind::Character('x'), pos());
        assert_eq!(builder.document.children(pre).count(), 1);
    }

    #[test]
    fn in_body_math_inserts_with_mathml_namespace() {
        let mut builder = TreeBuilder::new();
        bootstrap_in_body(&mut builder);

        builder.process_token(&start_tag("math"), pos());

        let math = builder.open_elements.current_node().unwrap();
        assert_eq!(
            builder.document.node(math).kind,
            NodeKind::Element {
                name: "math".to_owned(),
                namespace: Some(super::MATHML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
    }

    #[test]
    fn in_body_any_other_start_tag_is_inserted_as_an_ordinary_element() {
        let mut builder = TreeBuilder::new();
        let (_, body) = bootstrap_in_body(&mut builder);

        builder.process_token(&start_tag("span"), pos());

        let span = builder.open_elements.current_node().unwrap();
        assert_eq!(
            builder.document.node(span).kind,
            NodeKind::Element {
                name: "span".to_owned(),
                namespace: Some(super::HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
        assert_eq!(
            builder.document.children(body).collect::<Vec<_>>(),
            vec![span]
        );
    }

    #[test]
    fn in_body_stray_table_section_start_tags_are_ignored() {
        let mut builder = TreeBuilder::new();
        let (_, body) = bootstrap_in_body(&mut builder);

        builder.process_token(&start_tag("tbody"), pos());

        assert_eq!(builder.document.children(body).count(), 0);
        assert_eq!(builder.open_elements.current_node(), Some(body));
    }

    #[test]
    fn in_body_delegates_meta_to_in_head_rules() {
        let mut builder = TreeBuilder::new();
        let (_, body) = bootstrap_in_body(&mut builder);

        builder.process_token(&start_tag("meta"), pos());

        let children: Vec<_> = builder.document.children(body).collect();
        assert_eq!(children.len(), 1);
        assert_eq!(
            builder.document.node(children[0]).kind,
            NodeKind::Element {
                name: "meta".to_owned(),
                namespace: Some(super::HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
        assert_eq!(builder.open_elements.current_node(), Some(body));
    }

    #[test]
    fn in_body_any_other_end_tag_pops_through_a_matching_ancestor() {
        let mut builder = TreeBuilder::new();
        let (_, body) = bootstrap_in_body(&mut builder);
        builder.process_token(&start_tag("custom-element"), pos());
        let custom = builder.open_elements.current_node().unwrap();

        builder.process_token(&end_tag("custom-element"), pos());

        assert!(!builder.open_elements.contains(custom));
        assert_eq!(builder.open_elements.current_node(), Some(body));
    }

    #[test]
    fn text_mode_inserts_characters_into_the_rcdata_element() {
        let mut builder = TreeBuilder::new();
        bootstrap_in_body(&mut builder);
        builder.process_token(&start_tag("title"), pos());
        let title = builder.open_elements.current_node().unwrap();
        assert_eq!(builder.insertion_mode, InsertionMode::Text);

        builder.process_token(&TokenKind::Character('h'), pos());
        builder.process_token(&TokenKind::Character('i'), pos());

        let children: Vec<_> = builder.document.children(title).collect();
        assert_eq!(children.len(), 1);
        assert_eq!(
            builder.document.node(children[0]).kind,
            NodeKind::Text {
                content: "hi".to_owned()
            }
        );
    }

    #[test]
    fn text_mode_end_tag_pops_and_restores_original_insertion_mode() {
        let mut builder = TreeBuilder::new();
        let (_, body) = bootstrap_in_body(&mut builder);
        builder.process_token(&start_tag("title"), pos());
        let title = builder.open_elements.current_node().unwrap();

        // Any end tag closes it — even one that doesn't match "title",
        // matching "text" mode's "any other end tag" rule.
        builder.process_token(&end_tag("nonsense"), pos());

        assert!(!builder.open_elements.contains(title));
        assert_eq!(builder.insertion_mode, InsertionMode::InBody);
        assert_eq!(builder.open_elements.current_node(), Some(body));
        assert_eq!(builder.original_insertion_mode, None);
    }

    #[test]
    fn text_mode_eof_pops_and_reprocesses_in_the_original_mode() {
        let mut builder = TreeBuilder::new();
        let (_, body) = bootstrap_in_body(&mut builder);
        builder.process_token(&start_tag("title"), pos());
        let title = builder.open_elements.current_node().unwrap();

        builder.process_token(&TokenKind::Eof, pos());

        assert!(!builder.open_elements.contains(title));
        assert_eq!(builder.insertion_mode, InsertionMode::InBody);
        assert_eq!(builder.open_elements.current_node(), Some(body));
    }

    /// Drives a fresh builder into "in table" via `bootstrap_in_body`
    /// plus a `<table>` start tag (already exercised above). Returns
    /// `(body, table)`.
    fn bootstrap_in_table(builder: &mut TreeBuilder) -> (NodeId, NodeId) {
        let (_, body) = bootstrap_in_body(builder);
        builder.process_token(&start_tag("table"), pos());
        let table = builder.open_elements.current_node().unwrap();
        (body, table)
    }

    #[test]
    fn in_table_caption_switches_mode_and_pushes_a_marker() {
        let mut builder = TreeBuilder::new();
        let (_, table) = bootstrap_in_table(&mut builder);

        builder.process_token(&start_tag("caption"), pos());

        assert_eq!(builder.insertion_mode, InsertionMode::InCaption);
        let caption = builder.open_elements.current_node().unwrap();
        assert_eq!(
            builder.document.children(table).collect::<Vec<_>>(),
            vec![caption]
        );
        assert!(matches!(
            builder.active_formatting_elements.entries.last(),
            Some(super::FormattingEntry::Marker)
        ));
    }

    #[test]
    fn in_table_col_synthesizes_a_colgroup_and_reprocesses() {
        let mut builder = TreeBuilder::new();
        let (_, table) = bootstrap_in_table(&mut builder);

        builder.process_token(&start_tag("col"), pos());

        assert_eq!(builder.insertion_mode, InsertionMode::InColumnGroup);
        let colgroup = builder.document.children(table).next().unwrap();
        assert_eq!(
            builder.document.node(colgroup).kind,
            NodeKind::Element {
                name: "colgroup".to_owned(),
                namespace: Some(super::HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
        let col = builder.document.children(colgroup).next().unwrap();
        assert_eq!(
            builder.document.node(col).kind,
            NodeKind::Element {
                name: "col".to_owned(),
                namespace: Some(super::HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
        // The synthesized colgroup was popped back off after inserting
        // col; current node is the colgroup itself (still open).
        assert_eq!(builder.open_elements.current_node(), Some(colgroup));
    }

    #[test]
    fn in_table_tr_synthesizes_tbody_then_cascades_into_in_row() {
        let mut builder = TreeBuilder::new();
        let (_, table) = bootstrap_in_table(&mut builder);

        // A single process_token call cascades InTable -> InTableBody
        // (synthesize tbody, reprocess) -> InRow (insert the real tr).
        builder.process_token(&start_tag("tr"), pos());

        assert_eq!(builder.insertion_mode, InsertionMode::InRow);
        let tbody = builder.document.children(table).next().unwrap();
        assert_eq!(
            builder.document.node(tbody).kind,
            NodeKind::Element {
                name: "tbody".to_owned(),
                namespace: Some(super::HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
        let tr = builder.open_elements.current_node().unwrap();
        assert_eq!(
            builder.document.children(tbody).collect::<Vec<_>>(),
            vec![tr]
        );
    }

    #[test]
    fn in_table_nested_table_closes_the_first_and_starts_a_sibling() {
        let mut builder = TreeBuilder::new();
        let (body, table1) = bootstrap_in_table(&mut builder);

        builder.process_token(&start_tag("table"), pos());

        // The nested <table> does NOT nest: it implicitly closes the
        // first one (reset_the_insertion_mode_appropriately lands back
        // in "in body"), then "in body"'s own table rule creates a
        // fresh sibling table.
        assert_eq!(builder.insertion_mode, InsertionMode::InTable);
        let table2 = builder.open_elements.current_node().unwrap();
        assert_ne!(table1, table2);
        assert_eq!(
            builder.document.children(body).collect::<Vec<_>>(),
            vec![table1, table2]
        );
        assert_eq!(builder.document.children(table1).count(), 0);
    }

    #[test]
    fn in_table_whitespace_character_is_buffered_and_inserted_normally() {
        let mut builder = TreeBuilder::new();
        let (_, table) = bootstrap_in_table(&mut builder);

        builder.process_token(&TokenKind::Character(' '), pos());
        assert_eq!(builder.insertion_mode, InsertionMode::InTableText);

        // A non-character token (a start tag) flushes the buffer and
        // reprocesses itself.
        builder.process_token(&start_tag("caption"), pos());

        let children: Vec<_> = builder.document.children(table).collect();
        assert_eq!(
            builder.document.node(children[0]).kind,
            NodeKind::Text {
                content: " ".to_owned()
            }
        );
        assert_eq!(builder.insertion_mode, InsertionMode::InCaption);
    }

    #[test]
    fn in_table_non_whitespace_character_is_foster_parented_before_the_table() {
        let mut builder = TreeBuilder::new();
        let (body, table) = bootstrap_in_table(&mut builder);

        builder.process_token(&TokenKind::Character('x'), pos());
        builder.process_token(&start_tag("caption"), pos());

        // Foster-parented: the text lands as body's child immediately
        // before the table, not inside it.
        let body_children: Vec<_> = builder.document.children(body).collect();
        assert_eq!(body_children.len(), 2);
        assert_eq!(
            builder.document.node(body_children[0]).kind,
            NodeKind::Text {
                content: "x".to_owned()
            }
        );
        assert_eq!(body_children[1], table);
        assert_eq!(builder.document.children(table).count(), 1); // just the caption
    }

    #[test]
    fn in_caption_end_tag_closes_and_returns_to_in_table() {
        let mut builder = TreeBuilder::new();
        let (_, table) = bootstrap_in_table(&mut builder);
        builder.process_token(&start_tag("caption"), pos());
        let caption = builder.open_elements.current_node().unwrap();

        builder.process_token(&end_tag("caption"), pos());

        assert_eq!(builder.insertion_mode, InsertionMode::InTable);
        assert!(!builder.open_elements.contains(caption));
        assert_eq!(builder.open_elements.current_node(), Some(table));
    }

    #[test]
    fn in_column_group_non_whitespace_closes_and_reprocesses_in_table() {
        let mut builder = TreeBuilder::new();
        let (_, table) = bootstrap_in_table(&mut builder);
        builder.process_token(&start_tag("colgroup"), pos());
        let colgroup = builder.open_elements.current_node().unwrap();

        builder.process_token(&start_tag("tbody"), pos());

        assert_eq!(builder.insertion_mode, InsertionMode::InTableBody);
        assert!(!builder.open_elements.contains(colgroup));
        let tbody = builder.open_elements.current_node().unwrap();
        assert_eq!(
            builder.document.children(table).collect::<Vec<_>>(),
            vec![colgroup, tbody]
        );
    }

    #[test]
    fn in_table_body_end_tag_table_closes_tbody_and_the_table() {
        let mut builder = TreeBuilder::new();
        let (body, table) = bootstrap_in_table(&mut builder);
        builder.process_token(&start_tag("tbody"), pos());
        assert_eq!(builder.insertion_mode, InsertionMode::InTableBody);

        builder.process_token(&end_tag("table"), pos());

        assert!(!builder.open_elements.contains(table));
        assert_eq!(builder.open_elements.current_node(), Some(body));
    }

    #[test]
    fn in_row_td_switches_to_in_cell_and_pushes_a_marker() {
        let mut builder = TreeBuilder::new();
        bootstrap_in_table(&mut builder);
        builder.process_token(&start_tag("tr"), pos());
        assert_eq!(builder.insertion_mode, InsertionMode::InRow);
        let tr = builder.open_elements.current_node().unwrap();

        builder.process_token(&start_tag("td"), pos());

        assert_eq!(builder.insertion_mode, InsertionMode::InCell);
        let td = builder.open_elements.current_node().unwrap();
        assert_eq!(builder.document.children(tr).collect::<Vec<_>>(), vec![td]);
        assert!(matches!(
            builder.active_formatting_elements.entries.last(),
            Some(super::FormattingEntry::Marker)
        ));
    }

    #[test]
    fn in_row_end_tag_tr_returns_to_in_table_body() {
        let mut builder = TreeBuilder::new();
        bootstrap_in_table(&mut builder);
        builder.process_token(&start_tag("tr"), pos());
        let tr = builder.open_elements.current_node().unwrap();

        builder.process_token(&end_tag("tr"), pos());

        assert_eq!(builder.insertion_mode, InsertionMode::InTableBody);
        assert!(!builder.open_elements.contains(tr));
    }

    #[test]
    fn in_cell_end_tag_td_closes_and_returns_to_in_row() {
        let mut builder = TreeBuilder::new();
        bootstrap_in_table(&mut builder);
        builder.process_token(&start_tag("tr"), pos());
        let tr = builder.open_elements.current_node().unwrap();
        builder.process_token(&start_tag("td"), pos());
        let td = builder.open_elements.current_node().unwrap();

        builder.process_token(&end_tag("td"), pos());

        assert_eq!(builder.insertion_mode, InsertionMode::InRow);
        assert!(!builder.open_elements.contains(td));
        assert_eq!(builder.open_elements.current_node(), Some(tr));
    }

    #[test]
    fn in_cell_next_cell_start_tag_closes_the_current_one_and_reprocesses() {
        let mut builder = TreeBuilder::new();
        bootstrap_in_table(&mut builder);
        builder.process_token(&start_tag("tr"), pos());
        let tr = builder.open_elements.current_node().unwrap();
        builder.process_token(&start_tag("td"), pos());
        let td1 = builder.open_elements.current_node().unwrap();

        builder.process_token(&start_tag("td"), pos());

        assert_eq!(builder.insertion_mode, InsertionMode::InCell);
        let td2 = builder.open_elements.current_node().unwrap();
        assert_ne!(td1, td2);
        assert!(!builder.open_elements.contains(td1));
        assert_eq!(
            builder.document.children(tr).collect::<Vec<_>>(),
            vec![td1, td2]
        );
    }

    /// Drives a fresh builder into "after body" via `bootstrap_in_body`
    /// plus a `</body>` end tag. Returns `(html, body)`.
    fn bootstrap_after_body(builder: &mut TreeBuilder) -> (NodeId, NodeId) {
        let (html, body) = bootstrap_in_body(builder);
        builder.process_token(&end_tag("body"), pos());
        (html, body)
    }

    #[test]
    fn after_body_comment_is_appended_to_the_html_element() {
        let mut builder = TreeBuilder::new();
        let (html, body) = bootstrap_after_body(&mut builder);
        assert_eq!(builder.insertion_mode, InsertionMode::AfterBody);

        builder.process_token(&TokenKind::Comment("hi".to_owned()), pos());

        // html's children are [head, body, comment] — the comment is
        // appended last, after the already-present head and body.
        let html_children: Vec<_> = builder.document.children(html).collect();
        assert_eq!(html_children.len(), 3);
        assert_eq!(html_children[1], body);
        assert_eq!(
            builder.document.node(html_children[2]).kind,
            NodeKind::Comment {
                content: "hi".to_owned()
            }
        );
    }

    #[test]
    fn after_body_end_tag_html_switches_to_after_after_body() {
        let mut builder = TreeBuilder::new();
        bootstrap_after_body(&mut builder);

        builder.process_token(&end_tag("html"), pos());

        assert_eq!(builder.insertion_mode, InsertionMode::AfterAfterBody);
    }

    #[test]
    fn after_body_whitespace_character_is_delegated_to_in_body() {
        let mut builder = TreeBuilder::new();
        let (_, body) = bootstrap_after_body(&mut builder);

        builder.process_token(&TokenKind::Character(' '), pos());

        let children: Vec<_> = builder.document.children(body).collect();
        assert_eq!(children.len(), 1);
        assert_eq!(
            builder.document.node(children[0]).kind,
            NodeKind::Text {
                content: " ".to_owned()
            }
        );
        assert_eq!(builder.insertion_mode, InsertionMode::AfterBody);
    }

    #[test]
    fn after_body_anything_else_switches_to_in_body_and_reprocesses() {
        let mut builder = TreeBuilder::new();
        let (_, body) = bootstrap_after_body(&mut builder);

        builder.process_token(&start_tag("p"), pos());

        assert_eq!(builder.insertion_mode, InsertionMode::InBody);
        let p = builder.open_elements.current_node().unwrap();
        assert_eq!(builder.document.children(body).collect::<Vec<_>>(), vec![p]);
    }

    /// Drives a fresh builder into "after after body" via
    /// `bootstrap_after_body` plus a `</html>` end tag. Returns the
    /// document root.
    fn bootstrap_after_after_body(builder: &mut TreeBuilder) -> NodeId {
        bootstrap_after_body(builder);
        builder.process_token(&end_tag("html"), pos());
        builder.document.root()
    }

    #[test]
    fn after_after_body_comment_is_appended_to_the_document() {
        let mut builder = TreeBuilder::new();
        let root = bootstrap_after_after_body(&mut builder);

        builder.process_token(&TokenKind::Comment("hi".to_owned()), pos());

        let children: Vec<_> = builder.document.children(root).collect();
        let last = *children.last().unwrap();
        assert_eq!(
            builder.document.node(last).kind,
            NodeKind::Comment {
                content: "hi".to_owned()
            }
        );
    }

    #[test]
    fn after_after_body_whitespace_character_is_delegated_to_in_body() {
        let mut builder = TreeBuilder::new();
        bootstrap_after_after_body(&mut builder);

        builder.process_token(&TokenKind::Character(' '), pos());

        // Whitespace in "after after body" stays put (delegates to "in
        // body", which inserts it into the still-open body element) —
        // the insertion mode itself doesn't change for whitespace.
        assert_eq!(builder.insertion_mode, InsertionMode::AfterAfterBody);
    }

    #[test]
    fn after_after_body_anything_else_switches_to_in_body_and_reprocesses() {
        let mut builder = TreeBuilder::new();
        bootstrap_after_after_body(&mut builder);

        builder.process_token(&start_tag("p"), pos());

        assert_eq!(builder.insertion_mode, InsertionMode::InBody);
        let p = builder.open_elements.current_node().unwrap();
        assert_eq!(
            builder.document.node(p).kind,
            NodeKind::Element {
                name: "p".to_owned(),
                namespace: Some(super::HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
    }

    /// Drives a fresh builder into foreign content via `bootstrap_in_body`
    /// plus an `<svg>` start tag. Returns `(body, svg)`.
    fn bootstrap_in_svg(builder: &mut TreeBuilder) -> (NodeId, NodeId) {
        let (_, body) = bootstrap_in_body(builder);
        builder.process_token(&start_tag("svg"), pos());
        let svg = builder.open_elements.current_node().unwrap();
        (body, svg)
    }

    #[test]
    fn foreign_content_ordinary_svg_element_gets_svg_namespace() {
        let mut builder = TreeBuilder::new();
        let (_, svg) = bootstrap_in_svg(&mut builder);

        builder.process_token(&start_tag("circle"), pos());

        let circle = builder.open_elements.current_node().unwrap();
        assert_eq!(
            builder.document.node(circle).kind,
            NodeKind::Element {
                name: "circle".to_owned(),
                namespace: Some(super::SVG_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
        assert_eq!(
            builder.document.children(svg).collect::<Vec<_>>(),
            vec![circle]
        );
    }

    #[test]
    fn foreign_content_svg_tag_name_is_case_fixed() {
        let mut builder = TreeBuilder::new();
        bootstrap_in_svg(&mut builder);

        builder.process_token(&start_tag("foreignobject"), pos());

        let node = builder.open_elements.current_node().unwrap();
        let NodeKind::Element { name, .. } = &builder.document.node(node).kind else {
            unreachable!()
        };
        assert_eq!(name, "foreignObject");
    }

    #[test]
    fn foreign_content_svg_attribute_is_case_fixed() {
        let mut builder = TreeBuilder::new();
        bootstrap_in_svg(&mut builder);

        builder.process_token(
            &start_tag_with_attrs("rect", &[("viewbox", "0 0 1 1")]),
            pos(),
        );

        let node = builder.open_elements.current_node().unwrap();
        let NodeKind::Element { attributes, .. } = &builder.document.node(node).kind else {
            unreachable!()
        };
        assert_eq!(attributes[0].name, "viewBox");
    }

    #[test]
    fn foreign_content_xlink_attribute_gets_namespaced() {
        let mut builder = TreeBuilder::new();
        bootstrap_in_svg(&mut builder);

        builder.process_token(&start_tag_with_attrs("use", &[("xlink:href", "#a")]), pos());

        let node = builder.open_elements.current_node().unwrap();
        let NodeKind::Element { attributes, .. } = &builder.document.node(node).kind else {
            unreachable!()
        };
        assert_eq!(attributes[0].name, "xlink:href");
        assert_eq!(
            attributes[0].namespace.as_deref(),
            Some(super::XLINK_NAMESPACE)
        );
    }

    #[test]
    fn math_definitionurl_attribute_is_case_fixed() {
        let mut builder = TreeBuilder::new();
        bootstrap_in_body(&mut builder);

        builder.process_token(
            &start_tag_with_attrs("math", &[("definitionurl", "x")]),
            pos(),
        );

        let math = builder.open_elements.current_node().unwrap();
        let NodeKind::Element { attributes, .. } = &builder.document.node(math).kind else {
            unreachable!()
        };
        assert_eq!(attributes[0].name, "definitionURL");
    }

    #[test]
    fn foreign_content_html_integration_point_allows_html_rules_inside() {
        let mut builder = TreeBuilder::new();
        bootstrap_in_svg(&mut builder);
        builder.process_token(&start_tag("foreignobject"), pos());
        let foreign_object = builder.open_elements.current_node().unwrap();

        builder.process_token(&start_tag("div"), pos());

        let div = builder.open_elements.current_node().unwrap();
        assert_eq!(
            builder.document.node(div).kind,
            NodeKind::Element {
                name: "div".to_owned(),
                namespace: Some(super::HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
        assert_eq!(
            builder
                .document
                .children(foreign_object)
                .collect::<Vec<_>>(),
            vec![div]
        );
    }

    #[test]
    fn foreign_content_mathml_text_integration_point_allows_html_start_tags() {
        let mut builder = TreeBuilder::new();
        bootstrap_in_body(&mut builder);
        builder.process_token(&start_tag("math"), pos());
        builder.process_token(&start_tag("mi"), pos());
        let mi = builder.open_elements.current_node().unwrap();

        builder.process_token(&start_tag("b"), pos());

        let b = builder.open_elements.current_node().unwrap();
        assert_eq!(
            builder.document.node(b).kind,
            NodeKind::Element {
                name: "b".to_owned(),
                namespace: Some(super::HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
        assert_eq!(builder.document.children(mi).collect::<Vec<_>>(), vec![b]);
    }

    #[test]
    fn foreign_content_mathml_text_integration_point_still_treats_mglyph_as_foreign() {
        let mut builder = TreeBuilder::new();
        bootstrap_in_body(&mut builder);
        builder.process_token(&start_tag("math"), pos());
        builder.process_token(&start_tag("mi"), pos());

        builder.process_token(&start_tag("mglyph"), pos());

        let mglyph = builder.open_elements.current_node().unwrap();
        assert_eq!(
            builder.document.node(mglyph).kind,
            NodeKind::Element {
                name: "mglyph".to_owned(),
                namespace: Some(super::MATHML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
    }

    #[test]
    fn foreign_content_block_element_start_tag_pops_out_and_reprocesses_as_html() {
        let mut builder = TreeBuilder::new();
        let (body, svg) = bootstrap_in_svg(&mut builder);

        builder.process_token(&start_tag("b"), pos());

        // <b> is in the escape list: pops back out of <svg> entirely
        // and gets processed as ordinary HTML content instead.
        assert!(!builder.open_elements.contains(svg));
        let b = builder.open_elements.current_node().unwrap();
        assert_eq!(
            builder.document.node(b).kind,
            NodeKind::Element {
                name: "b".to_owned(),
                namespace: Some(super::HTML_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
        assert_eq!(
            builder.document.children(body).collect::<Vec<_>>(),
            vec![svg, b]
        );
    }

    #[test]
    fn foreign_content_end_tag_p_pops_out_of_foreign_content() {
        let mut builder = TreeBuilder::new();
        let (_, svg) = bootstrap_in_svg(&mut builder);

        builder.process_token(&end_tag("p"), pos());

        assert!(!builder.open_elements.contains(svg));
        assert_eq!(builder.insertion_mode, InsertionMode::InBody);
    }

    #[test]
    fn foreign_content_nul_character_becomes_replacement_character() {
        let mut builder = TreeBuilder::new();
        let (_, svg) = bootstrap_in_svg(&mut builder);

        builder.process_token(&TokenKind::Character('\0'), pos());

        let children: Vec<_> = builder.document.children(svg).collect();
        assert_eq!(
            builder.document.node(children[0]).kind,
            NodeKind::Text {
                content: "\u{FFFD}".to_owned()
            }
        );
    }

    #[test]
    fn foreign_content_any_other_end_tag_pops_through_a_matching_element() {
        let mut builder = TreeBuilder::new();
        let (_, svg) = bootstrap_in_svg(&mut builder);
        builder.process_token(&start_tag("circle"), pos());
        let circle = builder.open_elements.current_node().unwrap();

        builder.process_token(&end_tag("circle"), pos());

        assert!(!builder.open_elements.contains(circle));
        assert_eq!(builder.open_elements.current_node(), Some(svg));
    }

    #[test]
    fn foreign_content_math_inside_svg_keeps_the_svg_namespace() {
        // <math> is only special-cased by "in body"'s own dispatch,
        // which never runs while already inside foreign content — a
        // literal <math> tag nested inside <svg> (not at an
        // integration point) is just "any other start tag", inserted
        // into the *current* (SVG) namespace like anything else.
        let mut builder = TreeBuilder::new();
        bootstrap_in_svg(&mut builder);

        builder.process_token(&start_tag("math"), pos());

        let math = builder.open_elements.current_node().unwrap();
        assert_eq!(
            builder.document.node(math).kind,
            NodeKind::Element {
                name: "math".to_owned(),
                namespace: Some(super::SVG_NAMESPACE.to_owned()),
                attributes: vec![],
            }
        );
    }
}
