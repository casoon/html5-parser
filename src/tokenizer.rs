// WHATWG HTML5 tokenizer state machine
// (https://html.spec.whatwg.org/multipage/parsing.html#tokenization).
//
// The full state machine is implemented: tag/attribute/text tokenization,
// character references, comments, DOCTYPE, processing instructions,
// RCDATA/RAWTEXT/PLAINTEXT/script-data (including escaped/double-escaped),
// and CDATA sections. See plan/02-tokenizer.md.

use std::collections::VecDeque;

use crate::entities;

/// A source position: where a node parsed from the input started.
/// Matches `html-conform::finding::SourceLocation`'s field layout exactly
/// so the eventual integration (Phase 05) needs no conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    /// One-based line number.
    pub line: u32,
    /// One-based column number.
    pub column: u32,
    /// Zero-based byte offset.
    pub byte_offset: usize,
}

/// A single WHATWG "parse error" (§13.2.2) — a point where the input
/// deviated from strict grammar but the tokenizer still recovered per its
/// own well-defined algorithm. Never fatal: [`crate::Document`]
/// construction always completes regardless of how many of these occur.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseError {
    pub kind: ParseErrorKind,
    pub position: Position,
}

/// Which WHATWG "parse error" (§13.2.2) occurred. Variant names mirror
/// the spec's own kebab-case error identifiers, translated to
/// PascalCase. Only variants this crate actually detects and reports
/// exist — no catch-all/string-payload variant, so matching on a
/// specific kind stays meaningful. `#[non_exhaustive]` because more
/// variants are expected in follow-up phases (`plan/07-parse-errors.md`:
/// tokenizer-level errors only so far — tree-construction-level errors,
/// e.g. stray end tags across the whole document, are follow-up work,
/// not yet represented here) — adding one later must not be a breaking
/// change for any caller matching on this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParseErrorKind {
    // Markup declaration open / comments (§13.2.5.42, .45–.52)
    CdataInHtmlContent,
    IncorrectlyOpenedComment,
    AbruptClosingOfEmptyComment,
    NestedComment,
    IncorrectlyClosedComment,
    EofInComment,
    // Tag open / attributes (§13.2.5.6–.41)
    InvalidFirstCharacterOfTagName,
    EofBeforeTagName,
    MissingEndTagName,
    EofInTag,
    DuplicateAttribute,
    UnexpectedNullCharacter,
    UnexpectedCharacterInAttributeName,
    MissingAttributeValue,
    UnexpectedCharacterInUnquotedAttributeValue,
    MissingWhitespaceBetweenAttributes,
    UnexpectedSolidusInTag,
    UnexpectedEqualsSignBeforeAttributeName,
    // Character references (§13.2.5.77–.84)
    UnknownNamedCharacterReference,
    AbsenceOfDigitsInNumericCharacterReference,
    MissingSemicolonAfterCharacterReference,
    NullCharacterReference,
    CharacterReferenceOutsideUnicodeRange,
    SurrogateCharacterReference,
    NoncharacterCharacterReference,
    ControlCharacterReference,
    // DOCTYPE (§13.2.5.53–.68)
    MissingWhitespaceBeforeDoctypeName,
    MissingDoctypeName,
    InvalidCharacterSequenceAfterDoctypeName,
    MissingWhitespaceBetweenDoctypePublicAndSystemIdentifiers,
    UnexpectedCharacterAfterDoctypeSystemIdentifier,
    EofInDoctype,
    // Processing instructions (§13.2.5.73–.76)
    EofInProcessingInstruction,
    InvalidFirstCharacterOfProcessingInstructionTarget,
    InvalidProcessingInstructionTarget,
    DisallowedProcessingInstructionTarget,
    // Text content (§13.2.5.x script-data-like/CDATA states)
    EofInScriptHtmlCommentLikeText,
    EofInCdata,
}

/// A single `name=value` attribute on a start or end tag token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Attribute {
    pub(crate) name: String,
    pub(crate) value: String,
}

/// A start or end tag token (§13.2.5: both share the same field set — the
/// tokenizer does not distinguish their meaning further, that is
/// tree-construction's job).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TagToken {
    pub(crate) name: String,
    pub(crate) self_closing: bool,
    pub(crate) attributes: Vec<Attribute>,
}

/// A DOCTYPE token.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DoctypeToken {
    pub(crate) name: Option<String>,
    pub(crate) public_identifier: Option<String>,
    pub(crate) system_identifier: Option<String>,
    pub(crate) force_quirks: bool,
}

/// A processing instruction token (§13.2.5.72–.76 — target and data,
/// distinct from a comment). `html-conform`'s `normalize()` drops
/// processing-instruction nodes downstream (see its doc comment), but the
/// tokenizer models the token faithfully regardless — that's a
/// tree-construction/adapter-layer decision, not this layer's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcessingInstructionToken {
    pub(crate) target: String,
    pub(crate) data: String,
}

/// The kinds of token the tokenizer emits (§13.2.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TokenKind {
    Doctype(DoctypeToken),
    StartTag(TagToken),
    EndTag(TagToken),
    Comment(String),
    ProcessingInstruction(ProcessingInstructionToken),
    /// A single character. Runs of adjacent character tokens are merged
    /// into text nodes by tree-construction/the adapter layer, not here —
    /// analogous to `html-conform::infoset::merge_text_and_comment_runs`.
    Character(char),
    Eof,
}

/// A token together with its start position in the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Token {
    pub(crate) kind: TokenKind,
    pub(crate) position: Position,
}

/// The tokenizer's current state (§13.2.5). Only the states needed for
/// tag/attribute/text tokenization are implemented so far — see the module
/// header and plan/02-tokenizer.md for what is still missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Data,
    TagOpen,
    EndTagOpen,
    TagName,
    BeforeAttributeName,
    AttributeName,
    AfterAttributeName,
    BeforeAttributeValue,
    AttributeValueDoubleQuoted,
    AttributeValueSingleQuoted,
    AttributeValueUnquoted,
    AfterAttributeValueQuoted,
    SelfClosingStartTag,
    CharacterReference,
    NamedCharacterReference,
    AmbiguousAmpersand,
    NumericCharacterReference,
    HexadecimalCharacterReferenceStart,
    HexadecimalCharacterReference,
    DecimalCharacterReference,
    NumericCharacterReferenceEnd,
    MarkupDeclarationOpen,
    BogusComment,
    CommentStart,
    CommentStartDash,
    Comment,
    CommentLessThanSign,
    CommentLessThanSignBang,
    CommentLessThanSignBangDash,
    CommentLessThanSignBangDashDash,
    CommentEndDash,
    CommentEnd,
    CommentEndBang,
    Doctype,
    BeforeDoctypeName,
    DoctypeName,
    AfterDoctypeName,
    AfterDoctypePublicKeyword,
    BeforeDoctypePublicIdentifier,
    DoctypePublicIdentifierDoubleQuoted,
    DoctypePublicIdentifierSingleQuoted,
    AfterDoctypePublicIdentifier,
    BetweenDoctypePublicAndSystemIdentifiers,
    AfterDoctypeSystemKeyword,
    BeforeDoctypeSystemIdentifier,
    DoctypeSystemIdentifierDoubleQuoted,
    DoctypeSystemIdentifierSingleQuoted,
    AfterDoctypeSystemIdentifier,
    BogusDoctype,
    ProcessingInstructionOpen,
    ProcessingInstructionTarget,
    AfterProcessingInstructionTarget,
    ProcessingInstructionData,
    ProcessingInstructionQuestionable,
    RcData,
    RcDataLessThanSign,
    RcDataEndTagOpen,
    RcDataEndTagName,
    RawText,
    RawTextLessThanSign,
    RawTextEndTagOpen,
    RawTextEndTagName,
    PlainText,
    ScriptData,
    ScriptDataLessThanSign,
    ScriptDataEndTagOpen,
    ScriptDataEndTagName,
    ScriptDataEscapeStart,
    ScriptDataEscapeStartDash,
    ScriptDataEscaped,
    ScriptDataEscapedDash,
    ScriptDataEscapedDashDash,
    ScriptDataEscapedLessThanSign,
    ScriptDataEscapedEndTagOpen,
    ScriptDataEscapedEndTagName,
    ScriptDataDoubleEscapeStart,
    ScriptDataDoubleEscaped,
    ScriptDataDoubleEscapedDash,
    ScriptDataDoubleEscapedDashDash,
    ScriptDataDoubleEscapedLessThanSign,
    ScriptDataDoubleEscapeEnd,
    CdataSection,
    CdataSectionBracket,
    CdataSectionEnd,
}

/// The tokenizer states reachable only via explicit external signaling
/// from tree-construction (§13.2.6), never decided by the tokenizer
/// itself: entering RCDATA/RAWTEXT/script-data/PLAINTEXT depends on which
/// HTML element was just inserted (`<title>`/`<textarea>` → `RcData`,
/// `<style>`/`<xmp>`/`<iframe>`/`<noembed>`/`<noframes>` → `RawText`,
/// `<script>` → `ScriptData`, `<plaintext>` → `PlainText`), knowledge the
/// tokenizer deliberately does not have — see plan/02-tokenizer.md's
/// Normative Grundlage.
///
/// There is deliberately no matching "switch back" method: leaving these
/// states again is entirely tokenizer-internal, driven by the
/// "appropriate end tag token" mechanism (§13.2.5, based on the last
/// start tag *this tokenizer* emitted, tracked in `last_start_tag_name`)
/// — tree-construction has no say in it. `PlainText` never leaves at all
/// (a one-way trip, per spec there is no returning state). `ScriptData`'s
/// internal escaped/double-escaped sub-states are entirely
/// self-contained too — the `<!--`/`<script`/`</script`-in-script-data
/// dance (§13.2.5.18–.31) never needs external input either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExternalState {
    RcData,
    RawText,
    ScriptData,
    PlainText,
}

/// Which of a DOCTYPE token's two quoted identifiers is currently being
/// consumed — shared by the (double-quoted)/(single-quoted) public/system
/// identifier states, analogous to how `step_attribute_value_quoted`
/// shares one function across the two attribute-value quoting states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DoctypeIdentifierKind {
    Public,
    System,
}

/// Where attribute-value characters currently being consumed should go:
/// either into a real attribute on the current tag token, or nowhere.
/// §13.2.5's duplicate-attribute rule: a duplicate attribute's value is
/// still parsed (to keep the tokenizer in sync with the input), but
/// discarded rather than kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttributeValueTarget {
    Index(usize),
    Discarded,
}

/// The WHATWG HTML5 tokenizer (§13.2.5). Consumes a whole input string up
/// front — character-reference/foreign-content switching does not need a
/// byte-level streaming reader for this crate's Step-1 scope — and yields
/// [`Token`]s one at a time via [`Iterator`].
pub(crate) struct Tokenizer {
    chars: Vec<char>,
    positions: Vec<Position>,
    index: usize,
    /// Set when a state reconsumes the just-processed character: the next
    /// `consume()` call returns this instead of advancing.
    saved: Option<(Option<char>, Position)>,
    state: State,
    current_tag: Option<TagToken>,
    current_tag_is_end: bool,
    /// Position of the `<` that started the tag token currently being
    /// built (or, in a malformed-markup error path, the `<` being
    /// re-emitted as a plain character token).
    current_tag_start: Position,
    /// Position of the `/` that started the current end-tag-open attempt —
    /// only needed to place the synthesized `/` character token correctly
    /// in the rare `eof-before-tag-name` case (input ending in `</`).
    slash_position: Position,
    current_attribute_name: String,
    attribute_value_target: AttributeValueTarget,
    /// The state to switch back to once the current character reference
    /// (§13.2.5.77–.84) has been resolved.
    return_state: State,
    /// Position of the `&` that started the character reference attempt
    /// currently in progress.
    character_reference_start: Position,
    /// Index into `chars` of that same `&`.
    character_reference_start_index: usize,
    /// Accumulator for `&#...;`/`&#x...;` numeric character references.
    character_reference_code: u32,
    /// The comment token currently being built (bogus-comment or real
    /// comment states). Its start position reuses `current_tag_start`:
    /// only one of {tag, comment} is ever in progress at a time, both
    /// always start at the same `<` the Data state saw.
    current_comment_data: String,
    /// The DOCTYPE token currently being built. Its start position also
    /// reuses `current_tag_start`, same reasoning as `current_comment_data`.
    current_doctype: Option<DoctypeToken>,
    /// Accumulates a processing instruction's target name (§13.2.5.73)
    /// before the PI token itself exists yet.
    pi_temporary_buffer: String,
    /// The processing instruction token currently being built, once its
    /// target is known. Start position also reuses `current_tag_start`.
    current_processing_instruction: Option<ProcessingInstructionToken>,
    /// The tag name of the last *start* tag this tokenizer emitted —
    /// drives the "appropriate end tag token" check (§13.2.5) that
    /// RCDATA/RAWTEXT/script-data end-tag-name states use to decide
    /// whether a `</...>`-looking sequence is a real end tag or just more
    /// text. `None` until the first start tag is emitted (per spec: "If
    /// no start tag has been emitted from this tokenizer, then no end tag
    /// token is appropriate").
    last_start_tag_name: Option<String>,
    /// Scratch buffer for RCDATA/RAWTEXT/script-data's end-tag-name
    /// states: accumulates the possible end tag's name so it can be
    /// flushed back out as literal text if it turns out not to be an
    /// appropriate end tag token.
    text_end_tag_buffer: String,
    /// Set by the tree-builder (Phase 03) whenever the "adjusted current
    /// node" changes — whether it is currently in a non-HTML namespace.
    /// Consulted only by `run_markup_declaration_open`'s `[CDATA[` branch
    /// (§13.2.5.42), which needs this as a synchronous fact at the moment
    /// of the match, not as a persisted mode switch like `switch_to()`.
    /// Defaults to `false` (never foreign), correct for any document with
    /// no foreign content.
    in_foreign_content: bool,
    /// Positions of the `]` characters currently withheld while
    /// CDATA-section-bracket/-end (§13.2.5.70/.71) figure out whether
    /// they're part of the `]]>` terminator or just content — never more
    /// than 2 entries, oldest first. Needed because those states don't
    /// emit a token for a `]` the moment they consume it (unlike almost
    /// everything else in this tokenizer): whether/when a withheld `]`
    /// becomes a real character token depends on what follows it.
    cdata_pending_brackets: Vec<Position>,
    pending: VecDeque<Token>,
    eof_returned: bool,
    /// Accumulated [`ParseError`]s, in the order encountered. Drained
    /// once by [`Tokenizer::take_errors`] after tokenization finishes
    /// (see `lib.rs::parse`) — never read mid-stream, so plain
    /// accumulation (not a queue like `pending`) is enough.
    errors: Vec<ParseError>,
}

impl Tokenizer {
    pub(crate) fn new(input: &str) -> Self {
        // §13.2.3.5 "Preprocessing the input stream": normalize CRLF and
        // lone CR to a single LF before tokenization. `byte_offset` tracks
        // the *original* input's byte offsets (not the normalized
        // stream's), so positions stay meaningful against the source text
        // callers actually have.
        let mut chars = Vec::new();
        let mut positions = Vec::new();
        let mut line = 1u32;
        let mut column = 1u32;
        let mut iter = input.char_indices().peekable();
        while let Some((byte_offset, c)) = iter.next() {
            let (emitted, skip_next) = if c == '\r' {
                ('\n', matches!(iter.peek(), Some((_, '\n'))))
            } else {
                (c, false)
            };
            positions.push(Position {
                line,
                column,
                byte_offset,
            });
            chars.push(emitted);
            if emitted == '\n' {
                line += 1;
                column = 1;
            } else {
                column += 1;
            }
            if skip_next {
                iter.next();
            }
        }
        positions.push(Position {
            line,
            column,
            byte_offset: input.len(),
        });

        let origin = Position {
            line: 1,
            column: 1,
            byte_offset: 0,
        };
        Tokenizer {
            chars,
            positions,
            index: 0,
            saved: None,
            state: State::Data,
            current_tag: None,
            current_tag_is_end: false,
            current_tag_start: origin,
            slash_position: origin,
            current_attribute_name: String::new(),
            attribute_value_target: AttributeValueTarget::Discarded,
            return_state: State::Data,
            character_reference_start: origin,
            character_reference_start_index: 0,
            character_reference_code: 0,
            current_comment_data: String::new(),
            current_doctype: None,
            pi_temporary_buffer: String::new(),
            current_processing_instruction: None,
            last_start_tag_name: None,
            text_end_tag_buffer: String::new(),
            in_foreign_content: false,
            cdata_pending_brackets: Vec::new(),
            pending: VecDeque::new(),
            eof_returned: false,
            errors: Vec::new(),
        }
    }

