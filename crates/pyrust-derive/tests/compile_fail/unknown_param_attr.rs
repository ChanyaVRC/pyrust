//! Unrecognised parameter attributes (anything that isn't `#[default]`,
//! `#[positional_only]`, `#[keyword_only]`, or one of the silently-passed
//! `#[doc]` / `#[cfg]` / `#[allow]`) get rejected at parse time.  This
//! pins the diagnostic for typos like `#[positionalonly]`.

use pyrust_derive::pyrust_module;

const MODULE_NAME: &str = "test";
const FN_PREFIX: &str = "";

pyrust_module! {
    fn bad(#[positionalonly] x: PyInt) -> Result<Value> {
        unimplemented!()
    }
}

fn main() {}
