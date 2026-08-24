//! Runs this crate's `parse()` against the vendored html5lib-tests
//! tree-construction corpus (`tests/html5lib-tests/*.dat`, see that
//! directory's `README.md`) and compares the resulting tree, dumped via
//! `support::dump`, against each case's expected `#document` section.
//!
//! Per-case applicability (not per-file — a single `.dat` file can mix
//! applicable and inapplicable cases):
//! - `#document-fragment` cases are skipped: this crate has no fragment
//!   parsing.
//! - `#script-on`-only cases are skipped: this crate always models
//!   scripting as disabled.
//! - Everything else runs for real.
//!
//! Known failures (frameset/template gaps — see README.md's "Known
//! limitations" — and any real bugs the corpus surfaces) are tracked
//! explicitly in `tests/html5lib_known_failures.txt` rather than
//! silently ignored: an unlisted failure fails this test (regression
//! detection), and so does a listed entry that unexpectedly starts
//! passing (keeps the list honest). Regenerate it with
//! `HTML5LIB_BLESS=1 cargo test --test html5lib_conformance` after
//! investigating each new entry — see that file's own header.

mod support;

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use support::dat::parse_dat_file;
use support::dump::dump_document;

const VENDORED_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/html5lib-tests");
const KNOWN_FAILURES_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/html5lib_known_failures.txt"
);
/// Everything up to and including this line in `tests/html5lib_known_failures.txt`
/// is the fixed header (see that file); `HTML5LIB_BLESS` rewrites only
/// what follows it.
const KNOWN_FAILURES_HEADER_LAST_LINE: &str = "# --- entries below, one per line ---\n";

struct Failure {
    key: String,
    detail: String,
}

fn load_known_failures() -> HashSet<String> {
    let contents = fs::read_to_string(KNOWN_FAILURES_PATH).unwrap_or_default();
    contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.split_whitespace().next().unwrap_or(line).to_owned())
        .collect()
}

fn vendored_dat_files() -> Vec<PathBuf> {
    let mut files: Vec<_> = fs::read_dir(VENDORED_DIR)
        .unwrap_or_else(|error| {
            panic!(
                "reading {VENDORED_DIR}: {error} — run `python3 xtask/fetch-html5lib-tests.py` \
                 to vendor the html5lib-tests corpus first"
            )
        })
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("dat"))
        .collect();
    files.sort();
    files
}

fn indent_block(text: &str) -> String {
    text.lines()
        .map(|line| format!("    {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Rewrites `tests/html5lib_known_failures.txt`'s entry list (keeping
/// its fixed header) from the current run's failures.
fn bless(failures: &[Failure]) {
    let mut keys: Vec<&str> = failures
        .iter()
        .map(|failure| failure.key.as_str())
        .collect();
    keys.sort_unstable();

    let existing = fs::read_to_string(KNOWN_FAILURES_PATH).unwrap_or_default();
    let header_end = existing
        .find(KNOWN_FAILURES_HEADER_LAST_LINE)
        .unwrap_or_else(|| {
            panic!(
                "{KNOWN_FAILURES_PATH}'s header no longer contains the expected marker line \
                 {KNOWN_FAILURES_HEADER_LAST_LINE:?} — fix the header (or this constant) rather \
                 than blessing, or the entry list below it will be silently dropped"
            )
        })
        + KNOWN_FAILURES_HEADER_LAST_LINE.len();
    let mut out = existing[..header_end].to_owned();
    out.push('\n');
    for key in &keys {
        out.push_str(key);
        out.push('\n');
    }
    fs::write(KNOWN_FAILURES_PATH, out).expect("writing known-failures file");
    println!(
        "HTML5LIB_BLESS: wrote {} known-failing case(s) to {KNOWN_FAILURES_PATH}",
        keys.len()
    );
}

#[test]
fn html5lib_tree_construction_conformance() {
    let dat_files = vendored_dat_files();
    assert!(
        !dat_files.is_empty(),
        "no .dat files found in {VENDORED_DIR} — run `python3 xtask/fetch-html5lib-tests.py`"
    );

    let mut total = 0usize;
    let mut skipped_fragment = 0usize;
    let mut skipped_scripting = 0usize;
    let mut passed = 0usize;
    let mut failures: Vec<Failure> = Vec::new();

    for path in &dat_files {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("vendored .dat file name is valid UTF-8")
            .to_owned();
        let contents = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));

        for (index, case) in parse_dat_file(&contents).into_iter().enumerate() {
            total += 1;

            if case.is_fragment {
                skipped_fragment += 1;
                continue;
            }
            if case.script_on_only {
                skipped_scripting += 1;
                continue;
            }

            let document = html5_parser::parse(&case.data);
            let actual = dump_document(&document);

            if actual == case.expected_document {
                passed += 1;
            } else {
                let key = format!("{file_name}#{index}");
                let detail = format!(
                    "{key} (input: {:?})\n  expected:\n{}\n  actual:\n{}",
                    case.data,
                    indent_block(&case.expected_document),
                    indent_block(&actual),
                );
                failures.push(Failure { key, detail });
            }
        }
    }

    println!(
        "html5lib conformance: {passed}/{total} passed, {} failing, \
         {skipped_fragment} skipped (fragment), {skipped_scripting} skipped (scripting)",
        failures.len(),
    );

    if std::env::var_os("HTML5LIB_BLESS").is_some() {
        bless(&failures);
        return;
    }

    let known_failures = load_known_failures();
    let failing_keys: HashSet<&str> = failures
        .iter()
        .map(|failure| failure.key.as_str())
        .collect();

    let unexpected: Vec<&str> = failures
        .iter()
        .filter(|failure| !known_failures.contains(&failure.key))
        .map(|failure| failure.detail.as_str())
        .collect();
    let stale: Vec<&String> = known_failures
        .iter()
        .filter(|key| !failing_keys.contains(key.as_str()))
        .collect();

    assert!(
        unexpected.is_empty(),
        "{} test case(s) failed that are not in tests/html5lib_known_failures.txt:\n\n{}\n\n\
         If these are real, understood gaps (see README.md's \"Known limitations\") or newly \
         found bugs worth tracking rather than fixing immediately, add them via \
         `HTML5LIB_BLESS=1 cargo test --test html5lib_conformance` — don't hand-edit the list.",
        unexpected.len(),
        unexpected.join("\n\n"),
    );
    assert!(
        stale.is_empty(),
        "tests/html5lib_known_failures.txt lists {} case(s) that now pass (or no longer exist) \
         — regenerate it with `HTML5LIB_BLESS=1 cargo test --test html5lib_conformance`: {:?}",
        stale.len(),
        stale,
    );
}
