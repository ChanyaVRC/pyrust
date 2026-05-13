//! Overload-set compatibility — the `#[keyword_only]` flag must
//! match across all overloads.  See the sibling `positional_only`
//! fixture for the rationale; same idea, opposite flag.

use pyrust_derive::pyrust_module;

const MODULE_NAME: &str = "test";
const FN_PREFIX: &str = "";

pyrust_module! {
    fn bad(#[keyword_only] x: PyInt) -> Result<Value> {
        unimplemented!()
    }

    fn bad(x: PyFloat) -> Result<Value> {
        unimplemented!()
    }
}

fn main() {}
