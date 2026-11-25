//! `#[varargs]` cannot be combined with `#[default(...)]`.

use pyrust_derive::pyrust_module;

const MODULE_NAME: &str = "test";
const FN_PREFIX: &str = "";

pyrust_module! {
    fn bad(#[varargs] #[default(&[])] args: &[Value]) -> Result<Value> {
        unimplemented!()
    }
}

fn main() {}
