//! `#[arity_style(...)]` drives the typed-dialect prelude; on a legacy
//! `(args)` fn it would be silently ignored, so it's rejected up front
//! (#2331).

use pyrust_derive::pyrust_module;

const MODULE_NAME: &str = "test";
const FN_PREFIX: &str = "";

pyrust_module! {
    #[arity_style(expected_got)]
    fn bad(args) -> Result<Value> {
        unimplemented!()
    }
}

fn main() {}
