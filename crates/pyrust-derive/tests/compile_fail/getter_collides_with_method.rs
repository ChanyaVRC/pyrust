//! A `#[getter]`/`#[setter]` property cannot collide with a method of the same name.

use pyrust_derive::pyrust_module;

const MODULE_NAME: &str = "test";
const FN_PREFIX: &str = "";

pyrust_module! {
    class Bad {
        #[getter]
        fn size(args) -> Result<Value> {
            unimplemented!()
        }

        fn size(args) -> Result<Value> {
            unimplemented!()
        }
    }
}

fn main() {}
