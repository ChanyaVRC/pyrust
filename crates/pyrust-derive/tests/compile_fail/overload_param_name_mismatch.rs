//! Overload-set compatibility — every overload must agree on
//! parameter names at the same positions.  Otherwise the dispatcher's
//! shared kwarg-validation step couldn't unify them.

use pyrust_derive::pyrust_module;

const MODULE_NAME: &str = "test";
const FN_PREFIX: &str = "";

pyrust_module! {
    fn bad(x: PyInt) -> Result<Value> {
        unimplemented!()
    }

    fn bad(y: PyFloat) -> Result<Value> {
        unimplemented!()
    }
}

fn main() {}
