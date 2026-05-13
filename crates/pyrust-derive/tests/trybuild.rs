//! `trybuild`-driven compile-fail tests for `pyrust_module!`'s parse-time
//! diagnostics.  Each fixture in `tests/compile_fail/` exercises one
//! invalid-input shape and pins both *that compilation fails* and *what
//! the diagnostic says* via the sibling `.stderr` snapshot.
//!
//! Running locally:
//!
//! ```text
//! cargo test --package pyrust-derive --test trybuild
//! ```
//!
//! Updating snapshots (after intentionally tweaking a diagnostic):
//!
//! ```text
//! TRYBUILD=overwrite cargo test --package pyrust-derive --test trybuild
//! ```
//!
//! The fixtures rely on the macro emitting `compile_error!` from
//! `syn::Error` at parse time — so they don't need stub definitions for
//! the symbols the macro's *successful* expansion would reference.

#[test]
fn compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile_fail/*.rs");
}
