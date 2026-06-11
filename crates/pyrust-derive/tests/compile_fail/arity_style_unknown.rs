//! `#[arity_style(...)]` only accepts the three known wording names
//! (`standard`, `takes_exactly_one`, `expected_got`).  A typo is rejected
//! at parse time with a list of the valid values (#2331).

use pyrust_derive::pyrust_module;

const MODULE_NAME: &str = "test";
const FN_PREFIX: &str = "";

pyrust_module! {
    #[arity_style(takes_one)]
    fn bad(#[positional_only] x: PyInt) -> Result<Value> {
        unimplemented!()
    }
}

fn main() {}
