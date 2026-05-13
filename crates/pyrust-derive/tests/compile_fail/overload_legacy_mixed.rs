//! Overload-set compatibility — every overload must use the typed
//! dialect.  Mixing the legacy `(args)` form into an overload set
//! defeats the type-based dispatch (the legacy form has no per-arg
//! type to predicate on); the macro rejects it at parse time.

use pyrust_derive::pyrust_module;

const MODULE_NAME: &str = "test";
const FN_PREFIX: &str = "";

pyrust_module! {
    fn bad(x: PyInt) -> Result<Value> {
        unimplemented!()
    }

    fn bad(args) -> Result<Value> {
        unimplemented!()
    }
}

fn main() {}
