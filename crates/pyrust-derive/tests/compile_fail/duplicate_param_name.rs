//! Two parameters with the same name would silently shadow in the
//! generated `let` bindings (Rust doesn't warn on macro-emitted
//! shadows).  The macro now rejects the duplicate at parse time so the
//! mistake surfaces with a clear span on the second `x`.

use pyrust_derive::pyrust_module;

const MODULE_NAME: &str = "test";
const FN_PREFIX: &str = "";

pyrust_module! {
    fn bad(x: PyInt, x: PyStr) -> Result<Value> {
        unimplemented!()
    }
}

fn main() {}