    /// Records a [`ParseError`] at `position`. Called at every point in
    /// the state machine below marked with a `// <kebab-case-name> parse
    /// error.` comment (identified during Phase 02's spec research,
    /// implemented in Phase 07 — see `plan/07-parse-errors.md`).
    fn error(&mut self, kind: ParseErrorKind, position: Position) {
        self.errors.push(ParseError { kind, position });
    }

    /// Drains and returns every [`ParseError`] recorded so far. Called
    /// once by `lib.rs::parse` after tokenization finishes.
    pub(crate) fn take_errors(&mut self) -> Vec<ParseError> {
        std::mem::take(&mut self.errors)
    }

    /// Called by the tree-builder (Phase 03) right after it inserts an
    /// element that switches the tokenizer's content model. See
    /// [`ExternalState`] for which element maps to which state and why
    /// there is no matching "switch back" method.
    pub(crate) fn switch_to(&mut self, state: ExternalState) {
        self.state = match state {
            ExternalState::RcData => State::RcData,
            ExternalState::RawText => State::RawText,
            ExternalState::ScriptData => State::ScriptData,
            ExternalState::PlainText => State::PlainText,
        };
    }

    /// Called by the tree-builder (Phase 03) whenever the "adjusted
    /// current node" changes (i.e. whenever the stack of open elements is
    /// pushed or popped) — see the `in_foreign_content` field doc.
    pub(crate) fn set_in_foreign_content(&mut self, in_foreign_content: bool) {
        self.in_foreign_content = in_foreign_content;
    }

    fn consume(&mut self) -> (Option<char>, Position) {
        if let Some(saved) = self.saved.take() {
            return saved;
        }
        let position = self.positions[self.index];
        let ch = self.chars.get(self.index).copied();
        if ch.is_some() {
            self.index += 1;
        }
        (ch, position)
    }

    fn is_whitespace(c: char) -> bool {
        matches!(c, '\t' | '\n' | '\x0C' | ' ')
    }

    /// Starts a new tag token. `current_tag_start` must already hold the
    /// position of the `<` that introduced it (set in the Data state).
    fn start_tag_token(&mut self, is_end: bool) {
        self.current_tag = Some(TagToken {
            name: String::new(),
            self_closing: false,
            attributes: Vec::new(),
        });
        self.current_tag_is_end = is_end;
    }

    fn emit_tag(&mut self, out: &mut Vec<Token>) {
        let tag = self
            .current_tag
            .take()
            .expect("emit_tag called with no tag token in progress");
        let kind = if self.current_tag_is_end {
            TokenKind::EndTag(tag)
        } else {
            // "appropriate end tag token" (§13.2.5) is defined against the
            // last *start* tag emitted, so only start tags update it.
            self.last_start_tag_name = Some(tag.name.clone());
            TokenKind::StartTag(tag)
        };
        out.push(Token {
            kind,
            position: self.current_tag_start,
        });
    }

    /// Switches to the Data state and emits the current tag token — the
    /// common "`>` closes the tag" pattern shared by tag-name/attribute
    /// states.
    fn close_tag(&mut self, out: &mut Vec<Token>) -> bool {
        self.state = State::Data;
        self.emit_tag(out);
        false
    }

    fn emit_comment(&mut self, out: &mut Vec<Token>) {
        let data = std::mem::take(&mut self.current_comment_data);
        out.push(Token {
            kind: TokenKind::Comment(data),
            position: self.current_tag_start,
        });
    }

    fn current_doctype_mut(&mut self) -> &mut DoctypeToken {
        self.current_doctype
            .as_mut()
            .expect("doctype state reached with no doctype token in progress")
    }

    fn doctype_identifier_mut(&mut self, kind: DoctypeIdentifierKind) -> &mut String {
        let doctype = self.current_doctype_mut();
        let field = match kind {
            DoctypeIdentifierKind::Public => &mut doctype.public_identifier,
            DoctypeIdentifierKind::System => &mut doctype.system_identifier,
        };
        field
            .as_mut()
            .expect("doctype identifier appended to before being set to Some")
    }

    /// Sets the current DOCTYPE token's public identifier to the empty
    /// string (not missing) and switches to `quote_state` — the common
    /// "public identifier starts here" pattern shared by
    /// `AfterDoctypePublicKeyword`/`BeforeDoctypePublicIdentifier`'s `"`/`'`
    /// branches.
    fn start_doctype_public_identifier(&mut self, quote_state: State) -> bool {
        self.current_doctype_mut().public_identifier = Some(String::new());
        self.state = quote_state;
        false
    }

    /// Same as [`start_doctype_public_identifier`](Self::start_doctype_public_identifier),
    /// for the system identifier — shared by
    /// `AfterDoctypePublicIdentifier`/`BetweenDoctypePublicAndSystemIdentifiers`/
    /// `AfterDoctypeSystemKeyword`/`BeforeDoctypeSystemIdentifier`'s `"`/`'`
    /// branches.
    fn start_doctype_system_identifier(&mut self, quote_state: State) -> bool {
        self.current_doctype_mut().system_identifier = Some(String::new());
        self.state = quote_state;
        false
    }

    fn emit_doctype(&mut self, out: &mut Vec<Token>) {
        let doctype = self
            .current_doctype
            .take()
            .expect("emit_doctype called with no doctype token in progress");
        out.push(Token {
            kind: TokenKind::Doctype(doctype),
            position: self.current_tag_start,
        });
    }

    /// Switches to the Data state and emits the current DOCTYPE token
    /// as-is (`force_quirks` untouched).
    fn close_doctype(&mut self, out: &mut Vec<Token>) -> bool {
        self.state = State::Data;
        self.emit_doctype(out);
        false
    }

    /// Sets `force_quirks`, then behaves like `close_doctype` — the
    /// common "premature `>`" pattern across most DOCTYPE sub-states.
    fn close_doctype_with_quirks(&mut self, out: &mut Vec<Token>) -> bool {
        self.current_doctype_mut().force_quirks = true;
        self.close_doctype(out)
    }

    /// The common `eof-in-doctype` handling shared by every DOCTYPE
    /// sub-state *except* the DOCTYPE state itself (13.2.5.53), which is
    /// reached before any token exists yet and so creates one first
    /// instead of assuming one is already in progress (that site reports
    /// the same error itself, see its own call site).
    fn eof_in_doctype(&mut self, out: &mut Vec<Token>, position: Position) -> bool {
        self.error(ParseErrorKind::EofInDoctype, position);
        self.current_doctype_mut().force_quirks = true;
        self.emit_doctype(out);
        push_eof(out, position);
        false
    }

    /// The common "unexpected character here" pattern across most DOCTYPE
    /// sub-states: sets `force_quirks` and reconsumes in the bogus DOCTYPE
    /// state.
    fn bogus_doctype_with_quirks(&mut self) -> bool {
        self.current_doctype_mut().force_quirks = true;
        self.state = State::BogusDoctype;
        true
    }

    fn emit_processing_instruction(&mut self, out: &mut Vec<Token>) {
        let pi = self.current_processing_instruction.take().expect(
            "emit_processing_instruction called with no processing instruction token in progress",
        );
        out.push(Token {
            kind: TokenKind::ProcessingInstruction(pi),
            position: self.current_tag_start,
        });
    }

    /// §13.2.5's "convert the temporary buffer to a comment": a
    /// processing instruction with an invalid/disallowed target is
    /// instead treated as a bogus comment whose data is "?" followed by
    /// whatever was accumulated in `pi_temporary_buffer` so far.
    fn convert_pi_temporary_buffer_to_comment(&mut self) {
        let target = std::mem::take(&mut self.pi_temporary_buffer);
        self.current_comment_data = format!("?{target}");
        self.state = State::BogusComment;
    }

    /// True if the upcoming input, starting at `self.index`, matches
    /// `literal` character-for-character (or ASCII-case-insensitively, if
    /// requested) — without consuming anything.
    fn peek_matches(&self, literal: &str, case_insensitive: bool) -> bool {
        self.peek_matches_at(self.index, literal, case_insensitive)
    }

    /// Like `peek_matches`, but starting at an arbitrary index rather than
    /// `self.index` — needed where the lookahead window starts at an
    /// already-consumed character (e.g. after DOCTYPE state's "the six
    /// characters starting from the current input character").
    fn peek_matches_at(&self, start: usize, literal: &str, case_insensitive: bool) -> bool {
        literal.chars().enumerate().all(|(offset, expected)| {
            self.chars.get(start + offset).is_some_and(|&actual| {
                if case_insensitive {
                    actual.eq_ignore_ascii_case(&expected)
                } else {
                    actual == expected
                }
            })
        })
    }

    /// §13.2.5.42 "Markup declaration open state": branches on
    /// multi-character lookahead rather than a single consumed character,
    /// so — like the named character reference state — it is implemented
    /// as its own routine outside the one-character-per-`step()`-call
    /// model, dispatched from `run_until_token`. Never itself emits a
    /// token, only ever changes `self.state` (and consumes 0+ characters).
    fn run_markup_declaration_open(&mut self) {
        if self.peek_matches("--", false) {
            self.index += 2;
            self.current_comment_data.clear();
            self.state = State::CommentStart;
            return;
        }
        if self.peek_matches("DOCTYPE", true) {
            self.index += 7;
            self.state = State::Doctype;
            return;
        }
        if self.peek_matches("[CDATA[", false) {
            self.index += 7;
            if self.in_foreign_content {
                self.state = State::CdataSection;
            } else {
                // cdata-in-html-content parse error.
                self.error(
                    ParseErrorKind::CdataInHtmlContent,
                    self.positions[self.index],
                );
                self.current_comment_data = "[CDATA[".to_owned();
                self.state = State::BogusComment;
            }
            return;
        }
        // incorrectly-opened-comment parse error; don't consume anything.
        self.error(
            ParseErrorKind::IncorrectlyOpenedComment,
            self.positions[self.index],
        );
        self.current_comment_data.clear();
        self.state = State::BogusComment;
    }

    /// §13.2.5's duplicate-attribute rule, applied exactly once, when
    /// leaving the attribute name state: commits the just-built attribute
    /// name onto the current tag token (or discards it, if it duplicates
    /// an already-present name), and points `attribute_value_target` at
    /// where any following value characters should go.
    fn commit_attribute_name(&mut self, position: Position) {
        let name = std::mem::take(&mut self.current_attribute_name);
        let tag = self
            .current_tag
            .as_mut()
            .expect("commit_attribute_name called with no tag token in progress");
        if tag
            .attributes
            .iter()
            .any(|attribute| attribute.name == name)
        {
            // duplicate-attribute parse error: this attribute (and its
            // value, once parsed) is discarded — the earlier one wins.
            self.error(ParseErrorKind::DuplicateAttribute, position);
            self.attribute_value_target = AttributeValueTarget::Discarded;
        } else {
            tag.attributes.push(Attribute {
                name,
                value: String::new(),
            });
            self.attribute_value_target = AttributeValueTarget::Index(tag.attributes.len() - 1);
        }
    }

    fn push_attribute_value_char(&mut self, c: char) {
        if let AttributeValueTarget::Index(i) = self.attribute_value_target {
            self.current_tag
                .as_mut()
                .expect("push_attribute_value_char called with no tag token in progress")
                .attributes[i]
                .value
                .push(c);
        }
    }

    fn run_until_token(&mut self) {
        loop {
            // §13.2.5.78 "Named character reference state" is a
            // maximal-munch lookup against the whole named-character-
            // references table, not a single-character transition — it
            // gets its own non-consuming-loop handling rather than being
            // forced through the one-character-at-a-time `step()` model.
            if self.state == State::NamedCharacterReference {
                self.run_named_character_reference();
                if !self.pending.is_empty() {
                    return;
                }
                continue;
            }
            // §13.2.5.42 "Markup declaration open state" branches on
            // multi-character lookahead ("if the next few characters
            // are...") rather than consuming one character at a time, and
            // never itself emits a token — same non-consuming-loop
            // reasoning as the named character reference state above.
            if self.state == State::MarkupDeclarationOpen {
                self.run_markup_declaration_open();
                continue;
            }
            let (ch, position) = self.consume();
            let mut out = Vec::new();
            let reconsume = self.step(ch, position, &mut out);
            if reconsume {
                self.saved = Some((ch, position));
            }
            if !out.is_empty() {
                self.pending.extend(out);
                return;
            }
        }
    }

