//! `#[state]` / `#[state_mut]` requires `#[native_state(MyState)]` on the enclosing class.

use pyrust_derive::pyrust_module;

const MODULE_NAME: &str = "test";
const FN_PREFIX: &str = "";

pyrust_module! {
    class Bad {
        fn read(#[state] st: &u32) -> Result<Value> {
            unimplemented!()
        }
    }
}

fn main() {}
