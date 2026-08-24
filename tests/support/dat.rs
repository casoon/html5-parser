//! Parser for the html5lib-tests tree-construction `.dat` format. See
//! `tests/html5lib-tests/README.md` for the format and its provenance.
//!
//! Ported 1:1 from the reference implementation
//! (`resources/test.js`'s `parseDat()` in
//! <https://github.com/web-platform-tests/wpt/tree/master/html/syntax/parsing>,
//! itself described there as mirroring html5lib-python's own `TestData`
//! parser) rather than re-derived from the format's prose description —
//! getting the trailing-newline bookkeeping subtly wrong would corrupt
//! any multi-line `#data`/`#document` content (e.g. a text node whose
//! content itself contains a literal newline, or CDATA spanning several
//! lines), and a straight port sidesteps re-litigating those edge cases.

use std::collections::HashMap;

/// A single test case parsed out of a `.dat` file.
pub struct TestCase {
    /// The `#data` section's content — passed to `parse()` unchanged.
    pub data: String,
    /// True if a `#document-fragment` section is present. This crate has
    /// no fragment parsing, so callers should skip these.
    pub is_fragment: bool,
    /// True if a `#script-on` section is present. This crate always
    /// models scripting as disabled, so a case that only holds under
    /// scripting-enabled behavior doesn't apply — callers should skip
    /// these. (`#script-off`-only and unmarked cases, meant to hold
    /// under both modes, need no special handling: they're simply run.)
    pub script_on_only: bool,
    /// The expected `#document` dump, as one string with embedded `\n`
    /// between (and, for multi-line text nodes, within) its `"| "`-style
    /// lines — directly comparable against `dump::dump_document`'s
    /// output via plain string equality, the same way the reference
    /// runner's `assert_equals(actual, expected)` does.
    pub expected_document: String,
}

/// Returns the section name for a line like `"#data\n"` (already
/// including its trailing newline, matching the reference
/// implementation's per-line reconstruction) — `Some("data")` — or
/// `None` if the line isn't a `#`-prefixed section heading.
fn section_heading(line: &str) -> Option<String> {
    let rest = line.strip_prefix('#')?;
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

/// Strips exactly one trailing `\n` from every section buffer, matching
/// `lines.join("\n")` semantics (each content line contributed a
/// trailing `\n`; the join has none at the very end).
fn normalise(mut sections: HashMap<String, String>) -> HashMap<String, String> {
    for value in sections.values_mut() {
        if let Some(stripped) = value.strip_suffix('\n') {
            *value = stripped.to_owned();
        }
    }
    sections
}

/// Parses every test case out of a `.dat` file's contents.
pub fn parse_dat_file(contents: &str) -> Vec<TestCase> {
    let text = contents.strip_suffix('\n').unwrap_or(contents);
    let file_lines: Vec<&str> = text.split('\n').collect();
    let last_index = file_lines.len().saturating_sub(1);

    let mut cases: Vec<HashMap<String, String>> = Vec::new();
    let mut data: Option<HashMap<String, String>> = None;
    let mut key: Option<String> = None;

    for (i, raw_line) in file_lines.iter().enumerate() {
        // Every line gets its "\n" back except the file's very last line
        // (the file's own trailing newline was already stripped above).
        let line = if i == last_index {
            (*raw_line).to_owned()
        } else {
            format!("{raw_line}\n")
        };

        if let Some(heading) = section_heading(&line) {
            if heading == "data" {
                // A new test starts: finalize whatever was accumulating.
                if let Some(mut finished) = data.take() {
                    // The blank line separating this test from the next
                    // contributed one extra character (its own "\n") to
                    // the previous section's buffer, on top of the
                    // "one \n per content line" that `normalise` below
                    // accounts for — trim that one first.
                    if let Some(k) = &key
                        && let Some(buffer) = finished.get_mut(k)
                        && !buffer.is_empty()
                    {
                        buffer.pop();
                    }
                    cases.push(normalise(finished));
                }
            }
            data.get_or_insert_with(HashMap::new)
                .insert(heading.clone(), String::new());
            key = Some(heading);
        } else if let Some(k) = &key
            && let Some(sections) = data.as_mut()
        {
            sections.entry(k.clone()).or_default().push_str(&line);
        }
    }
    if let Some(finished) = data.take() {
        cases.push(normalise(finished));
    }

    cases
        .into_iter()
        .map(|mut sections| TestCase {
            data: sections.remove("data").unwrap_or_default(),
            is_fragment: sections.contains_key("document-fragment"),
            script_on_only: sections.contains_key("script-on"),
            expected_document: sections.remove("document").unwrap_or_default(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::parse_dat_file;

    #[test]
    fn parses_a_single_simple_case() {
        let cases = parse_dat_file(
            "#data\n<p>Hi\n#errors\n(1,0): some-error\n#document\n| <html>\n|   <head>\n|   <body>\n|     <p>\n|       \"Hi\"\n",
        );
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].data, "<p>Hi");
        assert!(!cases[0].is_fragment);
        assert!(!cases[0].script_on_only);
        assert_eq!(
            cases[0].expected_document,
            "| <html>\n|   <head>\n|   <body>\n|     <p>\n|       \"Hi\""
        );
    }

    #[test]
    fn data_section_preserves_internal_blank_lines() {
        let cases = parse_dat_file("#data\nfoo\n\nbar\n#errors\n#document\n| \"foo\n\nbar\"\n");
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].data, "foo\n\nbar");
        assert_eq!(cases[0].expected_document, "| \"foo\n\nbar\"");
    }

    #[test]
    fn multi_line_document_text_node_is_preserved_verbatim() {
        // Matches domjs-unsafe.dat's `<svg><![CDATA[foo\nbar]]>` case: the
        // expected dump's text-node line itself spans two physical lines.
        let cases = parse_dat_file(
            "#data\n<svg><![CDATA[foo\nbar]]>\n#errors\n#document\n| <html>\n|   \"foo\nbar\"\n",
        );
        assert_eq!(cases[0].data, "<svg><![CDATA[foo\nbar]]>");
        assert_eq!(cases[0].expected_document, "| <html>\n|   \"foo\nbar\"");
    }

    #[test]
    fn splits_multiple_cases_separated_by_a_blank_line() {
        let contents = "\
#data
A
#errors
#document
| \"A\"

#data
B
#errors
#document
| \"B\"
";
        let cases = parse_dat_file(contents);
        assert_eq!(cases.len(), 2);
        assert_eq!(cases[0].data, "A");
        assert_eq!(cases[0].expected_document, "| \"A\"");
        assert_eq!(cases[1].data, "B");
        assert_eq!(cases[1].expected_document, "| \"B\"");
    }

    #[test]
    fn document_fragment_and_script_on_are_flagged() {
        let fragment =
            parse_dat_file("#data\n<td>\n#errors\n#document-fragment\ntd\n#document\n| <td>\n");
        assert!(fragment[0].is_fragment);

        let script_on = parse_dat_file("#data\n<p>\n#errors\n#script-on\n#document\n| <p>\n");
        assert!(script_on[0].script_on_only);

        let plain = parse_dat_file("#data\n<p>\n#errors\n#document\n| <p>\n");
        assert!(!plain[0].is_fragment);
        assert!(!plain[0].script_on_only);
    }

    #[test]
    fn last_test_in_a_file_needs_no_separator_trim() {
        // No trailing blank line after the last case — only the
        // "one \n per content line" trim applies, not the extra
        // separator-line trim.
        let cases = parse_dat_file("#data\nA\n#errors\n#document\n| \"A\"");
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].expected_document, "| \"A\"");
    }
}
