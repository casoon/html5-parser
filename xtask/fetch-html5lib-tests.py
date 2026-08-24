#!/usr/bin/env python3
"""Vendors the WHATWG/html5lib tree-construction conformance test corpus
into `tests/html5lib-tests/`.

Source: the html5lib-tests project's own README (as of 2026-08-24) says
tree-construction tests are "now solely maintained on
web-platform-tests" — the `.dat` fixture files (same format html5lib-tests
always used: `#data`/`#errors`/`#document-fragment`/`#script-off`/
`#script-on`/`#document` sections, see `tests/html5lib-tests/README.md`
after fetching) now live at
<https://github.com/web-platform-tests/wpt/tree/master/html/syntax/parsing/resources>,
consumed there by `html5lib_write.html`'s browser-based test runner. This
script fetches the raw `.dat` files directly — this crate's harness
(`tests/html5lib_conformance.rs`) reads them itself rather than running
wpt's browser harness.

FILES below is every `.dat` file present in that directory as of
2026-08-24 (checked via the GitHub API's directory listing, not
hardcoded from memory) — the full set, not a curated subset: which test
cases actually apply to this crate (no fragment parsing, no scripting) is
a per-test-case decision the conformance harness itself makes at run time
(skip `#document-fragment`/`#script-on` cases) — see
`tests/html5lib_conformance.rs`. Filtering at vendor time would hide that
decision instead of making it inspectable. If wpt adds new `.dat` files
later, re-derive this list from the API
(`gh api repos/web-platform-tests/wpt/contents/html/syntax/parsing/resources`)
rather than guessing.

Usage:
    python3 xtask/fetch-html5lib-tests.py

Network access required (fetches each `.dat` file from
raw.githubusercontent.com); pass a local directory of already-downloaded
`.dat` files as the first argument to regenerate offline from a saved
copy instead:
    python3 xtask/fetch-html5lib-tests.py /path/to/local/dat/files
"""

import sys
import urllib.request
from pathlib import Path

RAW_BASE_URL = (
    "https://raw.githubusercontent.com/web-platform-tests/wpt/master/"
    "html/syntax/parsing/resources"
)
DEST = Path(__file__).resolve().parent.parent / "tests" / "html5lib-tests"

FILES = [
    "adoption01.dat",
    "adoption02.dat",
    "blocks.dat",
    "comments01.dat",
    "doctype01.dat",
    "domjs-unsafe.dat",
    "entities01.dat",
    "entities02.dat",
    "foreign-fragment.dat",
    "html5test-com.dat",
    "inbody01.dat",
    "isindex.dat",
    "main-element.dat",
    "math.dat",
    "menuitem-element.dat",
    "namespace-sensitivity.dat",
    "noscript01.dat",
    "pending-spec-changes-plain-text-unsafe.dat",
    "pending-spec-changes.dat",
    "plain-text-unsafe.dat",
    "processing-instructions.dat",
    "quirks01.dat",
    "ruby.dat",
    "scriptdata01.dat",
    "scripted_adoption01.dat",
    "scripted_ark.dat",
    "scripted_foster01.dat",
    "scripted_webkit01.dat",
    "search-element.dat",
    "svg.dat",
    "tables01.dat",
    "template.dat",
    "tests1.dat",
    "tests10.dat",
    "tests11.dat",
    "tests12.dat",
    "tests14.dat",
    "tests15.dat",
    "tests16.dat",
    "tests17.dat",
    "tests18.dat",
    "tests19.dat",
    "tests2.dat",
    "tests20.dat",
    "tests21.dat",
    "tests22.dat",
    "tests23.dat",
    "tests24.dat",
    "tests25.dat",
    "tests26.dat",
    "tests3.dat",
    "tests4.dat",
    "tests5.dat",
    "tests6.dat",
    "tests7.dat",
    "tests8.dat",
    "tests9.dat",
    "tests_innerHTML_1.dat",
    "tricky01.dat",
    "void-in-phrasing.dat",
    "webkit01.dat",
    "webkit02.dat",
]


def fetch_file(name):
    with urllib.request.urlopen(f"{RAW_BASE_URL}/{name}") as response:
        return response.read()


def main():
    DEST.mkdir(parents=True, exist_ok=True)
    if len(sys.argv) > 1:
        source_dir = Path(sys.argv[1])
        for name in FILES:
            (DEST / name).write_bytes((source_dir / name).read_bytes())
    else:
        for name in FILES:
            (DEST / name).write_bytes(fetch_file(name))
    print(f"Vendored {len(FILES)} .dat files into {DEST}")


if __name__ == "__main__":
    main()
