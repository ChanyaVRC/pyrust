//! All overloads share one dispatcher, so a later overload that disagrees
//! on `#[arity_style]` with the head overload would be silently ignored.
//! The macro rejects it because one overload set shares one dispatcher (#2331).

use pyrust_derive::pyrust_module;

const MODULE_NAME: &str = "test";
const FN_PREFIX: &str = "";

pyrust_module! {
    #[arity_style(expected_got)]
    fn bad(#[positional_only] x: PyInt) -> Result<Value> {
        unimplemented!()
    }

    fn bad(#[positional_only] x: PyStr) -> Result<Value> {
        unimplemented!()
    }
}

fn main() {}
