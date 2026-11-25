//! A `#[positional_only]` parameter cannot appear after `#[varargs]`.

use pyrust_derive::pyrust_module;

const MODULE_NAME: &str = "test";
const FN_PREFIX: &str = "";

pyrust_module! {
    fn bad(#[varargs] rest: &[Value], #[positional_only] x: PyInt) -> Result<Value> {
        unimplemented!()
    }
}

fn main() {}
