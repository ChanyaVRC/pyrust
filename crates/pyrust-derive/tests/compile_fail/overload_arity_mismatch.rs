//! Overload-set compatibility — every overload must have the same
//! arity.  Different arities should be a single signature with
//! `Option<T>` + `#[default(None)]`, not multiple overloads.

use pyrust_derive::pyrust_module;

const MODULE_NAME: &str = "test";
const FN_PREFIX: &str = "";

pyrust_module! {
    fn bad(x: PyInt) -> Result<Value> {
        unimplemented!()
    }

    fn bad(x: PyFloat, y: PyFloat) -> Result<Value> {
        unimplemented!()
    }
}

fn main() {}
