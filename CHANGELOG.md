# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Releases before 0.4.0 are described in the git history only.

## [0.4.0] - Unreleased

`parse()` now reports tree-construction parse errors (WHATWG §13.2.6) that
0.3.0 dropped silently. The parsed tree is unchanged (html5lib-tests
tree-construction conformance stays at 1,726/1,726), but `ParseResult::errors`
contains more entries for the same input, hence the minor version bump.

### Added
- `ParseErrorKind::StrayStartTag`: a start tag the spec ignores (or merges into
  an existing element) with a parse error: `html`, `body` and `frameset` in
  body, a second `head`, `caption`/`col`/`colgroup`/`frame`/`tbody`/`td`/
  `tfoot`/`th`/`thead`/`tr` outside a table, and a `select` start tag while a
  `select` is open.
- `ParseErrorKind::NestedFormattingElement`: an `a` start tag while an `a` is
  still in the list of active formatting elements, or `nobr` while a `nobr` is
  in scope (§13.2.6.4.7).
- `ParseErrorKind::MisnestedFormattingElement`: adoption agency algorithm step
  4.6, e.g. `</b>` in `<b><i>x</b></i>`.
- `ParseErrorKind::FormattingElementNotInScope`: adoption agency algorithm step
  4.5, e.g. `</b>` in `<b><table></b>`.

### Changed
- `StrayEndTag` is now also reported when an end tag has no matching element
  in scope: `</div>`, `</header>`, `</li>`, `</dd>`/`</dt>`, `</h1>`–`</h6>`,
  `</form>`, `</applet>`/`</marquee>`/`</object>`, `</body>`/`</html>` (the
  whole "in body" end-tag group), any other end tag in "before html", "before
  head", "in head", "in head noscript", "after head" and "in template", and
  adoption agency step 4.4 (e.g. `</i>` in `<b><i>x</b></i>`).
  Previously only the "any other end tag" path (`</span>`) reported it.
- `StrayEndTagInTable` is now also reported by "in caption", "in column group",
  "in table body", "in row" and "in cell" for the end tags they ignore, and
  `MisplacedTokenInTable` for the start tags they ignore and for a `form` start
  tag in a table.
- `StrayDoctype` is now also reported inside foreign content (§13.2.6.5).
