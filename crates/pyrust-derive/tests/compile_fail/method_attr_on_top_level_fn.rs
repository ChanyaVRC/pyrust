//! `#[classmethod]` / `#[staticmethod]` / `#[getter]` / `#[setter]` are only
//! valid on methods inside a `class { … }` block.

use pyrust_derive::pyrust_module;

const MODULE_NAME: &str = "test";
const FN_PREFIX: &str = "";

pyrust_module! {
    #[classmethod]
    fn bad(args) -> Result<Value> {
        unimplemented!()
    }
}

fn main() {}
