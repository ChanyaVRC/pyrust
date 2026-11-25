//! Duplicate `#[getter]` for the same property name is rejected.

use pyrust_derive::pyrust_module;

const MODULE_NAME: &str = "test";
const FN_PREFIX: &str = "";

pyrust_module! {
    class Bad {
        #[getter]
        fn value(args) -> Result<Value> {
            unimplemented!()
        }

        #[getter("value")]
        fn get_value(args) -> Result<Value> {
            unimplemented!()
        }
    }
}

fn main() {}
