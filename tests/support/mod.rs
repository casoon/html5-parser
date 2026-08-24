// Shared, non-test helper code for `tests/html5lib_conformance.rs`. Lives
// under `tests/support/` (a directory, not `tests/support.rs`) so cargo
// doesn't treat it as its own separate test binary — see
// https://doc.rust-lang.org/book/ch11-03-test-organization.html#submodules-in-integration-tests.

pub mod dat;
pub mod dump;
