//! Catch-all detection also covers `Option<PyValue>` — both `None`
//! and `Some(any)` match unconditionally, so an `Option<PyValue>`-only
//! overload silently shadows every overload after it just like a
//! plain `PyValue`.  Verifies the extension landed alongside the
//! original catch-all check (per the review on PR #397).

use pyrust_derive::pyrust_module;

const MODULE_NAME: &str = "test";
const FN_PREFIX: &str = "";

pyrust_module! {
    fn bad(x: PyInt) -> Result<Value> {
        unimplemented!()
    }

    fn bad(x: Option<PyValue>) -> Result<Value> {
        unimplemented!()
    }

    // Unreachable — the Option<PyValue> overload above always matches.
    fn bad(x: PyFloat) -> Result<Value> {
        unimplemented!()
    }
}

fn main() {}
