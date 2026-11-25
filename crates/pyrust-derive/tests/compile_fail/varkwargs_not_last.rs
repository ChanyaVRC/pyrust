//! `#[varkwargs]` must be the last parameter in a signature.

use pyrust_derive::pyrust_module;

const MODULE_NAME: &str = "test";
const FN_PREFIX: &str = "";

pyrust_module! {
    fn bad(#[varkwargs] kw: IndexMap<String, Value>, extra: PyInt) -> Result<Value> {
        unimplemented!()
    }
}

fn main() {}
