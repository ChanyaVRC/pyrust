// `secrets` module — 100% native Rust implementation of CPython 3.12.3's
// `Lib/secrets.py` (issues #3146, #3495); there is no Python-source injection.
//
// Like the Python source, the module is a thin layer over three other
// modules and resolves everything at call time through its own namespace:
//
//  * `_sysrand = SystemRandom()` is created per interpreter when the module
//    loads (a fresh `_sysrand` per import generation, as CPython's module
//    body does) and `randbits` / `choice` are its bound methods.
//  * `randbelow`, `token_bytes`, `token_hex` and `token_urlsafe` read
//    `_sysrand`, `DEFAULT_ENTROPY`, `token_bytes` and `base64` from the
//    module's attributes on every call, so rebinding or monkeypatching any of
//    them (`secrets.DEFAULT_ENTROPY = 16`, `secrets._sysrand = ...`) behaves
//    as it does for the pure-Python module's globals.
//  * `SystemRandom` is `random.SystemRandom` and `compare_digest` is
//    `hmac.compare_digest` (same objects).
//
// All argument validation (`nbytes < 0`, non-`int` bounds, ...) is done by the
// delegated calls, so the exception types and messages are CPython's.
//
// Deliberate difference: native functions carry no `__doc__`.
//
// Reference: <https://docs.python.org/3.12/library/secrets.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::ast::BinaryOp;
use crate::builtin_modules::json::args::{self, Sig};
use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyModule, Value};

const MODULE_DOC: &str = "Generate cryptographically strong pseudo-random numbers suitable for\nmanaging secrets such as account authentication, tokens, and similar.\n\nSee PEP 506 for more information.\nhttps://peps.python.org/pep-0506/\n\n";

/// `__all__`, in CPython's order.
const ALL: [&str; 8] = [
    "choice",
    "randbelow",
    "randbits",
    "SystemRandom",
    "token_bytes",
    "token_hex",
    "token_urlsafe",
    "compare_digest",
];

/// CPython's `vars(secrets)` order for the names this module binds.
const MODULE_ORDER: &[&str] = &[
    "__all__",
    "base64",
    "compare_digest",
    "SystemRandom",
    "_sysrand",
    "randbits",
    "choice",
    "randbelow",
    "DEFAULT_ENTROPY",
    "token_bytes",
    "token_hex",
    "token_urlsafe",
];

