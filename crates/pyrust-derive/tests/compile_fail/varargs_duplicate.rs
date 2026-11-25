//! Duplicate `#[varargs]` parameter is rejected at parse time.

use pyrust_derive::pyrust_module;

const MODULE_NAME: &str = "test";
const FN_PREFIX: &str = "";

pyrust_module! {
    fn bad(#[varargs] a: &[Value], #[varargs] b: &[Value]) -> Result<Value> {
        unimplemented!()
    }
}

fn main() {}
