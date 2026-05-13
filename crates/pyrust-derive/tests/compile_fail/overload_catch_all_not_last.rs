//! A `PyValue`-only "catch-all" overload must be declared **last** in
//! its overload set.  `PyValue::matches` is unconditional, so any
//! overload after it would be silently unreachable.  The macro catches
//! this at parse time and emits a clear diagnostic on the offending
//! catch-all overload (per design review on #395, concern 1).

use pyrust_derive::pyrust_module;

const MODULE_NAME: &str = "test";
const FN_PREFIX: &str = "";

pyrust_module! {
    fn bad(x: PyInt) -> Result<Value> {
        unimplemented!()
    }

    fn bad(x: PyValue) -> Result<Value> {
        unimplemented!()
    }

    // Unreachable — the PyValue overload above always matches first.
    fn bad(x: PyFloat) -> Result<Value> {
        unimplemented!()
    }
}

fn main() {}