    /// Processes one input character (or EOF, as `None`) under the current
    /// state. Returns `true` if `ch` must be reprocessed under the
    /// (possibly just-changed) state — "reconsume" in spec terms.
    fn step(&mut self, ch: Option<char>, position: Position, out: &mut Vec<Token>) -> bool {
        match self.state {
            State::Data => match ch {
                Some('&') => {
                    self.begin_character_reference(State::Data, position);
                    false
                }
                Some('<') => {
                    self.current_tag_start = position;
                    self.state = State::TagOpen;
                    false
                }
                Some('\0') => {
                    // unexpected-null-character parse error, but — unlike
                    // RCDATA/RAWTEXT/script-data — the Data state does
                    // *not* replace it with U+FFFD, per spec.
                    self.error(ParseErrorKind::UnexpectedNullCharacter, position);
                    push_character(out, '\0', position);
                    false
                }
                Some(c) => {
                    push_character(out, c, position);
                    false
                }
                None => {
                    push_eof(out, position);
                    false
                }
            },
            State::TagOpen => match ch {
                Some('!') => {
                    self.state = State::MarkupDeclarationOpen;
                    false
                }
                Some('/') => {
                    self.slash_position = position;
                    self.state = State::EndTagOpen;
                    false
                }
                Some(c) if c.is_ascii_alphabetic() => {
                    self.start_tag_token(false);
                    self.state = State::TagName;
                    true
                }
                Some('?') => {
                    self.pi_temporary_buffer.clear();
                    self.state = State::ProcessingInstructionOpen;
                    false
                }
                Some(_) => {
                    // invalid-first-character-of-tag-name parse error.
                    self.error(ParseErrorKind::InvalidFirstCharacterOfTagName, position);
                    push_character(out, '<', self.current_tag_start);
                    self.state = State::Data;
                    true
                }
                None => {
                    // eof-before-tag-name parse error.
                    self.error(ParseErrorKind::EofBeforeTagName, position);
                    push_character(out, '<', self.current_tag_start);
                    push_eof(out, position);
                    false
                }
            },
            State::EndTagOpen => match ch {
                Some(c) if c.is_ascii_alphabetic() => {
                    self.start_tag_token(true);
                    self.state = State::TagName;
                    true
                }
                Some('>') => {
                    // missing-end-tag-name parse error.
                    self.error(ParseErrorKind::MissingEndTagName, position);
                    self.state = State::Data;
                    false
                }
                Some(_) => {
                    // invalid-first-character-of-tag-name parse error.
                    self.error(ParseErrorKind::InvalidFirstCharacterOfTagName, position);
                    self.current_comment_data.clear();
                    self.state = State::BogusComment;
                    true
                }
                None => {
                    // eof-before-tag-name parse error.
                    self.error(ParseErrorKind::EofBeforeTagName, position);
                    push_character(out, '<', self.current_tag_start);
                    push_character(out, '/', self.slash_position);
                    push_eof(out, position);
                    false
                }
            },
            State::TagName => match ch {
                Some(c) if Self::is_whitespace(c) => {
                    self.state = State::BeforeAttributeName;
                    false
                }
                Some('/') => {
                    self.state = State::SelfClosingStartTag;
                    false
                }
                Some('>') => self.close_tag(out),
                Some(c) if c.is_ascii_uppercase() => {
                    self.current_tag_mut().name.push(c.to_ascii_lowercase());
                    false
                }
                Some('\0') => {
                    self.current_tag_mut().name.push('\u{FFFD}');
                    false
                }
                Some(c) => {
                    self.current_tag_mut().name.push(c);
                    false
                }
                None => {
                    // eof-in-tag parse error: no tag token is emitted.
                    self.error(ParseErrorKind::EofInTag, position);
                    push_eof(out, position);
                    false
                }
            },
            State::BeforeAttributeName => match ch {
                Some(c) if Self::is_whitespace(c) => false,
                Some('/') | Some('>') | None => {
                    self.state = State::AfterAttributeName;
                    true
                }
                Some('=') => {
                    // unexpected-equals-sign-before-attribute-name parse
                    // error, but still starts an attribute literally
                    // named "=".
                    self.error(
                        ParseErrorKind::UnexpectedEqualsSignBeforeAttributeName,
                        position,
                    );
                    self.current_attribute_name.clear();
                    self.current_attribute_name.push('=');
                    self.state = State::AttributeName;
                    false
                }
                Some(_) => {
                    self.current_attribute_name.clear();
                    self.state = State::AttributeName;
                    true
                }
            },
            State::AttributeName => match ch {
                Some(c) if Self::is_whitespace(c) || c == '/' || c == '>' => {
                    self.commit_attribute_name(position);
                    self.state = State::AfterAttributeName;
                    true
                }
                None => {
                    self.commit_attribute_name(position);
                    self.state = State::AfterAttributeName;
                    true
                }
                Some('=') => {
                    self.commit_attribute_name(position);
                    self.state = State::BeforeAttributeValue;
                    false
                }
                Some(c) if c.is_ascii_uppercase() => {
                    self.current_attribute_name.push(c.to_ascii_lowercase());
                    false
                }
                Some('\0') => {
                    self.current_attribute_name.push('\u{FFFD}');
                    false
                }
                Some(c @ ('"' | '\'' | '<')) => {
                    // unexpected-character-in-attribute-name parse error,
                    // but still appended as-is, per spec.
                    self.error(ParseErrorKind::UnexpectedCharacterInAttributeName, position);
                    self.current_attribute_name.push(c);
                    false
                }
                Some(c) => {
                    self.current_attribute_name.push(c);
                    false
                }
            },
            State::AfterAttributeName => match ch {
                Some(c) if Self::is_whitespace(c) => false,
                Some('/') => {
                    self.state = State::SelfClosingStartTag;
                    false
                }
                Some('=') => {
                    self.state = State::BeforeAttributeValue;
                    false
                }
                Some('>') => self.close_tag(out),
                None => {
                    // eof-in-tag parse error.
                    self.error(ParseErrorKind::EofInTag, position);
                    push_eof(out, position);
                    false
                }
                Some(_) => {
                    self.current_attribute_name.clear();
                    self.state = State::AttributeName;
                    true
                }
            },
            State::BeforeAttributeValue => match ch {
                Some(c) if Self::is_whitespace(c) => false,
                Some('"') => {
                    self.state = State::AttributeValueDoubleQuoted;
                    false
                }
                Some('\'') => {
                    self.state = State::AttributeValueSingleQuoted;
                    false
                }
                Some('>') => {
                    // missing-attribute-value parse error.
                    self.error(ParseErrorKind::MissingAttributeValue, position);
                    self.close_tag(out)
                }
                _ => {
                    self.state = State::AttributeValueUnquoted;
                    true
                }
            },
            State::AttributeValueDoubleQuoted => {
                self.step_attribute_value_quoted(ch, position, out, '"')
            }
            State::AttributeValueSingleQuoted => {
                self.step_attribute_value_quoted(ch, position, out, '\'')
            }
            State::AttributeValueUnquoted => match ch {
                Some(c) if Self::is_whitespace(c) => {
                    self.state = State::BeforeAttributeName;
                    false
                }
                Some('&') => {
                    self.begin_character_reference(State::AttributeValueUnquoted, position);
                    false
                }
                Some('>') => self.close_tag(out),
                Some('\0') => {
                    self.push_attribute_value_char('\u{FFFD}');
                    false
                }
                Some(c @ ('"' | '\'' | '<' | '=' | '`')) => {
                    // unexpected-character-in-unquoted-attribute-value
                    // parse error, but still appended as-is, per spec.
                    self.error(
                        ParseErrorKind::UnexpectedCharacterInUnquotedAttributeValue,
                        position,
                    );
                    self.push_attribute_value_char(c);
                    false
                }
                Some(c) => {
                    self.push_attribute_value_char(c);
                    false
                }
                None => {
                    // eof-in-tag parse error.
                    self.error(ParseErrorKind::EofInTag, position);
                    push_eof(out, position);
                    false
                }
            },
            State::AfterAttributeValueQuoted => match ch {
                Some(c) if Self::is_whitespace(c) => {
                    self.state = State::BeforeAttributeName;
                    false
                }
                Some('/') => {
                    self.state = State::SelfClosingStartTag;
                    false
                }
                Some('>') => self.close_tag(out),
                None => {
                    // eof-in-tag parse error.
                    self.error(ParseErrorKind::EofInTag, position);
                    push_eof(out, position);
                    false
                }
                Some(_) => {
                    // missing-whitespace-between-attributes parse error.
                    self.error(ParseErrorKind::MissingWhitespaceBetweenAttributes, position);
                    self.state = State::BeforeAttributeName;
                    true
                }
            },
            State::SelfClosingStartTag => match ch {
                Some('>') => {
                    self.current_tag_mut().self_closing = true;
                    self.close_tag(out)
                }
                None => {
                    // eof-in-tag parse error.
                    self.error(ParseErrorKind::EofInTag, position);
                    push_eof(out, position);
                    false
                }
                Some(_) => {
                    // unexpected-solidus-in-tag parse error.
                    self.error(ParseErrorKind::UnexpectedSolidusInTag, position);
                    self.state = State::BeforeAttributeName;
                    true
                }
            },
            State::CharacterReference => match ch {
                Some(c) if c.is_ascii_alphanumeric() => {
                    self.state = State::NamedCharacterReference;
                    true
                }
                Some('#') => {
                    self.state = State::NumericCharacterReference;
                    false
                }
                _ => {
                    // Buffer is always exactly "&" here (1 char): `ch` may
                    // be EOF, which — unlike a real character — never
                    // advances `self.index`, so the end offset must be
                    // computed from `character_reference_start_index`
                    // directly rather than from `self.index`.
                    let end = self.character_reference_start_index + 1;
                    self.flush_literal_character_reference_attempt(end, out);
                    self.state = self.return_state;
                    true
                }
            },
            // State::NamedCharacterReference is handled entirely outside
            // `step()` — see `run_until_token`/`run_named_character_reference`.
            State::NamedCharacterReference => {
                unreachable!("NamedCharacterReference is dispatched before step() is called")
            }
            State::AmbiguousAmpersand => match ch {
                Some(c) if c.is_ascii_alphanumeric() => {
                    self.flush_char_as_character_reference(c, position, out);
                    false
                }
                Some(';') => {
                    // unknown-named-character-reference parse error.
                    self.error(ParseErrorKind::UnknownNamedCharacterReference, position);
                    self.state = self.return_state;
                    true
                }
                _ => {
                    self.state = self.return_state;
                    true
                }
            },
            State::NumericCharacterReference => {
                self.character_reference_code = 0;
                match ch {
                    Some('x') | Some('X') => {
                        self.state = State::HexadecimalCharacterReferenceStart;
                        false
                    }
                    Some(c) if c.is_ascii_digit() => {
                        self.state = State::DecimalCharacterReference;
                        true
                    }
                    _ => {
                        // absence-of-digits-in-numeric-character-reference
                        // parse error. Buffer is always exactly "&#" (2
                        // chars) here — see the CharacterReference state's
                        // fallback above for why this is a fixed offset
                        // rather than derived from `self.index`.
                        self.error(
                            ParseErrorKind::AbsenceOfDigitsInNumericCharacterReference,
                            position,
                        );
                        let end = self.character_reference_start_index + 2;
                        self.flush_literal_character_reference_attempt(end, out);
                        self.state = self.return_state;
                        true
                    }
                }
            }
            State::HexadecimalCharacterReferenceStart => match ch {
                Some(c) if c.is_ascii_hexdigit() => {
                    self.state = State::HexadecimalCharacterReference;
                    true
                }
                _ => {
                    // absence-of-digits-in-numeric-character-reference
                    // parse error. Buffer is always exactly "&#x"/"&#X" (3
                    // chars) here — same fixed-offset reasoning as above.
                    self.error(
                        ParseErrorKind::AbsenceOfDigitsInNumericCharacterReference,
                        position,
                    );
                    let end = self.character_reference_start_index + 3;
                    self.flush_literal_character_reference_attempt(end, out);
                    self.state = self.return_state;
                    true
                }
            },
            State::HexadecimalCharacterReference => match ch {
                Some(c) if c.is_ascii_digit() => {
                    self.character_reference_code = self
                        .character_reference_code
                        .saturating_mul(16)
                        .saturating_add(u32::from(c) - u32::from('0'));
                    false
                }
                Some(c) if ('A'..='F').contains(&c) => {
                    self.character_reference_code = self
                        .character_reference_code
                        .saturating_mul(16)
                        .saturating_add(u32::from(c) - 0x37);
                    false
                }
                Some(c) if ('a'..='f').contains(&c) => {
                    self.character_reference_code = self
                        .character_reference_code
                        .saturating_mul(16)
                        .saturating_add(u32::from(c) - 0x57);
                    false
                }
                Some(';') => {
                    self.state = State::NumericCharacterReferenceEnd;
                    false
                }
                _ => {
                    // missing-semicolon-after-character-reference parse
                    // error.
                    self.error(
                        ParseErrorKind::MissingSemicolonAfterCharacterReference,
                        position,
                    );
                    self.state = State::NumericCharacterReferenceEnd;
                    true
                }
            },
            State::DecimalCharacterReference => match ch {
                Some(c) if c.is_ascii_digit() => {
                    self.character_reference_code = self
                        .character_reference_code
                        .saturating_mul(10)
                        .saturating_add(u32::from(c) - u32::from('0'));
                    false
                }
                Some(';') => {
                    self.state = State::NumericCharacterReferenceEnd;
                    false
                }
                _ => {
                    // missing-semicolon-after-character-reference parse
                    // error.
                    self.error(
                        ParseErrorKind::MissingSemicolonAfterCharacterReference,
                        position,
                    );
                    self.state = State::NumericCharacterReferenceEnd;
                    true
                }
            },
            State::NumericCharacterReferenceEnd => {
                // §13.2.5.84 does not consume an input character at all —
                // whatever `ch` is must be handed on, unconsumed, to
                // `return_state`.
                let resolved =
                    self.resolve_numeric_character_reference_code(self.character_reference_start);
                self.flush_char_as_character_reference(
                    resolved,
                    self.character_reference_start,
                    out,
                );
                self.state = self.return_state;
                true
            }
            // State::MarkupDeclarationOpen is handled entirely outside
            // `step()` — see `run_until_token`/`run_markup_declaration_open`.
            State::MarkupDeclarationOpen => {
                unreachable!("MarkupDeclarationOpen is dispatched before step() is called")
            }
            State::BogusComment => match ch {
                Some('>') => {
                    self.state = State::Data;
                    self.emit_comment(out);
                    false
                }
                None => {
                    self.emit_comment(out);
                    push_eof(out, position);
                    false
                }
                Some('\0') => {
                    self.current_comment_data.push('\u{FFFD}');
                    false
                }
                Some(c) => {
                    self.current_comment_data.push(c);
                    false
                }
            },
            State::CommentStart => match ch {
                Some('-') => {
                    self.state = State::CommentStartDash;
                    false
                }
                Some('>') => {
                    // abrupt-closing-of-empty-comment parse error.
                    self.error(ParseErrorKind::AbruptClosingOfEmptyComment, position);
                    self.state = State::Data;
                    self.emit_comment(out);
                    false
                }
                _ => {
                    self.state = State::Comment;
                    true
                }
            },
            State::CommentStartDash => match ch {
                Some('-') => {
                    self.state = State::CommentEnd;
                    false
                }
                Some('>') => {
                    // abrupt-closing-of-empty-comment parse error.
                    self.error(ParseErrorKind::AbruptClosingOfEmptyComment, position);
                    self.state = State::Data;
                    self.emit_comment(out);
                    false
                }
                None => {
                    // eof-in-comment parse error.
                    self.error(ParseErrorKind::EofInComment, position);
                    self.emit_comment(out);
                    push_eof(out, position);
                    false
                }
                Some(_) => {
                    self.current_comment_data.push('-');
                    self.state = State::Comment;
                    true
                }
            },
            State::Comment => match ch {
                Some('<') => {
                    self.current_comment_data.push('<');
                    self.state = State::CommentLessThanSign;
                    false
                }
                Some('-') => {
                    self.state = State::CommentEndDash;
                    false
                }
                Some('\0') => {
                    self.current_comment_data.push('\u{FFFD}');
                    false
                }
                None => {
                    // eof-in-comment parse error.
                    self.error(ParseErrorKind::EofInComment, position);
                    self.emit_comment(out);
                    push_eof(out, position);
                    false
                }
                Some(c) => {
                    self.current_comment_data.push(c);
                    false
                }
            },
            State::CommentLessThanSign => match ch {
                Some('!') => {
                    self.current_comment_data.push('!');
                    self.state = State::CommentLessThanSignBang;
                    false
                }
                Some('<') => {
                    self.current_comment_data.push('<');
                    false
                }
                _ => {
                    self.state = State::Comment;
                    true
                }
            },
            State::CommentLessThanSignBang => match ch {
                Some('-') => {
                    self.state = State::CommentLessThanSignBangDash;
                    false
                }
                _ => {
                    self.state = State::Comment;
                    true
                }
            },
            State::CommentLessThanSignBangDash => match ch {
                Some('-') => {
                    self.state = State::CommentLessThanSignBangDashDash;
                    false
                }
                _ => {
                    self.state = State::CommentEndDash;
                    true
                }
            },
            State::CommentLessThanSignBangDashDash => {
                // Both the '>'/EOF branch and the "anything else"
                // (nested-comment parse error) branch reconsume in the
                // same state; they differ only in whether a parse error
                // is flagged.
                if !matches!(ch, Some('>') | None) {
                    self.error(ParseErrorKind::NestedComment, position);
                }
                self.state = State::CommentEnd;
                true
            }
            State::CommentEndDash => match ch {
                Some('-') => {
                    self.state = State::CommentEnd;
                    false
                }
                None => {
                    // eof-in-comment parse error.
                    self.error(ParseErrorKind::EofInComment, position);
                    self.emit_comment(out);
                    push_eof(out, position);
                    false
                }
                Some(_) => {
                    self.current_comment_data.push('-');
                    self.state = State::Comment;
                    true
                }
            },
            State::CommentEnd => match ch {
                Some('>') => {
                    self.state = State::Data;
                    self.emit_comment(out);
                    false
                }
                Some('!') => {
                    self.state = State::CommentEndBang;
                    false
                }
                Some('-') => {
                    self.current_comment_data.push('-');
                    false
                }
                None => {
                    // eof-in-comment parse error.
                    self.error(ParseErrorKind::EofInComment, position);
                    self.emit_comment(out);
                    push_eof(out, position);
                    false
                }
                Some(_) => {
                    self.current_comment_data.push_str("--");
                    self.state = State::Comment;
                    true
                }
            },
            State::CommentEndBang => match ch {
                Some('-') => {
                    self.current_comment_data.push_str("--!");
                    self.state = State::CommentEndDash;
                    false
                }
                Some('>') => {
                    // incorrectly-closed-comment parse error.
                    self.error(ParseErrorKind::IncorrectlyClosedComment, position);
                    self.state = State::Data;
                    self.emit_comment(out);
                    false
                }
                None => {
                    // eof-in-comment parse error.
                    self.error(ParseErrorKind::EofInComment, position);
                    self.emit_comment(out);
                    push_eof(out, position);
                    false
                }
                Some(_) => {
                    self.current_comment_data.push_str("--!");
                    self.state = State::Comment;
                    true
                }
            },
            State::Doctype => match ch {
                Some(c) if Self::is_whitespace(c) => {
                    self.state = State::BeforeDoctypeName;
                    false
                }
                Some('>') => {
                    self.state = State::BeforeDoctypeName;
                    true
                }
                None => {
                    // eof-in-doctype parse error. No DOCTYPE token exists
                    // yet at this point — unlike every other DOCTYPE
                    // sub-state's eof-in-doctype handling, one must be
                    // created first.
                    self.error(ParseErrorKind::EofInDoctype, position);
                    self.current_doctype = Some(DoctypeToken {
                        force_quirks: true,
                        ..Default::default()
                    });
                    self.emit_doctype(out);
                    push_eof(out, position);
                    false
                }
                Some(_) => {
                    // missing-whitespace-before-doctype-name parse error.
                    self.error(ParseErrorKind::MissingWhitespaceBeforeDoctypeName, position);
                    self.state = State::BeforeDoctypeName;
                    true
                }
            },
            State::BeforeDoctypeName => match ch {
                Some(c) if Self::is_whitespace(c) => false,
                Some(c) if c.is_ascii_uppercase() => {
                    self.current_doctype = Some(DoctypeToken {
                        name: Some(c.to_ascii_lowercase().to_string()),
                        ..Default::default()
                    });
                    self.state = State::DoctypeName;
                    false
                }
                Some('\0') => {
                    self.current_doctype = Some(DoctypeToken {
                        name: Some("\u{FFFD}".to_owned()),
                        ..Default::default()
                    });
                    self.state = State::DoctypeName;
                    false
                }
                Some('>') => {
                    // missing-doctype-name parse error.
                    self.error(ParseErrorKind::MissingDoctypeName, position);
                    self.current_doctype = Some(DoctypeToken {
                        force_quirks: true,
                        ..Default::default()
                    });
                    self.close_doctype(out)
                }
                None => {
                    // eof-in-doctype parse error.
                    self.error(ParseErrorKind::EofInDoctype, position);
                    self.current_doctype = Some(DoctypeToken {
                        force_quirks: true,
                        ..Default::default()
                    });
                    self.emit_doctype(out);
                    push_eof(out, position);
                    false
                }
                Some(c) => {
                    self.current_doctype = Some(DoctypeToken {
                        name: Some(c.to_string()),
                        ..Default::default()
                    });
                    self.state = State::DoctypeName;
                    false
                }
            },
            State::DoctypeName => match ch {
                Some(c) if Self::is_whitespace(c) => {
                    self.state = State::AfterDoctypeName;
                    false
                }
                Some('>') => self.close_doctype(out),
                Some(c) if c.is_ascii_uppercase() => {
                    self.current_doctype_mut()
                        .name
                        .as_mut()
                        .expect("doctype name should already be Some in DoctypeName state")
                        .push(c.to_ascii_lowercase());
                    false
                }
                Some('\0') => {
                    self.current_doctype_mut()
                        .name
                        .as_mut()
                        .expect("doctype name should already be Some in DoctypeName state")
                        .push('\u{FFFD}');
                    false
                }
                None => self.eof_in_doctype(out, position),
                Some(c) => {
                    self.current_doctype_mut()
                        .name
                        .as_mut()
                        .expect("doctype name should already be Some in DoctypeName state")
                        .push(c);
                    false
                }
            },
            State::AfterDoctypeName => match ch {
                Some(c) if Self::is_whitespace(c) => false,
                Some('>') => self.close_doctype(out),
                None => self.eof_in_doctype(out, position),
                Some(_) => {
                    let start = self.index - 1;
                    if self.peek_matches_at(start, "PUBLIC", true) {
                        self.index = start + 6;
                        self.state = State::AfterDoctypePublicKeyword;
                        false
                    } else if self.peek_matches_at(start, "SYSTEM", true) {
                        self.index = start + 6;
                        self.state = State::AfterDoctypeSystemKeyword;
                        false
                    } else {
                        // invalid-character-sequence-after-doctype-name
                        // parse error.
                        self.error(
                            ParseErrorKind::InvalidCharacterSequenceAfterDoctypeName,
                            position,
                        );
                        self.bogus_doctype_with_quirks()
                    }
                }
            },
            State::AfterDoctypePublicKeyword => match ch {
                Some(c) if Self::is_whitespace(c) => {
                    self.state = State::BeforeDoctypePublicIdentifier;
                    false
                }
                Some('"') => {
                    self.start_doctype_public_identifier(State::DoctypePublicIdentifierDoubleQuoted)
                }
                Some('\'') => {
                    self.start_doctype_public_identifier(State::DoctypePublicIdentifierSingleQuoted)
                }
                Some('>') => self.close_doctype_with_quirks(out),
                None => self.eof_in_doctype(out, position),
                Some(_) => self.bogus_doctype_with_quirks(),
            },
            State::BeforeDoctypePublicIdentifier => match ch {
                Some(c) if Self::is_whitespace(c) => false,
                Some('"') => {
                    self.start_doctype_public_identifier(State::DoctypePublicIdentifierDoubleQuoted)
                }
                Some('\'') => {
                    self.start_doctype_public_identifier(State::DoctypePublicIdentifierSingleQuoted)
                }
                Some('>') => self.close_doctype_with_quirks(out),
                None => self.eof_in_doctype(out, position),
                Some(_) => self.bogus_doctype_with_quirks(),
            },
            State::DoctypePublicIdentifierDoubleQuoted => self.step_doctype_identifier_quoted(
                ch,
                position,
                out,
                '"',
                DoctypeIdentifierKind::Public,
            ),
            State::DoctypePublicIdentifierSingleQuoted => self.step_doctype_identifier_quoted(
                ch,
                position,
                out,
                '\'',
                DoctypeIdentifierKind::Public,
            ),
            State::AfterDoctypePublicIdentifier => match ch {
                Some(c) if Self::is_whitespace(c) => {
                    self.state = State::BetweenDoctypePublicAndSystemIdentifiers;
                    false
                }
                Some('>') => self.close_doctype(out),
                Some(c @ ('"' | '\'')) => {
                    // missing-whitespace-between-doctype-public-and-
                    // system-identifiers parse error.
                    self.error(
                        ParseErrorKind::MissingWhitespaceBetweenDoctypePublicAndSystemIdentifiers,
                        position,
                    );
                    let quoted_state = if c == '"' {
                        State::DoctypeSystemIdentifierDoubleQuoted
                    } else {
                        State::DoctypeSystemIdentifierSingleQuoted
                    };
                    self.start_doctype_system_identifier(quoted_state)
                }
                None => self.eof_in_doctype(out, position),
                Some(_) => self.bogus_doctype_with_quirks(),
            },
            State::BetweenDoctypePublicAndSystemIdentifiers => match ch {
                Some(c) if Self::is_whitespace(c) => false,
                Some('>') => self.close_doctype(out),
                Some('"') => {
                    self.start_doctype_system_identifier(State::DoctypeSystemIdentifierDoubleQuoted)
                }
                Some('\'') => {
                    self.start_doctype_system_identifier(State::DoctypeSystemIdentifierSingleQuoted)
                }
                None => self.eof_in_doctype(out, position),
                Some(_) => self.bogus_doctype_with_quirks(),
            },
            State::AfterDoctypeSystemKeyword => match ch {
                Some(c) if Self::is_whitespace(c) => {
                    self.state = State::BeforeDoctypeSystemIdentifier;
                    false
                }
                Some('"') => {
                    self.start_doctype_system_identifier(State::DoctypeSystemIdentifierDoubleQuoted)
                }
                Some('\'') => {
                    self.start_doctype_system_identifier(State::DoctypeSystemIdentifierSingleQuoted)
                }
                Some('>') => self.close_doctype_with_quirks(out),
                None => self.eof_in_doctype(out, position),
                Some(_) => self.bogus_doctype_with_quirks(),
            },
            State::BeforeDoctypeSystemIdentifier => match ch {
                Some(c) if Self::is_whitespace(c) => false,
                Some('"') => {
                    self.start_doctype_system_identifier(State::DoctypeSystemIdentifierDoubleQuoted)
                }
                Some('\'') => {
                    self.start_doctype_system_identifier(State::DoctypeSystemIdentifierSingleQuoted)
                }
                Some('>') => self.close_doctype_with_quirks(out),
                None => self.eof_in_doctype(out, position),
                Some(_) => self.bogus_doctype_with_quirks(),
            },
            State::DoctypeSystemIdentifierDoubleQuoted => self.step_doctype_identifier_quoted(
                ch,
                position,
                out,
                '"',
                DoctypeIdentifierKind::System,
            ),
            State::DoctypeSystemIdentifierSingleQuoted => self.step_doctype_identifier_quoted(
                ch,
                position,
                out,
                '\'',
                DoctypeIdentifierKind::System,
            ),
            State::AfterDoctypeSystemIdentifier => match ch {
                Some(c) if Self::is_whitespace(c) => false,
                Some('>') => self.close_doctype(out),
                None => self.eof_in_doctype(out, position),
                Some(_) => {
                    // unexpected-character-after-doctype-system-identifier
                    // parse error — deliberately does *not* set
                    // force_quirks, per spec's explicit note.
                    self.error(
                        ParseErrorKind::UnexpectedCharacterAfterDoctypeSystemIdentifier,
                        position,
                    );
                    self.state = State::BogusDoctype;
                    true
                }
            },
            State::BogusDoctype => match ch {
                Some('>') => self.close_doctype(out),
                Some('\0') => false,
                None => {
                    self.emit_doctype(out);
                    push_eof(out, position);
                    false
                }
                Some(_) => false,
            },
            State::ProcessingInstructionOpen => match ch {
                Some(c) if c.is_ascii_alphabetic() || c == '_' => {
                    self.state = State::ProcessingInstructionTarget;
                    true
                }
                None => {
                    // eof-in-processing-instruction parse error. Unlike
                    // DOCTYPE/comment states' EOF handling, the spec here
                    // says only "emit an end-of-file token" — no
                    // in-progress token exists yet to emit anyway.
                    self.error(ParseErrorKind::EofInProcessingInstruction, position);
                    push_eof(out, position);
                    false
                }
                Some(_) => {
                    // invalid-first-character-of-processing-instruction-
                    // target parse error. Buffer is still empty here —
                    // nothing has been accumulated yet.
                    self.error(
                        ParseErrorKind::InvalidFirstCharacterOfProcessingInstructionTarget,
                        position,
                    );
                    self.convert_pi_temporary_buffer_to_comment();
                    true
                }
            },
            State::ProcessingInstructionTarget => match ch {
                Some(c) if Self::is_whitespace(c) || c == '?' || c == '>' => {
                    let target = std::mem::take(&mut self.pi_temporary_buffer);
                    if target.eq_ignore_ascii_case("xml")
                        || target.eq_ignore_ascii_case("xml-stylesheet")
                    {
                        // disallowed-processing-instruction-target parse
                        // error.
                        self.error(
                            ParseErrorKind::DisallowedProcessingInstructionTarget,
                            position,
                        );
                        self.current_comment_data = format!("?{target}");
                        self.state = State::BogusComment;
                    } else {
                        self.current_processing_instruction = Some(ProcessingInstructionToken {
                            target,
                            data: String::new(),
                        });
                        self.state = State::AfterProcessingInstructionTarget;
                    }
                    true
                }
                Some(c) if c.is_ascii_alphanumeric() || c == '-' || c == '_' => {
                    self.pi_temporary_buffer.push(c);
                    false
                }
                None => {
                    // eof-in-processing-instruction parse error.
                    self.error(ParseErrorKind::EofInProcessingInstruction, position);
                    push_eof(out, position);
                    false
                }
                Some(_) => {
                    // invalid-processing-instruction-target parse error.
                    self.error(ParseErrorKind::InvalidProcessingInstructionTarget, position);
                    self.convert_pi_temporary_buffer_to_comment();
                    true
                }
            },
            State::AfterProcessingInstructionTarget => match ch {
                Some(c) if Self::is_whitespace(c) => false,
                _ => {
                    self.state = State::ProcessingInstructionData;
                    true
                }
            },
            State::ProcessingInstructionData => match ch {
                Some('?') => {
                    self.state = State::ProcessingInstructionQuestionable;
                    false
                }
                Some('>') => {
                    self.state = State::Data;
                    self.emit_processing_instruction(out);
                    false
                }
                None => {
                    // eof-in-processing-instruction parse error: the
                    // in-progress PI token is discarded, not emitted —
                    // spec says only "emit an end-of-file token" here.
                    self.error(ParseErrorKind::EofInProcessingInstruction, position);
                    self.current_processing_instruction = None;
                    push_eof(out, position);
                    false
                }
                Some(c) => {
                    self.current_processing_instruction
                        .as_mut()
                        .expect("processing instruction token should already exist in data state")
                        .data
                        .push(c);
                    false
                }
            },
            State::ProcessingInstructionQuestionable => match ch {
                Some('>') => {
                    self.state = State::Data;
                    self.emit_processing_instruction(out);
                    false
                }
                None => {
                    self.current_processing_instruction = None;
                    push_eof(out, position);
                    false
                }
                Some(_) => {
                    self.current_processing_instruction
                        .as_mut()
                        .expect(
                            "processing instruction token should already exist in questionable state",
                        )
                        .data
                        .push('?');
                    self.state = State::ProcessingInstructionData;
                    true
                }
            },
            State::RcData => match ch {
                Some('&') => {
                    self.begin_character_reference(State::RcData, position);
                    false
                }
                Some('<') => {
                    self.current_tag_start = position;
                    self.state = State::RcDataLessThanSign;
                    false
                }
                Some('\0') => {
                    push_character(out, '\u{FFFD}', position);
                    false
                }
                Some(c) => {
                    push_character(out, c, position);
                    false
                }
                None => {
                    push_eof(out, position);
                    false
                }
            },
            State::RcDataLessThanSign => self.step_text_less_than_sign(
                ch,
                position,
                out,
                State::RcDataEndTagOpen,
                State::RcData,
            ),
            State::RcDataEndTagOpen => self.step_text_end_tag_open(
                ch,
                position,
                out,
                State::RcDataEndTagName,
                State::RcData,
            ),
            State::RcDataEndTagName => {
                self.step_text_end_tag_name(ch, position, out, State::RcData)
            }
            State::RawText => match ch {
                Some('<') => {
                    self.current_tag_start = position;
                    self.state = State::RawTextLessThanSign;
                    false
                }
                Some('\0') => {
                    push_character(out, '\u{FFFD}', position);
                    false
                }
                Some(c) => {
                    push_character(out, c, position);
                    false
                }
                None => {
                    push_eof(out, position);
                    false
                }
            },
            State::RawTextLessThanSign => self.step_text_less_than_sign(
                ch,
                position,
                out,
                State::RawTextEndTagOpen,
                State::RawText,
            ),
            State::RawTextEndTagOpen => self.step_text_end_tag_open(
                ch,
                position,
                out,
                State::RawTextEndTagName,
                State::RawText,
            ),
            State::RawTextEndTagName => {
                self.step_text_end_tag_name(ch, position, out, State::RawText)
            }
            State::PlainText => match ch {
                Some('\0') => {
                    push_character(out, '\u{FFFD}', position);
                    false
                }
                Some(c) => {
                    push_character(out, c, position);
                    false
                }
                None => {
                    push_eof(out, position);
                    false
                }
            },
            State::ScriptData => match ch {
                Some('<') => {
                    self.current_tag_start = position;
                    self.state = State::ScriptDataLessThanSign;
                    false
                }
                Some('\0') => {
                    push_character(out, '\u{FFFD}', position);
                    false
                }
                Some(c) => {
                    push_character(out, c, position);
                    false
                }
                None => {
                    push_eof(out, position);
                    false
                }
            },
            State::ScriptDataLessThanSign => match ch {
                Some('/') => {
                    self.text_end_tag_buffer.clear();
                    self.slash_position = position;
                    self.state = State::ScriptDataEndTagOpen;
                    false
                }
                Some('!') => {
                    self.state = State::ScriptDataEscapeStart;
                    push_character(out, '<', self.current_tag_start);
                    push_character(out, '!', position);
                    false
                }
                _ => {
                    push_character(out, '<', self.current_tag_start);
                    self.state = State::ScriptData;
                    true
                }
            },
            State::ScriptDataEndTagOpen => self.step_text_end_tag_open(
                ch,
                position,
                out,
                State::ScriptDataEndTagName,
                State::ScriptData,
            ),
            State::ScriptDataEndTagName => {
                self.step_text_end_tag_name(ch, position, out, State::ScriptData)
            }
            State::ScriptDataEscapeStart => match ch {
                Some('-') => {
                    self.state = State::ScriptDataEscapeStartDash;
                    push_character(out, '-', position);
                    false
                }
                _ => {
                    self.state = State::ScriptData;
                    true
                }
            },
            State::ScriptDataEscapeStartDash => match ch {
                Some('-') => {
                    self.state = State::ScriptDataEscapedDashDash;
                    push_character(out, '-', position);
                    false
                }
                _ => {
                    self.state = State::ScriptData;
                    true
                }
            },
            State::ScriptDataEscaped => match ch {
                Some('-') => {
                    self.state = State::ScriptDataEscapedDash;
                    push_character(out, '-', position);
                    false
                }
                Some('<') => {
                    self.current_tag_start = position;
                    self.state = State::ScriptDataEscapedLessThanSign;
                    false
                }
                Some('\0') => {
                    push_character(out, '\u{FFFD}', position);
                    false
                }
                Some(c) => {
                    push_character(out, c, position);
                    false
                }
                None => {
                    // eof-in-script-html-comment-like-text parse error.
                    self.error(ParseErrorKind::EofInScriptHtmlCommentLikeText, position);
                    push_eof(out, position);
                    false
                }
            },
            State::ScriptDataEscapedDash => match ch {
                Some('-') => {
                    self.state = State::ScriptDataEscapedDashDash;
                    push_character(out, '-', position);
                    false
                }
                Some('<') => {
                    self.current_tag_start = position;
                    self.state = State::ScriptDataEscapedLessThanSign;
                    false
                }
                Some('\0') => {
                    self.state = State::ScriptDataEscaped;
                    push_character(out, '\u{FFFD}', position);
                    false
                }
                None => {
                    push_eof(out, position);
                    false
                }
                Some(c) => {
                    self.state = State::ScriptDataEscaped;
                    push_character(out, c, position);
                    false
                }
            },
            State::ScriptDataEscapedDashDash => match ch {
                Some('-') => {
                    push_character(out, '-', position);
                    false
                }
                Some('<') => {
                    self.current_tag_start = position;
                    self.state = State::ScriptDataEscapedLessThanSign;
                    false
                }
                Some('>') => {
                    self.state = State::ScriptData;
                    push_character(out, '>', position);
                    false
                }
                Some('\0') => {
                    self.state = State::ScriptDataEscaped;
                    push_character(out, '\u{FFFD}', position);
                    false
                }
                None => {
                    push_eof(out, position);
                    false
                }
                Some(c) => {
                    self.state = State::ScriptDataEscaped;
                    push_character(out, c, position);
                    false
                }
            },
            State::ScriptDataEscapedLessThanSign => match ch {
                Some('/') => {
                    self.text_end_tag_buffer.clear();
                    self.slash_position = position;
                    self.state = State::ScriptDataEscapedEndTagOpen;
                    false
                }
                Some(c) if c.is_ascii_alphabetic() => {
                    self.text_end_tag_buffer.clear();
                    push_character(out, '<', self.current_tag_start);
                    self.state = State::ScriptDataDoubleEscapeStart;
                    true
                }
                _ => {
                    push_character(out, '<', self.current_tag_start);
                    self.state = State::ScriptDataEscaped;
                    true
                }
            },
            State::ScriptDataEscapedEndTagOpen => self.step_text_end_tag_open(
                ch,
                position,
                out,
                State::ScriptDataEscapedEndTagName,
                State::ScriptDataEscaped,
            ),
            State::ScriptDataEscapedEndTagName => {
                self.step_text_end_tag_name(ch, position, out, State::ScriptDataEscaped)
            }
            State::ScriptDataDoubleEscapeStart => match ch {
                Some(c) if Self::is_whitespace(c) || c == '/' || c == '>' => {
                    self.state = if self.text_end_tag_buffer == "script" {
                        State::ScriptDataDoubleEscaped
                    } else {
                        State::ScriptDataEscaped
                    };
                    push_character(out, c, position);
                    false
                }
                Some(c) if c.is_ascii_uppercase() => {
                    self.text_end_tag_buffer.push(c.to_ascii_lowercase());
                    push_character(out, c, position);
                    false
                }
                Some(c) if c.is_ascii_lowercase() => {
                    self.text_end_tag_buffer.push(c);
                    push_character(out, c, position);
                    false
                }
                _ => {
                    self.state = State::ScriptDataEscaped;
                    true
                }
            },
            State::ScriptDataDoubleEscaped => match ch {
                Some('-') => {
                    self.state = State::ScriptDataDoubleEscapedDash;
                    push_character(out, '-', position);
                    false
                }
                Some('<') => {
                    self.state = State::ScriptDataDoubleEscapedLessThanSign;
                    push_character(out, '<', position);
                    false
                }
                Some('\0') => {
                    push_character(out, '\u{FFFD}', position);
                    false
                }
                Some(c) => {
                    push_character(out, c, position);
                    false
                }
                None => {
                    push_eof(out, position);
                    false
                }
            },
            State::ScriptDataDoubleEscapedDash => match ch {
                Some('-') => {
                    self.state = State::ScriptDataDoubleEscapedDashDash;
                    push_character(out, '-', position);
                    false
                }
                Some('<') => {
                    self.state = State::ScriptDataDoubleEscapedLessThanSign;
                    push_character(out, '<', position);
                    false
                }
                Some('\0') => {
                    self.state = State::ScriptDataDoubleEscaped;
                    push_character(out, '\u{FFFD}', position);
                    false
                }
                None => {
                    push_eof(out, position);
                    false
                }
                Some(c) => {
                    self.state = State::ScriptDataDoubleEscaped;
                    push_character(out, c, position);
                    false
                }
            },
            State::ScriptDataDoubleEscapedDashDash => match ch {
                Some('-') => {
                    push_character(out, '-', position);
                    false
                }
                Some('<') => {
                    self.state = State::ScriptDataDoubleEscapedLessThanSign;
                    push_character(out, '<', position);
                    false
                }
                Some('>') => {
                    self.state = State::ScriptData;
                    push_character(out, '>', position);
                    false
                }
                Some('\0') => {
                    self.state = State::ScriptDataDoubleEscaped;
                    push_character(out, '\u{FFFD}', position);
                    false
                }
                None => {
                    push_eof(out, position);
                    false
                }
                Some(c) => {
                    self.state = State::ScriptDataDoubleEscaped;
                    push_character(out, c, position);
                    false
                }
            },
            State::ScriptDataDoubleEscapedLessThanSign => match ch {
                Some('/') => {
                    self.text_end_tag_buffer.clear();
                    self.state = State::ScriptDataDoubleEscapeEnd;
                    push_character(out, '/', position);
                    false
                }
                _ => {
                    self.state = State::ScriptDataDoubleEscaped;
                    true
                }
            },
            State::ScriptDataDoubleEscapeEnd => match ch {
                Some(c) if Self::is_whitespace(c) || c == '/' || c == '>' => {
                    self.state = if self.text_end_tag_buffer == "script" {
                        State::ScriptDataEscaped
                    } else {
                        State::ScriptDataDoubleEscaped
                    };
                    push_character(out, c, position);
                    false
                }
                Some(c) if c.is_ascii_uppercase() => {
                    self.text_end_tag_buffer.push(c.to_ascii_lowercase());
                    push_character(out, c, position);
                    false
                }
                Some(c) if c.is_ascii_lowercase() => {
                    self.text_end_tag_buffer.push(c);
                    push_character(out, c, position);
                    false
                }
                _ => {
                    self.state = State::ScriptDataDoubleEscaped;
                    true
                }
            },
            State::CdataSection => match ch {
                Some(']') => {
                    self.cdata_pending_brackets.push(position);
                    self.state = State::CdataSectionBracket;
                    false
                }
                None => {
                    // eof-in-cdata parse error.
                    self.error(ParseErrorKind::EofInCdata, position);
                    push_eof(out, position);
                    false
                }
                Some(c) => {
                    // U+0000 NULL is deliberately *not* replaced with
                    // U+FFFD here — the spec explicitly says NUL handling
                    // inside CDATA sections happens in tree-construction's
                    // "in foreign content" rules, not in the tokenizer.
                    push_character(out, c, position);
                    false
                }
            },
            State::CdataSectionBracket => match ch {
                Some(']') => {
                    self.cdata_pending_brackets.push(position);
                    self.state = State::CdataSectionEnd;
                    false
                }
                _ => {
                    // Exactly one `]` was withheld to get here (from
                    // CdataSection's own `]` branch) — flush it with its
                    // own remembered position, not this (different)
                    // character's.
                    let bracket_position = self
                        .cdata_pending_brackets
                        .pop()
                        .expect("CdataSectionBracket reached with no withheld ']'");
                    push_character(out, ']', bracket_position);
                    self.state = State::CdataSection;
                    true
                }
            },
            State::CdataSectionEnd => match ch {
                Some(']') => {
                    // A third (or later) consecutive `]`: the oldest
                    // withheld one can no longer be part of the `]]>`
                    // terminator (that needs exactly the *last* two `]`s
                    // before a `>`), so it's confirmed content — flush it
                    // with its own position, and slide the window: this
                    // new `]` joins the pending set in its place.
                    let oldest = self.cdata_pending_brackets.remove(0);
                    self.cdata_pending_brackets.push(position);
                    push_character(out, ']', oldest);
                    false
                }
                Some('>') => {
                    // The two withheld `]`s are consumed as part of the
                    // `]]>` terminator itself — never emitted as content.
                    self.cdata_pending_brackets.clear();
                    self.state = State::Data;
                    false
                }
                _ => {
                    // Exactly two `]`s are withheld here — flush both with
                    // their own remembered positions, in order.
                    for bracket_position in self.cdata_pending_brackets.drain(..) {
                        push_character(out, ']', bracket_position);
                    }
                    self.state = State::CdataSection;
                    true
                }
            },
        }
    }