// `def`s of `Lib/secrets.py`: arity errors use the interpreter's wording.
const SIG_RANDBELOW: Sig = Sig {
    qualname: "randbelow",
    params: &["exclusive_upper_bound"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_TOKEN: Sig = Sig {
    qualname: "token",
    params: &["nbytes"],
    defaults: 1,
    kwonly: &[],
    var_kw: false,
};

const fn token_sig(qualname: &'static str) -> Sig {
    Sig {
        qualname,
        ..SIG_TOKEN
    }
}

const SIG_TOKEN_BYTES: Sig = token_sig("token_bytes");
const SIG_TOKEN_HEX: Sig = token_sig("token_hex");
const SIG_TOKEN_URLSAFE: Sig = token_sig("token_urlsafe");

fn all_list() -> Value {
    Value::list(ALL.iter().map(|name| Value::string(*name)).collect())
}

fn positional(pos: &[Value]) -> Vec<ExpandedCallArg> {
    pos.iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect()
}

/// `callee(*pos)`.
fn call(interp: &mut Interpreter, callee: Value, pos: &[Value]) -> Result<Value> {
    interp.call_function_expanded(callee, &positional(pos))
}

/// `target.name(*pos)`.
fn call_method(
    interp: &mut Interpreter,
    target: &Value,
    name: &str,
    pos: &[Value],
) -> Result<Value> {
    let callee = interp.get_attr(target, name)?;
    call(interp, callee, pos)
}

/// A global of `secrets.py`, read from the module's namespace at call time.
fn global(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let module = interp.load_module("secrets")?;
    interp.get_attr(&module, name)
}

/// `token_bytes(nbytes)` as the module's current `token_bytes` global.
fn token_bytes_global(interp: &mut Interpreter, nbytes: Value) -> Result<Value> {
    let token_bytes = global(interp, "token_bytes")?;
    call(interp, token_bytes, &[nbytes])
}

/// `_sysrand = SystemRandom()`, `randbits`/`choice` as its bound methods, and
/// the imported names; ordered like CPython's module namespace.
pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let base64 = interp.load_module("base64")?;
    let hmac = interp.load_module("hmac")?;
    let random = interp.load_module("random")?;
    let compare_digest = interp.get_attr(&hmac, "compare_digest")?;
    let system_random = interp.get_attr(&random, "SystemRandom")?;
    let sysrand = call(interp, system_random.clone(), &[])?;
    let randbits = interp.get_attr(&sysrand, "getrandbits")?;
    let choice = interp.get_attr(&sysrand, "choice")?;

    let mut module = module.borrow_mut();
    module.insert_attr("base64".to_string(), base64);
    module.insert_attr("compare_digest".to_string(), compare_digest);
    module.insert_attr("SystemRandom".to_string(), system_random);
    module.insert_attr("_sysrand".to_string(), sysrand);
    module.insert_attr("randbits".to_string(), randbits);
    module.insert_attr("choice".to_string(), choice);
    for name in MODULE_ORDER {
        if let Some(value) = module.attrs.shift_remove(*name) {
            module.attrs.insert((*name).to_string(), value);
        }
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(MODULE_DOC),
        "__all__" => all_list(),
        "DEFAULT_ENTROPY" => Value::int(32),
    }

    /// Return a random int in the range [0, n).
    fn randbelow(args) -> Result<Value> {
        let mut b = args::bind(&SIG_RANDBELOW, args)?;
        let bound = b.take_or_none(0);
        let le = _interp.eval_binary(bound.clone(), BinaryOp::Le, Value::int(0))?;
        if _interp.truthy_value(&le)? {
            return Err(PyError::named(
                "ValueError",
                "Upper bound must be positive.".to_string(),
            ));
        }
        let sysrand = global(_interp, "_sysrand")?;
        call_method(_interp, &sysrand, "_randbelow", &[bound])
    }

    /// Return a random byte string containing *nbytes* bytes.
    ///
    /// If *nbytes* is ``None`` or not supplied, a reasonable
    /// default is used.
    fn token_bytes(args) -> Result<Value> {
        let mut b = args::bind(&SIG_TOKEN_BYTES, args)?;
        let mut nbytes = b.take_or_none(0);
        if nbytes.is_none() {
            nbytes = global(_interp, "DEFAULT_ENTROPY")?;
        }
        let sysrand = global(_interp, "_sysrand")?;
        call_method(_interp, &sysrand, "randbytes", &[nbytes])
    }

    /// Return a random text string, in hexadecimal.
    ///
    /// The string has *nbytes* random bytes, each byte converted to two
    /// hex digits.  If *nbytes* is ``None`` or not supplied, a reasonable
    /// default is used.
    fn token_hex(args) -> Result<Value> {
        let mut b = args::bind(&SIG_TOKEN_HEX, args)?;
        let nbytes = b.take_or_none(0);
        let tok = token_bytes_global(_interp, nbytes)?;
        call_method(_interp, &tok, "hex", &[])
    }

    /// Return a random URL-safe text string, in Base64 encoding.
    ///
    /// The string has *nbytes* random bytes.  If *nbytes* is ``None``
    /// or not supplied, a reasonable default is used.
    fn token_urlsafe(args) -> Result<Value> {
        let mut b = args::bind(&SIG_TOKEN_URLSAFE, args)?;
        let nbytes = b.take_or_none(0);
        let tok = token_bytes_global(_interp, nbytes)?;
        let base64 = global(_interp, "base64")?;
        let encoded = call_method(_interp, &base64, "urlsafe_b64encode", &[tok])?;
        let stripped = call_method(_interp, &encoded, "rstrip", &[Value::bytes(b"=".to_vec())])?;
        call_method(_interp, &stripped, "decode", &[Value::string("ascii")])
    }
}
