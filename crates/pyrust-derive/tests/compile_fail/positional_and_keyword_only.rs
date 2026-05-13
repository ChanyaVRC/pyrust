//! A parameter cannot simultaneously be positional-only and
//! keyword-only.  The conflict error is attached to the second
//! attribute via `syn::Error::new_spanned` so the user sees the
//! diagnostic on the offending token, not on the parameter ident.

use pyrust_derive::pyrust_module;

const MODULE_NAME: &str = "test";
const FN_PREFIX: &str = "";

pyrust_module! {
    fn bad(#[positional_only] #[keyword_only] x: PyInt) -> Result<Value> {
        unimplemented!()
    }
}

fn main() {}