    fn step_attribute_value_quoted(
        &mut self,
        ch: Option<char>,
        position: Position,
        out: &mut Vec<Token>,
        quote: char,
    ) -> bool {
        match ch {
            Some(c) if c == quote => {
                self.state = State::AfterAttributeValueQuoted;
                false
            }
            Some('&') => {
                let quoted_state = self.state;
                self.begin_character_reference(quoted_state, position);
                false
            }
            Some('\0') => {
                self.push_attribute_value_char('\u{FFFD}');
                false
            }
            Some(c) => {
                self.push_attribute_value_char(c);
                false
            }
            None => {
                // eof-in-tag parse error.
                self.error(ParseErrorKind::EofInTag, position);
                push_eof(out, position);
                false
            }
        }
    }

    fn current_tag_mut(&mut self) -> &mut TagToken {
        self.current_tag
            .as_mut()
            .expect("tag-name/self-closing state reached with no tag token in progress")
    }

    /// §13.2.5's "appropriate end tag token": the current (end) tag token
    /// in progress matches the last *start* tag this tokenizer emitted.
    fn is_appropriate_end_tag(&self) -> bool {
        let Some(tag) = &self.current_tag else {
            return false;
        };
        self.last_start_tag_name.as_deref() == Some(tag.name.as_str())
    }

