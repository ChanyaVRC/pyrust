//! Overload sets disallow `#[default(...)]` on any parameter (v1
//! limitation).  Defaults are applied before knowing which overload
//! matches, so the default's type would have to be compatible with
//! every overload — the same chicken-and-egg the overload mechanism
//! is meant to break.  Use a single signature with the catch-all
//! `PyValue` if defaults are needed.

use pyrust_derive::pyrust_module;

const MODULE_NAME: &str = "test";
const FN_PREFIX: &str = "";

pyrust_module! {
    fn bad(#[default(0)] x: PyInt) -> Result<Value> {
        unimplemented!()
    }

    fn bad(x: PyFloat) -> Result<Value> {
        unimplemented!()
    }
}

fn main() {}
