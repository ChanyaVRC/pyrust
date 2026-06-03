//! `iter_self;` injects the canonical return-self `__iter__`.  Declaring
//! an explicit `fn __iter__` alongside it would silently double-register
//! the method; the macro pre-seeds `__iter__` into the duplicate-name set
//! so the explicit declaration is rejected at parse time with the same
//! clear span the generic duplicate-method guard uses (#1895).

use pyrust_derive::pyrust_module;

const MODULE_NAME: &str = "test";
const FN_PREFIX: &str = "";

pyrust_module! {
    class bad {
        iter_self;
        fn __iter__(args) -> Result<Value> {
            Ok(args[0].value.clone())
        }
    }
}

fn main() {}