    /// Shared by RCDATA/RAWTEXT's "less-than sign" states (§13.2.5.9/.12):
    /// either start tracking a possible end tag, or bail straight back to
    /// literal text. `text_state` must already have set `current_tag_start`
    /// to the `<`'s own position before switching into this state.
    fn step_text_less_than_sign(
        &mut self,
        ch: Option<char>,
        position: Position,
        out: &mut Vec<Token>,
        end_tag_open_state: State,
        text_state: State,
    ) -> bool {
        match ch {
            Some('/') => {
                self.text_end_tag_buffer.clear();
                self.slash_position = position;
                self.state = end_tag_open_state;
                false
            }
            _ => {
                push_character(out, '<', self.current_tag_start);
                self.state = text_state;
                true
            }
        }
    }

    /// Shared by RCDATA/RAWTEXT's "end tag open" states (§13.2.5.10/.13).
    fn step_text_end_tag_open(
        &mut self,
        ch: Option<char>,
        _position: Position,
        out: &mut Vec<Token>,
        end_tag_name_state: State,
        text_state: State,
    ) -> bool {
        match ch {
            Some(c) if c.is_ascii_alphabetic() => {
                self.start_tag_token(true);
                self.state = end_tag_name_state;
                true
            }
            _ => {
                push_character(out, '<', self.current_tag_start);
                push_character(out, '/', self.slash_position);
                self.state = text_state;
                true
            }
        }
    }

    /// Shared by RCDATA/RAWTEXT's "end tag name" states (§13.2.5.11/.14):
    /// keeps building a possible end tag as long as it could still turn
    /// out appropriate, and bails via `abandon_text_end_tag` the moment it
    /// can't (wrong name, or a name character that can't appear in a tag
    /// name at all).
    fn step_text_end_tag_name(
        &mut self,
        ch: Option<char>,
        _position: Position,
        out: &mut Vec<Token>,
        text_state: State,
    ) -> bool {
        match ch {
            Some(c) if Self::is_whitespace(c) => {
                if self.is_appropriate_end_tag() {
                    self.state = State::BeforeAttributeName;
                    false
                } else {
                    self.abandon_text_end_tag(out, text_state)
                }
            }
            Some('/') => {
                if self.is_appropriate_end_tag() {
                    self.state = State::SelfClosingStartTag;
                    false
                } else {
                    self.abandon_text_end_tag(out, text_state)
                }
            }
            Some('>') => {
                if self.is_appropriate_end_tag() {
                    self.close_tag(out)
                } else {
                    self.abandon_text_end_tag(out, text_state)
                }
            }
            Some(c) if c.is_ascii_uppercase() => {
                self.current_tag_mut().name.push(c.to_ascii_lowercase());
                self.text_end_tag_buffer.push(c);
                false
            }
            Some(c) if c.is_ascii_lowercase() => {
                self.current_tag_mut().name.push(c);
                self.text_end_tag_buffer.push(c);
                false
            }
            _ => self.abandon_text_end_tag(out, text_state),
        }
    }

    /// The "anything else" fallback shared by both RCDATA/RAWTEXT
    /// end-tag-name states, and by the non-appropriate case of their
    /// whitespace/`/`/`>` branches: the `</name` seen so far wasn't (or
    /// can't become) a real end tag, so it's flushed back out as literal
    /// `<`, `/`, and each buffered name character, and `ch` is reconsumed
    /// under `text_state`. The buffer only ever contains ASCII letters
    /// (the only characters these states append), so — like
    /// `flush_literal_character_reference_attempt` — positions are safe
    /// to compute by simple increment; here from `slash_position` (the
    /// buffer's first character always immediately follows the `/`).
    fn abandon_text_end_tag(&mut self, out: &mut Vec<Token>, text_state: State) -> bool {
        push_character(out, '<', self.current_tag_start);
        push_character(out, '/', self.slash_position);
        let mut position = Position {
            line: self.slash_position.line,
            column: self.slash_position.column + 1,
            byte_offset: self.slash_position.byte_offset + 1,
        };
        let buffer = std::mem::take(&mut self.text_end_tag_buffer);
        for c in buffer.chars() {
            push_character(out, c, position);
            position.column += 1;
            position.byte_offset += 1;
        }
        self.current_tag = None;
        self.state = text_state;
        true
    }

