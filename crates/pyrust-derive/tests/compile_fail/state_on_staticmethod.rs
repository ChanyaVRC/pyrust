//! `#[state]` / `#[state_mut]` cannot be used on `#[staticmethod]` or `#[classmethod]`.

use pyrust_derive::pyrust_module;

const MODULE_NAME: &str = "test";
const FN_PREFIX: &str = "";

pyrust_module! {
    #[native_state(u32)]
    class Bad {
        #[staticmethod]
        fn read(#[state] st: &u32) -> Result<Value> {
            unimplemented!()
        }
    }
}

fn main() {}
