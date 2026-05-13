//! Overload-set compatibility — the `#[positional_only]` flag must
//! match across all overloads of a set, because the dispatcher's
//! shared kwarg-validation step computes the allowed-kwargs list
//! once from the first overload.  Disagreement would mean some
//! overloads silently accept call patterns the others reject.

use pyrust_derive::pyrust_module;

const MODULE_NAME: &str = "test";
const FN_PREFIX: &str = "";

pyrust_module! {
    fn bad(#[positional_only] x: PyInt) -> Result<Value> {
        unimplemented!()
    }

    fn bad(x: PyFloat) -> Result<Value> {
        unimplemented!()
    }
}

fn main() {}