    /// Shared by the four DOCTYPE public/system × double/single-quoted
    /// identifier states (§13.2.5.59/.60/.65/.66) — they differ only in
    /// which quote character closes them and which identifier field they
    /// append to.
    fn step_doctype_identifier_quoted(
        &mut self,
        ch: Option<char>,
        position: Position,
        out: &mut Vec<Token>,
        quote: char,
        kind: DoctypeIdentifierKind,
    ) -> bool {
        match ch {
            Some(c) if c == quote => {
                self.state = match kind {
                    DoctypeIdentifierKind::Public => State::AfterDoctypePublicIdentifier,
                    DoctypeIdentifierKind::System => State::AfterDoctypeSystemIdentifier,
                };
                false
            }
            Some('\0') => {
                self.doctype_identifier_mut(kind).push('\u{FFFD}');
                false
            }
            Some('>') => self.close_doctype_with_quirks(out),
            None => self.eof_in_doctype(out, position),
            Some(c) => {
                self.doctype_identifier_mut(kind).push(c);
                false
            }
        }
    }

    /// §13.2.5's `&`-transitions ("Set the return state to X. Switch to
    /// the character reference state."), shared by the Data state and the
    /// three attribute value states. `position` is the position of the
    /// `&` itself; `self.index` at call time already points one past it.
    fn begin_character_reference(&mut self, return_state: State, position: Position) {
        self.return_state = return_state;
        self.character_reference_start = position;
        self.character_reference_start_index = self.index - 1;
        self.state = State::CharacterReference;
    }

    /// §13.2.5's "consumed as part of an attribute" condition: true iff
    /// `return_state` is one of the three attribute value states.
    fn character_reference_in_attribute(&self) -> bool {
        matches!(
            self.return_state,
            State::AttributeValueDoubleQuoted
                | State::AttributeValueSingleQuoted
                | State::AttributeValueUnquoted
        )
    }

    /// §13.2.5's "flush code points consumed as a character reference":
    /// appends `c` to the current attribute's value if the reference was
    /// consumed as part of an attribute, or emits it as a character token
    /// otherwise.
    fn flush_char_as_character_reference(
        &mut self,
        c: char,
        position: Position,
        out: &mut Vec<Token>,
    ) {
        if self.character_reference_in_attribute() {
            self.push_attribute_value_char(c);
        } else {
            push_character(out, c, position);
        }
    }

    /// Flushes the literal source text from the `&` that started the
    /// current character reference attempt up to (but not including)
    /// `end_index` — used for every "no match"/"absence of digits"
    /// fallback path, where the attempted reference turns out not to be
    /// one and is emitted as plain text instead of being resolved. Every
    /// character in these buffers (`&`, `#`, `x`/`X`, the named-reference
    /// attempt's own letters/digits) is literal single-byte ASCII source
    /// text, so positions are safe to compute by simple increment from
    /// `character_reference_start`.
    fn flush_literal_character_reference_attempt(
        &mut self,
        end_index: usize,
        out: &mut Vec<Token>,
    ) {
        let chars: Vec<char> = self.chars[self.character_reference_start_index..end_index].to_vec();
        let mut position = self.character_reference_start;
        for c in chars {
            self.flush_char_as_character_reference(c, position, out);
            position.column += 1;
            position.byte_offset += 1;
        }
    }

    /// Looks up `name` (without the leading `&`) in the generated named
    /// character references table.
    fn lookup_named_character_reference(name: &str) -> Option<&'static str> {
        entities::NAMED_CHARACTER_REFERENCES
            .binary_search_by(|&(candidate, _)| candidate.cmp(name))
            .ok()
            .map(|i| entities::NAMED_CHARACTER_REFERENCES[i].1)
    }

    /// True if some table entry has `prefix` as a strict prefix — i.e. it
    /// is still worth trying to consume another character while looking
    /// for the longest match. The table is sorted, so matching entries
    /// form a contiguous run.
    fn named_character_reference_has_longer_match(prefix: &str) -> bool {
        let start =
            entities::NAMED_CHARACTER_REFERENCES.partition_point(|&(name, _)| name < prefix);
        entities::NAMED_CHARACTER_REFERENCES[start..]
            .iter()
            .take_while(|&&(name, _)| name.starts_with(prefix))
            .any(|&(name, _)| name.len() > prefix.len())
    }

    /// §13.2.5.78 "Named character reference state": a maximal-munch
    /// lookup against the whole table, not a single-character state
    /// transition — implemented as its own non-consuming-loop routine
    /// (dispatched from `run_until_token`) rather than forced through the
    /// one-character-per-`step()`-call model the rest of the tokenizer
    /// uses. Looks ahead via a local cursor without committing `self.index`
    /// until the match length is known, so a longer, ultimately-failed
    /// attempt can cheaply "give back" the extra characters it peeked at.
    fn run_named_character_reference(&mut self) {
        let (first_char, _) = self.consume();
        let first_char = first_char.expect(
            "named character reference state entered without an alphanumeric first character",
        );

        let mut candidate = String::new();
        candidate.push(first_char);
        let mut cursor = self.index;
        let mut best: Option<usize> = None;
        if Self::lookup_named_character_reference(&candidate).is_some() {
            best = Some(candidate.len());
        }
        loop {
            if !Self::named_character_reference_has_longer_match(&candidate) {
                break;
            }
            let Some(&c) = self.chars.get(cursor) else {
                break;
            };
            candidate.push(c);
            cursor += 1;
            if Self::lookup_named_character_reference(&candidate).is_some() {
                best = Some(candidate.len());
            }
        }

        let mut out = Vec::new();
        match best {
            Some(matched_len) => {
                self.index = self.character_reference_start_index + 1 + matched_len;
                let matched_name = candidate[..matched_len].to_owned();
                let next_char = self.chars.get(self.index).copied();
                let last_matched_is_semicolon = matched_name.ends_with(';');
                let next_is_equals_or_alphanumeric = matches!(next_char, Some('='))
                    || matches!(next_char, Some(c) if c.is_ascii_alphanumeric());
                let historical = self.character_reference_in_attribute()
                    && !last_matched_is_semicolon
                    && next_is_equals_or_alphanumeric;
                if historical {
                    self.flush_literal_character_reference_attempt(self.index, &mut out);
                } else {
                    // if !last_matched_is_semicolon: missing-semicolon-
                    // after-character-reference parse error (still
                    // resolved either way, per spec).
                    if !last_matched_is_semicolon {
                        self.error(
                            ParseErrorKind::MissingSemicolonAfterCharacterReference,
                            self.character_reference_start,
                        );
                    }
                    let replacement = Self::lookup_named_character_reference(&matched_name)
                        .expect("matched_name was already verified to be a table entry");
                    let start = self.character_reference_start;
                    for c in replacement.chars() {
                        self.flush_char_as_character_reference(c, start, &mut out);
                    }
                }
                self.state = self.return_state;
            }
            None => {
                // unknown named character reference attempt: the whole
                // greedily-examined (but never matching) run is flushed
                // literally, then the ambiguous ampersand state takes over
                // from wherever it left off.
                self.index = cursor;
                self.flush_literal_character_reference_attempt(self.index, &mut out);
                self.state = State::AmbiguousAmpersand;
            }
        }
        self.pending.extend(out);
    }

    /// §13.2.5.84 "Numeric character reference end state": resolves
    /// `character_reference_code` to the character it actually represents,
    /// applying the null/out-of-range/surrogate/noncharacter/control-
    /// character corrections the spec mandates — each with its own named
    /// parse error, reported at `position` (the reference's start,
    /// matching where the resolved character itself gets flushed).
    fn resolve_numeric_character_reference_code(&mut self, position: Position) -> char {
        const NUL: u32 = 0x00;
        const MAX_UNICODE: u32 = 0x10FFFF;
        let code = self.character_reference_code;
        let resolved = if code == NUL {
            self.error(ParseErrorKind::NullCharacterReference, position);
            0xFFFD
        } else if code > MAX_UNICODE {
            self.error(
                ParseErrorKind::CharacterReferenceOutsideUnicodeRange,
                position,
            );
            0xFFFD
        } else if is_surrogate(code) {
            self.error(ParseErrorKind::SurrogateCharacterReference, position);
            0xFFFD
        } else if is_noncharacter(code) {
            self.error(ParseErrorKind::NoncharacterCharacterReference, position);
            code
        } else if code == 0x0D || (is_control(code) && !is_ascii_whitespace(code)) {
            self.error(ParseErrorKind::ControlCharacterReference, position);
            windows_1252_override(code).unwrap_or(code)
        } else {
            code
        };
        char::from_u32(resolved).unwrap_or('\u{FFFD}')
    }
}

/// Pushes an end-of-file token. Free function, not a method: every
/// `step()` arm already holds `out: &mut Vec<Token>` and `position:
/// Position` locally, and this exact push is by far the most repeated
/// shape in the whole state machine (every state's EOF branch does
/// exactly this).
fn push_eof(out: &mut Vec<Token>, position: Position) {
    out.push(Token {
        kind: TokenKind::Eof,
        position,
    });
}

/// Pushes a character token. Free function for the same reason as
/// [`push_eof`] — the single most repeated shape after it.
fn push_character(out: &mut Vec<Token>, c: char, position: Position) {
    out.push(Token {
        kind: TokenKind::Character(c),
        position,
    });
}

fn is_surrogate(code: u32) -> bool {
    (0xD800..=0xDFFF).contains(&code)
}

/// Per the Infra Standard: code points U+FDD0–U+FDEF, or any code point
/// whose low 16 bits are 0xFFFE or 0xFFFF (U+FFFE, U+FFFF, U+1FFFE, ...,
/// U+10FFFE, U+10FFFF).
fn is_noncharacter(code: u32) -> bool {
    (0xFDD0..=0xFDEF).contains(&code) || matches!(code & 0xFFFF, 0xFFFE | 0xFFFF)
}

/// Per the Infra Standard: a C0 control (U+0000–U+001F) or a code point in
/// U+007F–U+009F.
fn is_control(code: u32) -> bool {
    (0x00..=0x1F).contains(&code) || (0x7F..=0x9F).contains(&code)
}

/// Per the Infra Standard: tab, LF, FF, CR, or space.
fn is_ascii_whitespace(code: u32) -> bool {
    matches!(code, 0x09 | 0x0A | 0x0C | 0x0D | 0x20)
}

/// §13.2.5.84's control-character-reference override table, transcribed
/// verbatim from <https://html.spec.whatwg.org/multipage/parsing.html#numeric-character-reference-end-state>
/// (the Windows-1252-derived remapping for C1 control code points). Rows
/// not listed here (0x81, 0x8D, 0x8F, 0x90, 0x9D) have no override — the
/// code point is left unchanged, per spec.
fn windows_1252_override(code: u32) -> Option<u32> {
    const TABLE: &[(u32, u32)] = &[
        (0x80, 0x20AC),
        (0x82, 0x201A),
        (0x83, 0x0192),
        (0x84, 0x201E),
        (0x85, 0x2026),
        (0x86, 0x2020),
        (0x87, 0x2021),
        (0x88, 0x02C6),
        (0x89, 0x2030),
        (0x8A, 0x0160),
        (0x8B, 0x2039),
        (0x8C, 0x0152),
        (0x8E, 0x017D),
        (0x91, 0x2018),
        (0x92, 0x2019),
        (0x93, 0x201C),
        (0x94, 0x201D),
        (0x95, 0x2022),
        (0x96, 0x2013),
        (0x97, 0x2014),
        (0x98, 0x02DC),
        (0x99, 0x2122),
        (0x9A, 0x0161),
        (0x9B, 0x203A),
        (0x9C, 0x0153),
        (0x9E, 0x017E),
        (0x9F, 0x0178),
    ];
    TABLE
        .iter()
        .find(|&&(from, _)| from == code)
        .map(|&(_, to)| to)
}

impl Iterator for Tokenizer {
    type Item = Token;

    fn next(&mut self) -> Option<Token> {
        if self.pending.is_empty() {
            if self.eof_returned {
                return None;
            }
            self.run_until_token();
        }
        let token = self.pending.pop_front()?;
        if matches!(token.kind, TokenKind::Eof) {
            self.eof_returned = true;
        }
        Some(token)
    }
}

#[cfg(test)]
mod tests {
    // Phase 02 tokenizer tests: the subset of plan/02-tokenizer.md's test
    // matrix covered by the states implemented so far (tags, attributes,
    // plain text). Character references, comments, DOCTYPE, CDATA, and
    // RCDATA/RAWTEXT/script-data/PLAINTEXT switching are not covered yet.
    use super::*;

    fn tokenize(input: &str) -> Vec<Token> {
        Tokenizer::new(input).collect()
    }

    fn kinds(tokens: &[Token]) -> Vec<&TokenKind> {
        tokens.iter().map(|token| &token.kind).collect()
    }

    fn pos(line: u32, column: u32, byte_offset: usize) -> Position {
        Position {
            line,
            column,
            byte_offset,
        }
    }

    /// Runs `input` to completion and returns every [`ParseErrorKind`]
    /// recorded, in encounter order. Phase 07 (`plan/07-parse-errors.md`)
    /// helper — mirrors [`tokenize`] above but for errors instead of
    /// tokens.
    fn errors_for(input: &str) -> Vec<ParseErrorKind> {
        let mut tokenizer = Tokenizer::new(input);
        for _ in tokenizer.by_ref() {}
        tokenizer
            .take_errors()
            .into_iter()
            .map(|error| error.kind)
            .collect()
    }

    #[test]
    fn plain_text_emits_one_character_token_per_char_then_eof() {
        let tokens = tokenize("hi");
        assert_eq!(
            tokens,
            vec![
                Token {
                    kind: TokenKind::Character('h'),
                    position: pos(1, 1, 0),
                },
                Token {
                    kind: TokenKind::Character('i'),
                    position: pos(1, 2, 1),
                },
                Token {
                    kind: TokenKind::Eof,
                    position: pos(1, 3, 2),
                },
            ]
        );
    }

    #[test]
    fn simple_start_tag_with_no_attributes() {
        let tokens = tokenize("<p>");
        assert_eq!(
            tokens,
            vec![
                Token {
                    kind: TokenKind::StartTag(TagToken {
                        name: "p".to_owned(),
                        self_closing: false,
                        attributes: vec![],
                    }),
                    position: pos(1, 1, 0),
                },
                Token {
                    kind: TokenKind::Eof,
                    position: pos(1, 4, 3),
                },
            ]
        );
    }

    #[test]
    fn end_tag() {
        let tokens = tokenize("</p>");
        match &tokens[0].kind {
            TokenKind::EndTag(tag) => assert_eq!(tag.name, "p"),
            other => panic!("expected end tag, got {other:?}"),
        }
    }

    #[test]
    fn self_closing_tag() {
        let tokens = tokenize("<br/>");
        match &tokens[0].kind {
            TokenKind::StartTag(tag) => {
                assert_eq!(tag.name, "br");
                assert!(tag.self_closing);
            }
            other => panic!("expected start tag, got {other:?}"),
        }
    }

