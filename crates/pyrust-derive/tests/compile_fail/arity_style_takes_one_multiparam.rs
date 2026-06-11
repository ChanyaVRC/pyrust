//! `#[arity_style(takes_exactly_one)]` hard-codes the "exactly one
//! argument" wording, so it's only valid on a single-positional-parameter
//! signature with no default (#2331).

use pyrust_derive::pyrust_module;

const MODULE_NAME: &str = "test";
const FN_PREFIX: &str = "";

pyrust_module! {
    #[arity_style(takes_exactly_one)]
    fn bad(#[positional_only] x: PyInt, #[positional_only] y: PyInt) -> Result<Value> {
        unimplemented!()
    }
}

fn main() {}
