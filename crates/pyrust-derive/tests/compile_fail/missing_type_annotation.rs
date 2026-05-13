//! A typed parameter (one that carries a `#[default(...)]` /
//! `#[positional_only]` / `#[keyword_only]` attribute) must spell its
//! type explicitly.  Forgetting `: PyStr` triggers a parse-time error
//! pointing at the parameter ident — far more actionable than the
//! "`FromValue` not implemented for `()`" diagnostic the unguarded
//! macro would otherwise produce from expanded code.

use pyrust_derive::pyrust_module;

// Stubs for the constants the macro reads from the surrounding scope.
// We only need them to satisfy `pyrust_module!`'s own bookkeeping; the
// macro fails before any of them is referenced from emitted code.
const MODULE_NAME: &str = "test";
const FN_PREFIX: &str = "";

pyrust_module! {
    fn bad(#[default("r".into())] mode) -> Result<Value> {
        unimplemented!()
    }
}

fn main() {}