    #[test]
    fn attribute_value_quoting_forms() {
        for html in ["<a href=\"x\">", "<a href='x'>", "<a href=x>"] {
            let tokens = tokenize(html);
            match &tokens[0].kind {
                TokenKind::StartTag(tag) => assert_eq!(
                    tag.attributes,
                    vec![Attribute {
                        name: "href".to_owned(),
                        value: "x".to_owned(),
                    }],
                    "input: {html:?}",
                ),
                other => panic!("expected start tag for {html:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn multiple_attributes_separated_by_whitespace() {
        let tokens = tokenize(r#"<a href="x" target="y">"#);
        match &tokens[0].kind {
            TokenKind::StartTag(tag) => assert_eq!(
                tag.attributes,
                vec![
                    Attribute {
                        name: "href".to_owned(),
                        value: "x".to_owned(),
                    },
                    Attribute {
                        name: "target".to_owned(),
                        value: "y".to_owned(),
                    },
                ]
            ),
            other => panic!("expected start tag, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_attribute_name_keeps_first_occurrence_only() {
        let tokens = tokenize(r#"<a href="x" href="y">"#);
        match &tokens[0].kind {
            TokenKind::StartTag(tag) => assert_eq!(
                tag.attributes,
                vec![Attribute {
                    name: "href".to_owned(),
                    value: "x".to_owned(),
                }]
            ),
            other => panic!("expected start tag, got {other:?}"),
        }
    }

    #[test]
    fn tag_and_attribute_names_are_ascii_lowercased() {
        let tokens = tokenize(r#"<DIV CLASS="a">"#);
        match &tokens[0].kind {
            TokenKind::StartTag(tag) => {
                assert_eq!(tag.name, "div");
                assert_eq!(tag.attributes[0].name, "class");
            }
            other => panic!("expected start tag, got {other:?}"),
        }
    }

    #[test]
    fn boolean_attribute_with_no_value() {
        let tokens = tokenize("<input disabled>");
        match &tokens[0].kind {
            TokenKind::StartTag(tag) => assert_eq!(
                tag.attributes,
                vec![Attribute {
                    name: "disabled".to_owned(),
                    value: String::new(),
                }]
            ),
            other => panic!("expected start tag, got {other:?}"),
        }
    }

    #[test]
    fn null_character_in_tag_name_is_replaced_with_u_fffd() {
        let tokens = tokenize("<a\u{0}b>");
        match &tokens[0].kind {
            TokenKind::StartTag(tag) => assert_eq!(tag.name, "a\u{FFFD}b"),
            other => panic!("expected start tag, got {other:?}"),
        }
    }

    #[test]
    fn null_character_in_data_state_is_emitted_literally_not_replaced() {
        let tokens = tokenize("\u{0}");
        assert_eq!(tokens[0].kind, TokenKind::Character('\u{0}'));
    }

    #[test]
    fn eof_inside_a_tag_emits_only_eof_no_tag_token() {
        let tokens = tokenize("<div");
        assert_eq!(
            tokens,
            vec![Token {
                kind: TokenKind::Eof,
                position: pos(1, 5, 4),
            }]
        );
    }

    #[test]
    fn eof_right_after_solidus_emits_synthesized_lt_and_solidus_then_eof() {
        let tokens = tokenize("</");
        assert_eq!(
            tokens,
            vec![
                Token {
                    kind: TokenKind::Character('<'),
                    position: pos(1, 1, 0),
                },
                Token {
                    kind: TokenKind::Character('/'),
                    position: pos(1, 2, 1),
                },
                Token {
                    kind: TokenKind::Eof,
                    position: pos(1, 3, 2),
                },
            ]
        );
    }

    #[test]
    fn position_tracking_across_a_newline() {
        let tokens = tokenize("a\nb");
        assert_eq!(tokens[0].position, pos(1, 1, 0));
        assert_eq!(tokens[1].position, pos(1, 2, 1)); // the '\n' character token itself
        assert_eq!(tokens[2].position, pos(2, 1, 2));
    }

    #[test]
    fn crlf_and_lone_cr_normalize_to_a_single_lf_character_token() {
        let crlf = tokenize("a\r\nb");
        assert_eq!(
            kinds(&crlf),
            vec![
                &TokenKind::Character('a'),
                &TokenKind::Character('\n'),
                &TokenKind::Character('b'),
                &TokenKind::Eof,
            ]
        );
        let lone_cr = tokenize("a\rb");
        assert_eq!(
            kinds(&lone_cr),
            vec![
                &TokenKind::Character('a'),
                &TokenKind::Character('\n'),
                &TokenKind::Character('b'),
                &TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn named_character_reference_with_semicolon() {
        assert_eq!(
            kinds(&tokenize("&amp;")),
            vec![&TokenKind::Character('&'), &TokenKind::Eof]
        );
    }

    #[test]
    fn named_character_reference_multi_codepoint() {
        // NotEqualTilde; -> U+2242 U+0338, a two-codepoint replacement.
        assert_eq!(
            kinds(&tokenize("&NotEqualTilde;")),
            vec![
                &TokenKind::Character('\u{2242}'),
                &TokenKind::Character('\u{338}'),
                &TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn legacy_named_character_reference_without_semicolon_still_resolves_outside_attributes() {
        // "amp" (no ';') is one of the 106 legacy entity names; outside an
        // attribute, it still resolves (with a missing-semicolon parse
        // error we don't track), unlike an unknown name.
        assert_eq!(
            kinds(&tokenize("&amp b")),
            vec![
                &TokenKind::Character('&'),
                &TokenKind::Character(' '),
                &TokenKind::Character('b'),
                &TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn unknown_named_character_reference_falls_back_to_ambiguous_ampersand() {
        // No entity name starts with a digit, so this can never even
        // start a viable match: the whole "&1" is flushed literally, then
        // ';' is reconsumed under the Data state as its own token.
        assert_eq!(
            kinds(&tokenize("&1;")),
            vec![
                &TokenKind::Character('&'),
                &TokenKind::Character('1'),
                &TokenKind::Character(';'),
                &TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn named_character_reference_historical_fallback_in_unquoted_attribute_value() {
        // "not" is a legacy (no-';') entity; here it's immediately
        // followed by 'i' (ASCII alphanumeric) inside an unquoted
        // attribute value, so — for historical reasons — it is *not*
        // resolved: the literal source text is kept instead.
        let tokens = tokenize("<a href=&notit=1>");
        match &tokens[0].kind {
            TokenKind::StartTag(tag) => assert_eq!(
                tag.attributes,
                vec![Attribute {
                    name: "href".to_owned(),
                    value: "&notit=1".to_owned(),
                }]
            ),
            other => panic!("expected start tag, got {other:?}"),
        }
    }

    #[test]
    fn named_character_reference_historical_fallback_applies_in_double_quoted_attributes_too() {
        // The "historical reasons" fallback ("consumed as part of an
        // attribute") applies to *any* attribute value state, not just
        // unquoted — this is what keeps query strings like
        // `href="?a=1&copy=2"` from corrupting into "?a=1©=2" inside
        // quoted attributes too.
        let tokens = tokenize(r#"<a href="&notit">"#);
        match &tokens[0].kind {
            TokenKind::StartTag(tag) => assert_eq!(
                tag.attributes,
                vec![Attribute {
                    name: "href".to_owned(),
                    value: "&notit".to_owned(),
                }]
            ),
            other => panic!("expected start tag, got {other:?}"),
        }
    }

    #[test]
    fn named_character_reference_ending_in_semicolon_resolves_normally_even_in_an_attribute() {
        // The historical fallback only ever applies when the match does
        // *not* end in ';' — a `;`-terminated match always resolves,
        // attribute or not.
        let tokens = tokenize(r#"<a href="&copy;">"#);
        match &tokens[0].kind {
            TokenKind::StartTag(tag) => assert_eq!(
                tag.attributes,
                vec![Attribute {
                    name: "href".to_owned(),
                    value: "\u{A9}".to_owned(),
                }]
            ),
            other => panic!("expected start tag, got {other:?}"),
        }
    }

    #[test]
    fn decimal_and_hexadecimal_character_references() {
        assert_eq!(
            kinds(&tokenize("&#65;")),
            vec![&TokenKind::Character('A'), &TokenKind::Eof]
        );
        assert_eq!(
            kinds(&tokenize("&#x41;")),
            vec![&TokenKind::Character('A'), &TokenKind::Eof]
        );
        assert_eq!(
            kinds(&tokenize("&#X41;")),
            vec![&TokenKind::Character('A'), &TokenKind::Eof]
        );
    }

    #[test]
    fn numeric_character_reference_missing_semicolon_still_resolves() {
        assert_eq!(
            kinds(&tokenize("&#65x")),
            vec![
                &TokenKind::Character('A'),
                &TokenKind::Character('x'),
                &TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn numeric_character_reference_null_is_replaced_with_u_fffd() {
        assert_eq!(
            kinds(&tokenize("&#0;")),
            vec![&TokenKind::Character('\u{FFFD}'), &TokenKind::Eof]
        );
    }

    #[test]
    fn numeric_character_reference_outside_unicode_range_is_replaced_with_u_fffd() {
        assert_eq!(
            kinds(&tokenize("&#x110000;")),
            vec![&TokenKind::Character('\u{FFFD}'), &TokenKind::Eof]
        );
    }

    #[test]
    fn numeric_character_reference_surrogate_is_replaced_with_u_fffd() {
        assert_eq!(
            kinds(&tokenize("&#xD800;")),
            vec![&TokenKind::Character('\u{FFFD}'), &TokenKind::Eof]
        );
    }

    #[test]
    fn numeric_character_reference_windows_1252_control_override() {
        // 0x80 is remapped to U+20AC EURO SIGN, per the spec's
        // control-character-reference override table.
        assert_eq!(
            kinds(&tokenize("&#128;")),
            vec![&TokenKind::Character('\u{20AC}'), &TokenKind::Eof]
        );
    }

    #[test]
    fn numeric_character_reference_unmapped_c1_control_is_left_unchanged() {
        // 0x81 is a control character but has no row in the override
        // table, so it passes through unchanged (still a parse error,
        // which we don't track).
        assert_eq!(
            kinds(&tokenize("&#129;")),
            vec![&TokenKind::Character('\u{81}'), &TokenKind::Eof]
        );
    }

    #[test]
    fn absence_of_digits_in_numeric_character_reference_falls_back_to_literal_text() {
        assert_eq!(
            kinds(&tokenize("&#;")),
            vec![
                &TokenKind::Character('&'),
                &TokenKind::Character('#'),
                &TokenKind::Character(';'),
                &TokenKind::Eof,
            ]
        );
        assert_eq!(
            kinds(&tokenize("&#x;")),
            vec![
                &TokenKind::Character('&'),
                &TokenKind::Character('#'),
                &TokenKind::Character('x'),
                &TokenKind::Character(';'),
                &TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn lone_ampersand_at_eof_is_emitted_literally() {
        assert_eq!(
            kinds(&tokenize("&")),
            vec![&TokenKind::Character('&'), &TokenKind::Eof]
        );
    }

    #[test]
    fn simple_comment() {
        assert_eq!(
            kinds(&tokenize("<!-- hi -->")),
            vec![&TokenKind::Comment(" hi ".to_owned()), &TokenKind::Eof]
        );
    }

    #[test]
    fn abrupt_closing_of_empty_comment_still_emits_an_empty_comment_token() {
        assert_eq!(
            kinds(&tokenize("<!-->")),
            vec![&TokenKind::Comment(String::new()), &TokenKind::Eof]
        );
    }

    #[test]
    fn comment_containing_a_lone_hyphen() {
        assert_eq!(
            kinds(&tokenize("<!-- a - b -->")),
            vec![&TokenKind::Comment(" a - b ".to_owned()), &TokenKind::Eof]
        );
    }

    #[test]
    fn eof_inside_a_comment_still_emits_the_comment_token() {
        assert_eq!(
            kinds(&tokenize("<!-- unterminated")),
            vec![
                &TokenKind::Comment(" unterminated".to_owned()),
                &TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn nested_comment_open_sequence_is_absorbed_as_data_not_a_real_nested_comment() {
        // HTML comments don't nest; "<!--" appearing again inside a
        // comment is just more comment data, ending at the first real
        // "-->" — per the comment-less-than-sign-bang-dash-dash state's
        // "reconsume in comment end state" behavior.
        assert_eq!(
            kinds(&tokenize("<!--<!-->")),
            vec![&TokenKind::Comment("<!".to_owned()), &TokenKind::Eof]
        );
    }

    #[test]
    fn incorrectly_opened_comment_falls_back_to_bogus_comment() {
        // "<!" not followed by "--", "DOCTYPE", or "[CDATA[" is a bogus
        // comment whose data is everything up to the next '>'.
        assert_eq!(
            kinds(&tokenize("<!weird>")),
            vec![&TokenKind::Comment("weird".to_owned()), &TokenKind::Eof]
        );
    }

    #[test]
    fn end_tag_with_invalid_first_character_falls_back_to_bogus_comment() {
        // "</1>": '1' can't start a tag name, so this becomes a bogus
        // comment whose data is "1" (the reconsumed invalid character).
        assert_eq!(
            kinds(&tokenize("</1>")),
            vec![&TokenKind::Comment("1".to_owned()), &TokenKind::Eof]
        );
    }

    #[test]
    fn cdata_outside_foreign_content_becomes_a_bogus_comment() {
        // No tree-construction "adjusted current node" exists at this
        // tokenizer-only layer, so "<![CDATA[...]]>" can never take the
        // real CDATA-section branch — it's always a bogus comment whose
        // data starts with the literal "[CDATA[".
        assert_eq!(
            kinds(&tokenize("<![CDATA[x]]>")),
            vec![
                &TokenKind::Comment("[CDATA[x]]".to_owned()),
                &TokenKind::Eof,
            ]
        );
    }

    fn expect_doctype(tokens: &[Token]) -> &DoctypeToken {
        match &tokens[0].kind {
            TokenKind::Doctype(doctype) => doctype,
            other => panic!("expected DOCTYPE token, got {other:?}"),
        }
    }

    #[test]
    fn simple_doctype() {
        let tokens = tokenize("<!DOCTYPE html>");
        assert_eq!(
            expect_doctype(&tokens),
            &DoctypeToken {
                name: Some("html".to_owned()),
                ..Default::default()
            }
        );
        assert_eq!(kinds(&tokens)[1], &TokenKind::Eof);
    }

    #[test]
    fn doctype_keyword_and_name_are_ascii_case_insensitive_lowercased() {
        let tokens = tokenize("<!doctype HTML>");
        assert_eq!(
            expect_doctype(&tokens),
            &DoctypeToken {
                name: Some("html".to_owned()),
                ..Default::default()
            }
        );
    }

    #[test]
    fn doctype_with_no_name_sets_force_quirks() {
        let tokens = tokenize("<!DOCTYPE>");
        assert_eq!(
            expect_doctype(&tokens),
            &DoctypeToken {
                force_quirks: true,
                ..Default::default()
            }
        );
    }

    #[test]
    fn doctype_with_public_and_system_identifiers() {
        let tokens = tokenize(
            r#"<!DOCTYPE html PUBLIC "-//W3C//DTD HTML 4.01//EN" "http://www.w3.org/TR/html4/strict.dtd">"#,
        );
        assert_eq!(
            expect_doctype(&tokens),
            &DoctypeToken {
                name: Some("html".to_owned()),
                public_identifier: Some("-//W3C//DTD HTML 4.01//EN".to_owned()),
                system_identifier: Some("http://www.w3.org/TR/html4/strict.dtd".to_owned()),
                force_quirks: false,
            }
        );
    }

    #[test]
    fn eof_inside_doctype_name_sets_force_quirks_but_keeps_the_partial_name() {
        let tokens = tokenize("<!DOCTYPE html");
        assert_eq!(
            tokens,
            vec![
                Token {
                    kind: TokenKind::Doctype(DoctypeToken {
                        name: Some("html".to_owned()),
                        force_quirks: true,
                        ..Default::default()
                    }),
                    position: pos(1, 1, 0),
                },
                Token {
                    kind: TokenKind::Eof,
                    position: pos(1, 15, 14),
                },
            ]
        );
    }

    #[test]
    fn unrecognized_text_after_doctype_name_falls_back_to_bogus_doctype() {
        // Neither "PUBLIC" nor "SYSTEM" — force_quirks is set and
        // everything up to '>' is ignored (not appended anywhere).
        let tokens = tokenize("<!DOCTYPE html GARBAGE>");
        assert_eq!(
            expect_doctype(&tokens),
            &DoctypeToken {
                name: Some("html".to_owned()),
                force_quirks: true,
                ..Default::default()
            }
        );
    }

    #[test]
    fn simple_processing_instruction() {
        let tokens = tokenize("<?foo bar?>");
        assert_eq!(
            kinds(&tokens),
            vec![
                &TokenKind::ProcessingInstruction(ProcessingInstructionToken {
                    target: "foo".to_owned(),
                    data: "bar".to_owned(),
                }),
                &TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn xml_target_is_disallowed_and_becomes_a_bogus_comment() {
        let tokens = tokenize(r#"<?xml version="1.0"?>"#);
        assert_eq!(
            kinds(&tokens),
            vec![
                &TokenKind::Comment("?xml version=\"1.0\"?".to_owned()),
                &TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn xml_stylesheet_target_is_disallowed_and_becomes_a_bogus_comment() {
        let tokens = tokenize("<?xml-stylesheet foo?>");
        assert_eq!(
            kinds(&tokens),
            vec![
                &TokenKind::Comment("?xml-stylesheet foo?".to_owned()),
                &TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn eof_right_after_question_mark_emits_only_eof_no_token_at_all() {
        // Not even a bogus comment: the temporary buffer is still empty at
        // this point, and processing-instruction-open-state's EOF branch
        // only ever emits an end-of-file token.
        assert_eq!(kinds(&tokenize("<?")), vec![&TokenKind::Eof]);
    }

    #[test]
    fn eof_inside_processing_instruction_data_discards_the_pi_token() {
        // Unlike DOCTYPE/comment EOF handling, the spec's processing-
        // instruction states never emit their in-progress token on EOF.
        assert_eq!(kinds(&tokenize("<?foo bar")), vec![&TokenKind::Eof]);
    }

    #[test]
    fn lone_question_marks_inside_processing_instruction_data_are_kept_literally() {
        let tokens = tokenize("<?foo a?b?>");
        assert_eq!(
            kinds(&tokens),
            vec![
                &TokenKind::ProcessingInstruction(ProcessingInstructionToken {
                    target: "foo".to_owned(),
                    data: "a?b".to_owned(),
                }),
                &TokenKind::Eof,
            ]
        );
    }

    /// Simulates the tree-builder: tokenizes normally, but the instant a
    /// start tag comes out, immediately calls `switch_to(state)` — same
    /// sequencing Phase 03 will actually use ("insert the element, then
    /// switch the tokenizer").
    fn tokenize_switching_after_start_tag(input: &str, state: ExternalState) -> Vec<Token> {
        let mut tokenizer = Tokenizer::new(input);
        let mut tokens = Vec::new();
        while let Some(token) = tokenizer.next() {
            let is_start = matches!(&token.kind, TokenKind::StartTag(_));
            tokens.push(token);
            if is_start {
                tokenizer.switch_to(state);
            }
        }
        tokens
    }

    fn characters_only(tokens: &[Token]) -> String {
        tokens
            .iter()
            .filter_map(|token| match &token.kind {
                TokenKind::Character(c) => Some(*c),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn rcdata_resolves_character_references_and_ends_on_matching_end_tag() {
        let tokens =
            tokenize_switching_after_start_tag("<title>AT&amp;T</title>", ExternalState::RcData);
        assert_eq!(
            kinds(&tokens),
            vec![
                &TokenKind::StartTag(TagToken {
                    name: "title".to_owned(),
                    self_closing: false,
                    attributes: vec![],
                }),
                &TokenKind::Character('A'),
                &TokenKind::Character('T'),
                &TokenKind::Character('&'),
                &TokenKind::Character('T'),
                &TokenKind::EndTag(TagToken {
                    name: "title".to_owned(),
                    self_closing: false,
                    attributes: vec![],
                }),
                &TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn rcdata_end_tag_with_wrong_name_is_kept_as_literal_text() {
        // "</b>" doesn't match the open "title" element, so it's not an
        // appropriate end tag token — RCDATA keeps consuming as text.
        let tokens =
            tokenize_switching_after_start_tag("<title>a</b>c</title>", ExternalState::RcData);
        assert_eq!(
            characters_only(&tokens[1..tokens.len() - 2]),
            "a</b>c".to_owned()
        );
        match &tokens.last().unwrap().kind {
            TokenKind::Eof => {}
            other => panic!("expected trailing Eof, got {other:?}"),
        }
        match &tokens[tokens.len() - 2].kind {
            TokenKind::EndTag(tag) => assert_eq!(tag.name, "title"),
            other => panic!("expected closing end tag, got {other:?}"),
        }
    }

    #[test]
    fn rawtext_does_not_resolve_character_references() {
        let tokens =
            tokenize_switching_after_start_tag("<style>&amp;</style>", ExternalState::RawText);
        assert_eq!(characters_only(&tokens), "&amp;".to_owned());
    }

    #[test]
    fn plaintext_never_recognizes_any_end_tag_ever_again() {
        // PLAINTEXT has no '<' branch at all in the spec — everything,
        // including what looks like a closing tag, is literal text.
        let tokens = tokenize_switching_after_start_tag(
            "<plaintext>a</plaintext>b",
            ExternalState::PlainText,
        );
        assert_eq!(characters_only(&tokens), "a</plaintext>b".to_owned());
        match &tokens.last().unwrap().kind {
            TokenKind::Eof => {}
            other => panic!("expected trailing Eof, got {other:?}"),
        }
    }

    #[test]
    fn appropriate_end_tag_requires_a_start_tag_to_have_been_emitted_first() {
        // Switching straight into RcData without ever having tokenized a
        // start tag: no name to compare against, so nothing can ever be
        // an appropriate end tag token, per spec.
        let mut tokenizer = Tokenizer::new("</title>");
        tokenizer.switch_to(ExternalState::RcData);
        let tokens: Vec<_> = tokenizer.collect();
        assert_eq!(characters_only(&tokens), "</title>".to_owned());
        assert_eq!(tokens.last().unwrap().kind, TokenKind::Eof);
    }

    #[test]
    fn script_data_plain_content_no_html_comment_like_wrapper() {
        let tokens = tokenize_switching_after_start_tag(
            "<script>var x = 1;</script>",
            ExternalState::ScriptData,
        );
        assert_eq!(characters_only(&tokens), "var x = 1;".to_owned());
        match &tokens[tokens.len() - 2].kind {
            TokenKind::EndTag(tag) => assert_eq!(tag.name, "script"),
            other => panic!("expected closing end tag, got {other:?}"),
        }
    }

    #[test]
    fn script_data_does_not_resolve_character_references_either() {
        // Same non-processing as RAWTEXT — script data has no '&' branch.
        let tokens =
            tokenize_switching_after_start_tag("<script>&amp;</script>", ExternalState::ScriptData);
        assert_eq!(characters_only(&tokens), "&amp;".to_owned());
    }

    #[test]
    fn script_data_html_comment_like_wrapper_round_trips_literally() {
        // The whole point of the escaped-state dance: it changes what the
        // tokenizer *tracks* internally, never what characters come out.
        let source = "<!--alert(1);-->";
        let tokens = tokenize_switching_after_start_tag(
            &format!("<script>{source}</script>"),
            ExternalState::ScriptData,
        );
        assert_eq!(characters_only(&tokens), source.to_owned());
        match &tokens[tokens.len() - 2].kind {
            TokenKind::EndTag(tag) => assert_eq!(tag.name, "script"),
            other => panic!("expected closing end tag, got {other:?}"),
        }
    }

    #[test]
    fn nested_script_tags_inside_html_comment_like_wrapper_do_not_end_the_element_early() {
        // The classic script-data torture case (double escaping):
        // "<script>" appearing inside the "<!--...-->"-wrapped content
        // switches into "double escaped" mode so that the matching
        // "</script>" *inside the comment* doesn't close the real
        // element — only the final, real "</script>" does. Character
        // output is still the literal source either way.
        let source = "<!--<script>x</script>-->";
        let tokens = tokenize_switching_after_start_tag(
            &format!("<script>{source}</script>"),
            ExternalState::ScriptData,
        );
        assert_eq!(characters_only(&tokens), source.to_owned());
        let end_tags: Vec<_> = tokens
            .iter()
            .filter(|token| matches!(&token.kind, TokenKind::EndTag(_)))
            .collect();
        assert_eq!(
            end_tags.len(),
            1,
            "only the real closing tag should be an end tag token, not the one nested inside the comment-like wrapper"
        );
    }

    #[test]
    fn script_data_end_tag_with_wrong_name_is_kept_as_literal_text() {
        let tokens = tokenize_switching_after_start_tag(
            "<script>a</scriptx>b</script>",
            ExternalState::ScriptData,
        );
        assert_eq!(characters_only(&tokens), "a</scriptx>b".to_owned());
        let end_tags: Vec<_> = tokens
            .iter()
            .filter_map(|token| match &token.kind {
                TokenKind::EndTag(tag) => Some(tag.name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(end_tags, vec!["script"]);
    }

    fn tokenize_in_foreign_content(input: &str) -> Vec<Token> {
        let mut tokenizer = Tokenizer::new(input);
        tokenizer.set_in_foreign_content(true);
        tokenizer.collect()
    }

    #[test]
    fn cdata_section_outside_foreign_content_still_becomes_a_bogus_comment() {
        // Regression check: the new `in_foreign_content` field defaults to
        // `false`, so behavior for plain HTML (no Phase 03 tree-builder
        // ever calling `set_in_foreign_content`) is unchanged from before
        // this feature existed.
        assert_eq!(
            kinds(&tokenize("<![CDATA[x]]>")),
            vec![
                &TokenKind::Comment("[CDATA[x]]".to_owned()),
                &TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn cdata_section_in_foreign_content_yields_character_tokens() {
        assert_eq!(
            kinds(&tokenize_in_foreign_content("<![CDATA[hi]]>")),
            vec![
                &TokenKind::Character('h'),
                &TokenKind::Character('i'),
                &TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn cdata_section_null_character_is_kept_literal_not_replaced() {
        // Unlike RCDATA/RAWTEXT/script-data, CDATA-section-state does not
        // replace NUL — the spec says that's handled later, in
        // tree-construction's "in foreign content" rules.
        assert_eq!(
            kinds(&tokenize_in_foreign_content("<![CDATA[\u{0}]]>")),
            vec![&TokenKind::Character('\u{0}'), &TokenKind::Eof]
        );
    }

    #[test]
    fn cdata_section_single_bracket_not_followed_by_another_is_literal_content() {
        let tokens = tokenize_in_foreign_content("<![CDATA[a]b]]>");
        assert_eq!(
            kinds(&tokens),
            vec![
                &TokenKind::Character('a'),
                &TokenKind::Character(']'),
                &TokenKind::Character('b'),
                &TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn cdata_section_three_consecutive_brackets_then_close_keeps_only_the_first_as_content() {
        // "]]]>": only the *last* two `]`s immediately before `>` can be
        // part of the `]]>` terminator, so the first `]` is confirmed
        // content — with its own (earlier) position, not the position of
        // whatever character triggered the flush.
        let tokens = tokenize_in_foreign_content("<![CDATA[]]]>");
        assert_eq!(
            tokens,
            vec![
                Token {
                    kind: TokenKind::Character(']'),
                    position: pos(1, 10, 9),
                },
                Token {
                    kind: TokenKind::Eof,
                    position: pos(1, 14, 13),
                },
            ]
        );
    }

    #[test]
    fn cdata_section_four_consecutive_brackets_then_close_keeps_first_two_as_content() {
        let tokens = tokenize_in_foreign_content("<![CDATA[]]]]>");
        assert_eq!(
            kinds(&tokens),
            vec![
                &TokenKind::Character(']'),
                &TokenKind::Character(']'),
                &TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn eof_inside_cdata_section_emits_eof_after_any_withheld_brackets_flush() {
        let tokens = tokenize_in_foreign_content("<![CDATA[a]");
        assert_eq!(
            kinds(&tokens),
            vec![
                &TokenKind::Character('a'),
                &TokenKind::Character(']'),
                &TokenKind::Eof,
            ]
        );
    }

    // Phase 07 parse-error tests (`plan/07-parse-errors.md`): one minimal
    // triggering input per implemented `ParseErrorKind` — not per call
    // site (several kinds fire from more than one state; one
    // representative site is enough to confirm the kind itself is wired
    // correctly). `EofInCdata`/`EofInScriptHtmlCommentLikeText` need
    // their own setup (foreign content / external state switching) and
    // get their own tests below the table.
    #[test]
    fn tokenizer_level_parse_errors_fire_with_the_right_kind() {
        let cases: &[(&str, ParseErrorKind)] = &[
            ("<![CDATA[x]]>", ParseErrorKind::CdataInHtmlContent),
            ("<!x>", ParseErrorKind::IncorrectlyOpenedComment),
            ("<!-->", ParseErrorKind::AbruptClosingOfEmptyComment),
            ("<!--<!--x-->", ParseErrorKind::NestedComment),
            ("<!--x--!>", ParseErrorKind::IncorrectlyClosedComment),
            ("<!--", ParseErrorKind::EofInComment),
            ("<1>", ParseErrorKind::InvalidFirstCharacterOfTagName),
            ("<", ParseErrorKind::EofBeforeTagName),
            ("</>", ParseErrorKind::MissingEndTagName),
            ("<p x=", ParseErrorKind::EofInTag),
            (r#"<p id="a" id="b">"#, ParseErrorKind::DuplicateAttribute),
            ("\0", ParseErrorKind::UnexpectedNullCharacter),
            (
                r#"<p ">"#,
                ParseErrorKind::UnexpectedCharacterInAttributeName,
            ),
            ("<p x=>", ParseErrorKind::MissingAttributeValue),
            (
                r#"<p x=a"b>"#,
                ParseErrorKind::UnexpectedCharacterInUnquotedAttributeValue,
            ),
            (
                r#"<p x="a"y="b">"#,
                ParseErrorKind::MissingWhitespaceBetweenAttributes,
            ),
            ("<p/ x>", ParseErrorKind::UnexpectedSolidusInTag),
            (
                "<p =>",
                ParseErrorKind::UnexpectedEqualsSignBeforeAttributeName,
            ),
            ("&zzz;", ParseErrorKind::UnknownNamedCharacterReference),
            (
                "&#;",
                ParseErrorKind::AbsenceOfDigitsInNumericCharacterReference,
            ),
            (
                "&#65 ",
                ParseErrorKind::MissingSemicolonAfterCharacterReference,
            ),
            ("&#0;", ParseErrorKind::NullCharacterReference),
            (
                "&#x110000;",
                ParseErrorKind::CharacterReferenceOutsideUnicodeRange,
            ),
            ("&#xD800;", ParseErrorKind::SurrogateCharacterReference),
            ("&#xFFFE;", ParseErrorKind::NoncharacterCharacterReference),
            ("&#x01;", ParseErrorKind::ControlCharacterReference),
            (
                "<!DOCTYPEhtml>",
                ParseErrorKind::MissingWhitespaceBeforeDoctypeName,
            ),
            ("<!DOCTYPE >", ParseErrorKind::MissingDoctypeName),
            (
                "<!DOCTYPE html foo>",
                ParseErrorKind::InvalidCharacterSequenceAfterDoctypeName,
            ),
            (
                r#"<!DOCTYPE html PUBLIC "a""b">"#,
                ParseErrorKind::MissingWhitespaceBetweenDoctypePublicAndSystemIdentifiers,
            ),
            (
                r#"<!DOCTYPE html SYSTEM "a"b>"#,
                ParseErrorKind::UnexpectedCharacterAfterDoctypeSystemIdentifier,
            ),
            ("<!DOCTYPE", ParseErrorKind::EofInDoctype),
            ("<?", ParseErrorKind::EofInProcessingInstruction),
            (
                "<? >",
                ParseErrorKind::InvalidFirstCharacterOfProcessingInstructionTarget,
            ),
            ("<?a$>", ParseErrorKind::InvalidProcessingInstructionTarget),
            (
                "<?xml?>",
                ParseErrorKind::DisallowedProcessingInstructionTarget,
            ),
        ];
        for (input, expected_kind) in cases {
            let errors = errors_for(input);
            assert!(
                errors.contains(expected_kind),
                "input {input:?}: expected {expected_kind:?} among {errors:?}"
            );
        }
    }

    #[test]
    fn eof_in_cdata_fires_in_foreign_content() {
        let mut tokenizer = Tokenizer::new("<![CDATA[abc");
        tokenizer.set_in_foreign_content(true);
        for _ in tokenizer.by_ref() {}
        let kinds: Vec<_> = tokenizer
            .take_errors()
            .into_iter()
            .map(|error| error.kind)
            .collect();
        assert!(kinds.contains(&ParseErrorKind::EofInCdata));
    }

    #[test]
    fn eof_in_script_html_comment_like_text_fires_mid_wrapper() {
        let mut tokenizer = Tokenizer::new("<script><!--x");
        while let Some(token) = tokenizer.next() {
            if matches!(&token.kind, TokenKind::StartTag(_)) {
                tokenizer.switch_to(ExternalState::ScriptData);
            }
        }
        let kinds: Vec<_> = tokenizer
            .take_errors()
            .into_iter()
            .map(|error| error.kind)
            .collect();
        assert!(kinds.contains(&ParseErrorKind::EofInScriptHtmlCommentLikeText));
    }
}
