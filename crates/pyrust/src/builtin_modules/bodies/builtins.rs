// `builtins` module — included into `pub mod builtins { … }` declared by
// the `@flat builtins,` entry in `pyrust_builtin_modules!` in
// `builtin_modules/mod.rs`.
//
// `@flat` means functions register under their short name only (no
// `builtins.` prefix), so `abs` resolves to `BuiltinReg { name: "abs", … }`.
// Therefore `BuiltinFunction("abs")` from the global env (set up in
// `helpers.rs::register_builtins`) hits this dispatch via the registry
// probe in `calls.rs::call_function_expanded`.  Importable as
// `import builtins` too, which yields a `PyModule { name: "builtins", … }`
// containing every fn declared here plus declared constants.
//
// Reference: <https://docs.python.org/3/library/functions.html>

use std::rc::Rc;

use crate::ast::BinaryOp;
use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::{
    NativeIterFrame, ascii_repr, class_is_subclass_of, iter_values, lookup_class_attr,
    modpow_i64, py_mod_i64, reject_keyword_args_expanded, value_to_float, value_type_name_str,
};
use crate::value::{Value, ValueKind};
use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: chr(i) — return the string of one Unicode codepoint i.
    /// <https://docs.python.org/3/library/functions.html#chr>
    fn chr(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        let code_point = match args[0].value.kind() {
            ValueKind::Int(v) => v,
            ValueKind::Bool(b) => b as i64,
            _ => return Err(PyError::named(
                "TypeError",
                "an integer is required (got type {})".to_string(),
            )),
        };
        if !(0..=1114111).contains(&code_point) {
            return Err(PyError::named(
                "ValueError",
                format!("{FN_NAME}() arg not in range(0x110000): {code_point}"),
            ));
        }
        let ch = char::from_u32(code_point as u32).ok_or_else(|| {
            PyError::named(
                "ValueError",
                format!("{FN_NAME}() arg not in range(0x110000): {code_point}"),
            )
        })?;
        Ok(Value::string(ch.to_string()))
    }

    /// CPython: ord(c) — return the Unicode codepoint of a one-character string.
    /// <https://docs.python.org/3/library/functions.html#ord>
    fn ord(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        match args[0].value.kind() {
            ValueKind::Str(s) => {
                let mut chars = s.chars();
                let first = chars.next();
                let second = chars.next();
                match (first, second) {
                    (Some(c), None) => Ok(Value::int(c as i64)),
                    (None, _) => Err(PyError::named(
                        "TypeError",
                        format!("{FN_NAME}() expected a character, but string of length 0 found"),
                    )),
                    (Some(_), Some(_)) => Err(PyError::named(
                        "TypeError",
                        format!(
                            "{FN_NAME}() expected a character, but string of length {} found",
                            s.chars().count()
                        ),
                    )),
                }
            }
            _ => Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() expected string of length 1, but got non-string"),
            )),
        }
    }

    /// CPython: bin(x) — integer to '0b…' / '-0b…' string.
    /// <https://docs.python.org/3/library/functions.html#bin>
    fn bin(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        match args[0].value.kind() {
            ValueKind::Int(v) => {
                if v < 0 { Ok(Value::string(format!("-0b{:b}", -v))) }
                else { Ok(Value::string(format!("0b{:b}", v))) }
            }
            ValueKind::Bool(b) => Ok(Value::string(if b { "0b1".to_string() } else { "0b0".to_string() })),
            _ => Err(PyError::named(
                "TypeError",
                "'{}' object cannot be interpreted as an integer".to_string(),
            )),
        }
    }

    /// CPython: oct(x) — integer to '0o…' / '-0o…' string.
    /// <https://docs.python.org/3/library/functions.html#oct>
    fn oct(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        match args[0].value.kind() {
            ValueKind::Int(v) => {
                if v < 0 { Ok(Value::string(format!("-0o{:o}", -v))) }
                else { Ok(Value::string(format!("0o{:o}", v))) }
            }
            ValueKind::Bool(b) => Ok(Value::string(if b { "0o1".to_string() } else { "0o0".to_string() })),
            _ => Err(PyError::named(
                "TypeError",
                "'{}' object cannot be interpreted as an integer".to_string(),
            )),
        }
    }

    /// CPython: hex(x) — integer to '0x…' / '-0x…' string.
    /// <https://docs.python.org/3/library/functions.html#hex>
    fn hex(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        match args[0].value.kind() {
            ValueKind::Int(v) => {
                if v < 0 { Ok(Value::string(format!("-0x{:x}", -v))) }
                else { Ok(Value::string(format!("0x{:x}", v))) }
            }
            ValueKind::Bool(b) => Ok(Value::string(if b { "0x1".to_string() } else { "0x0".to_string() })),
            _ => Err(PyError::named(
                "TypeError",
                "'{}' object cannot be interpreted as an integer".to_string(),
            )),
        }
    }

    /// CPython: ascii(object) — ASCII-only escaped repr.
    /// <https://docs.python.org/3/library/functions.html#ascii>
    fn ascii(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        Ok(Value::string(ascii_repr(&args[0].value)))
    }

    /// CPython: id(object) — identity (CPython returns memory address).
    /// <https://docs.python.org/3/library/functions.html#id>
    fn id(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly 1 argument")));
        }
        let id_val: i64 = match args[0].value.kind() {
            ValueKind::PyInstance(rc) => Rc::as_ptr(rc) as i64,
            ValueKind::PyClass(rc) => Rc::as_ptr(rc) as i64,
            ValueKind::PyModule(rc) => Rc::as_ptr(rc) as i64,
            ValueKind::UserFunction(rc) => Rc::as_ptr(rc) as i64,
            ValueKind::Int(n) => n,
            ValueKind::Bool(b) => b as i64,
            ValueKind::None => 0,
            _ => args[0].value.value_id().unwrap_or(0),
        };
        Ok(Value::int(id_val))
    }

    /// CPython: abs(x) — absolute value.
    /// <https://docs.python.org/3/library/functions.html#abs>
    fn abs(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        let val = args[0].value.clone();
        if let ValueKind::PyInstance(inst) = val.kind() {
            let inst_rc = Rc::clone(inst);
            let class = Rc::clone(&inst_rc.borrow().class);
            if let Some(method_val) = lookup_class_attr(&class, "__abs__")
                && let ValueKind::UserFunction(f) = method_val.kind()
            {
                let func = Rc::clone(f);
                return _interp.call_user_function_expanded(
                    func,
                    &[],
                    &[Value::py_instance(inst_rc)],
                );
            }
            return Err(PyError::named(
                "TypeError",
                format!("bad operand type for abs(): '{}'", class.borrow().name),
            ));
        }
        match val.kind() {
            ValueKind::Int(v) => Ok(Value::int(v.abs())),
            ValueKind::Float(v) => Ok(Value::float(v.abs())),
            ValueKind::Bool(b) => Ok(Value::int(if b { 1 } else { 0 })),
            ValueKind::Complex(re, im) => Ok(Value::float((re * re + im * im).sqrt())),
            _ => Err(PyError::Runtime(format!("{FN_NAME}() argument must be a number"))),
        }
    }

    /// CPython: sum(iterable, /, start=0) — sum elements of an iterable.
    /// <https://docs.python.org/3/library/functions.html#sum>
    fn sum(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.is_empty() || args.len() > 2 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes 1 or 2 arguments")));
        }
        let items = iter_values(args[0].value.clone())?;
        let start = if args.len() == 2 { args[1].value.clone() } else { Value::int(0) };
        let mut acc = start;
        for item in items {
            acc = _interp.eval_binary(acc, BinaryOp::Add, item)?;
        }
        Ok(acc)
    }

    /// CPython: any(iterable) — true if any element is truthy.
    /// <https://docs.python.org/3/library/functions.html#any>
    fn any(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        let items = iter_values(args[0].value.clone())?;
        for item in items {
            if item.truthy() {
                return Ok(Value::bool_(true));
            }
        }
        Ok(Value::bool_(false))
    }

    /// CPython: all(iterable) — true if every element is truthy (or empty).
    /// <https://docs.python.org/3/library/functions.html#all>
    fn all(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        let items = iter_values(args[0].value.clone())?;
        for item in items {
            if !item.truthy() {
                return Ok(Value::bool_(false));
            }
        }
        Ok(Value::bool_(true))
    }

    /// CPython: repr(object) — printable representation string.
    /// <https://docs.python.org/3/library/functions.html#repr>
    fn repr(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        let obj = args[0].value.clone();
        if let ValueKind::PyInstance(instance) = obj.kind() {
            let instance_rc = Rc::clone(instance);
            let class = Rc::clone(&instance_rc.borrow().class);
            if let Some(method_val) = lookup_class_attr(&class, "__repr__")
                && let ValueKind::UserFunction(f) = method_val.kind()
            {
                let func = Rc::clone(f);
                let result = _interp.call_user_function_expanded(
                    func,
                    &[],
                    &[Value::py_instance(instance_rc)],
                )?;
                return match result.kind() {
                    ValueKind::Str(_) => Ok(result),
                    _ => Err(PyError::named(
                        "TypeError",
                        "__repr__ returned non-string".to_string(),
                    )),
                };
            }
        }
        Ok(Value::string(obj.repr()))
    }

    /// CPython: hash(object) — hash value if hashable.
    /// <https://docs.python.org/3/library/functions.html#hash>
    fn hash(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        let hash_val = match args[0].value.kind() {
            ValueKind::Int(v) => v,
            ValueKind::Bool(b) => b as i64,
            ValueKind::Float(v) => {
                if v.fract() == 0.0 && v.is_finite() { v as i64 }
                else { v.to_bits() as i64 }
            }
            ValueKind::Str(s) => {
                let mut h: u64 = 14695981039346656037u64;
                for b in s.bytes() {
                    h ^= b as u64;
                    h = h.wrapping_mul(1099511628211u64);
                }
                h as i64
            }
            ValueKind::None => 0,
            ValueKind::Tuple(items) => {
                let mut h: i64 = 3527539;
                for item in items {
                    let item_hash = match item.kind() {
                        ValueKind::Int(v) => v,
                        ValueKind::Bool(b) => b as i64,
                        ValueKind::Float(fv) => {
                            if fv.fract() == 0.0 && fv.is_finite() { fv as i64 }
                            else { fv.to_bits() as i64 }
                        }
                        ValueKind::Str(s) => {
                            let mut sh: u64 = 14695981039346656037u64;
                            for byte in s.bytes() {
                                sh ^= byte as u64;
                                sh = sh.wrapping_mul(1099511628211u64);
                            }
                            sh as i64
                        }
                        ValueKind::None => 0,
                        _ => return Err(PyError::named(
                            "TypeError",
                            "unhashable type in tuple".to_string(),
                        )),
                    };
                    h = h.wrapping_mul(1000003).wrapping_add(item_hash);
                }
                h
            }
            ValueKind::List(_) => return Err(PyError::named(
                "TypeError",
                "unhashable type: 'list'".to_string(),
            )),
            ValueKind::Dict(_) => return Err(PyError::named(
                "TypeError",
                "unhashable type: 'dict'".to_string(),
            )),
            ValueKind::Set(_) => return Err(PyError::named(
                "TypeError",
                "unhashable type: 'set'".to_string(),
            )),
            _ => return Err(PyError::named(
                "TypeError",
                "unhashable type".to_string(),
            )),
        };
        Ok(Value::int(hash_val))
    }

    /// CPython: divmod(a, b) — `(a // b, a % b)`.
    /// <https://docs.python.org/3/library/functions.html#divmod>
    fn divmod(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly 2 arguments")));
        }
        match (args[0].value.kind(), args[1].value.kind()) {
            (ValueKind::Int(a), ValueKind::Int(b)) => {
                if b == 0 {
                    return Err(PyError::named(
                        "ZeroDivisionError",
                        "integer division or modulo by zero".to_string(),
                    ));
                }
                let modulo = py_mod_i64(a, b);
                let quotient = (a - modulo) / b;
                Ok(Value::tuple(vec![Value::int(quotient), Value::int(modulo)]))
            }
            (ValueKind::Bool(a), ValueKind::Bool(b)) => {
                let a = a as i64;
                let b = b as i64;
                if b == 0 {
                    return Err(PyError::named(
                        "ZeroDivisionError",
                        "integer division or modulo by zero".to_string(),
                    ));
                }
                let modulo = py_mod_i64(a, b);
                let quotient = (a - modulo) / b;
                Ok(Value::tuple(vec![Value::int(quotient), Value::int(modulo)]))
            }
            _ => {
                let a = value_to_float(&args[0].value, FN_NAME)?;
                let b = value_to_float(&args[1].value, FN_NAME)?;
                if b == 0.0 {
                    return Err(PyError::named(
                        "ZeroDivisionError",
                        "float divmod()".to_string(),
                    ));
                }
                let quotient = (a / b).floor();
                let modulo = a - b * quotient;
                Ok(Value::tuple(vec![Value::float(quotient), Value::float(modulo)]))
            }
        }
    }

    /// CPython: pow(base, exp[, mod]) — exponentiation, optionally modular.
    /// <https://docs.python.org/3/library/functions.html#pow>
    fn pow(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() < 2 || args.len() > 3 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes 2 or 3 arguments")));
        }
        if args.len() == 3 {
            let base = match args[0].value.kind() {
                ValueKind::Int(v) => v,
                ValueKind::Bool(b) => b as i64,
                _ => return Err(PyError::named(
                    "TypeError",
                    "pow() 3-argument form requires integers".to_string(),
                )),
            };
            let exp = match args[1].value.kind() {
                ValueKind::Int(v) => v,
                ValueKind::Bool(b) => b as i64,
                _ => return Err(PyError::named(
                    "TypeError",
                    "pow() 3-argument form requires integers".to_string(),
                )),
            };
            let modulus = match args[2].value.kind() {
                ValueKind::Int(v) => v,
                ValueKind::Bool(b) => b as i64,
                _ => return Err(PyError::named(
                    "TypeError",
                    "pow() 3-argument form requires integers".to_string(),
                )),
            };
            if modulus == 0 {
                return Err(PyError::named(
                    "ValueError",
                    "pow() 3rd argument cannot be 0".to_string(),
                ));
            }
            if exp < 0 {
                return Err(PyError::named(
                    "ValueError",
                    "pow() 2nd argument cannot be negative when 3rd argument specified".to_string(),
                ));
            }
            let result = modpow_i64(base, exp as u64, modulus);
            Ok(Value::int(result))
        } else {
            match (args[0].value.kind(), args[1].value.kind()) {
                (ValueKind::Int(a), ValueKind::Int(b)) if b >= 0 => {
                    Ok(Value::int(a.wrapping_pow(b as u32)))
                }
                (ValueKind::Bool(a), ValueKind::Int(b)) if b >= 0 => {
                    Ok(Value::int((a as i64).wrapping_pow(b as u32)))
                }
                _ => {
                    let a = value_to_float(&args[0].value, FN_NAME)?;
                    let b = value_to_float(&args[1].value, FN_NAME)?;
                    Ok(Value::float(a.powf(b)))
                }
            }
        }
    }

    /// CPython: enumerate(iterable, start=0) — enumerate iterator.
    /// <https://docs.python.org/3/library/functions.html#enumerate>
    fn enumerate(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.is_empty() || args.len() > 2 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes 1 or 2 arguments")));
        }
        let start = if args.len() == 2 {
            match args[1].value.kind() {
                ValueKind::Int(n) => n,
                _ => return Err(PyError::Runtime(format!(
                    "{FN_NAME}() start argument must be an integer",
                ))),
            }
        } else {
            0i64
        };
        // Pass the source Value directly — `iter_helpers` materialises
        // lazily on first iter_next so side effects of e.g. `open()`
        // happen at iteration start, not at construction.
        Ok(pyrust_builtins::iter_helpers::enumerate(
            args[0].value.clone(),
            start,
        ))
    }

    /// CPython: zip(*iterables) — parallel iterator.
    /// <https://docs.python.org/3/library/functions.html#zip>
    fn zip(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let sources: Vec<Value> = args.iter().map(|a| a.value.clone()).collect();
        Ok(pyrust_builtins::iter_helpers::zip(sources))
    }

    /// CPython: reversed(seq) — reverse iterator.
    /// <https://docs.python.org/3/library/functions.html#reversed>
    fn reversed(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        Ok(pyrust_builtins::iter_helpers::reversed(args[0].value.clone()))
    }

    /// CPython: map(func, iterable) — apply func to each element.
    /// <https://docs.python.org/3/library/functions.html#map>
    fn map(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly 2 arguments")));
        }
        let func = args[0].value.clone();
        let items = iter_values(args[1].value.clone())?;
        let mut result = Vec::with_capacity(items.len());
        for item in items {
            let mapped = _interp.call_function_expanded(
                func.clone(),
                &[ExpandedCallArg { name: None, value: item }],
            )?;
            result.push(mapped);
        }
        Ok(Value::list(result))
    }

    /// CPython: filter(func, iterable) — keep elements where func is truthy.
    /// <https://docs.python.org/3/library/functions.html#filter>
    fn filter(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly 2 arguments")));
        }
        let func = args[0].value.clone();
        let items = iter_values(args[1].value.clone())?;
        let use_identity = func.is_none();
        let mut result = Vec::new();
        for item in items {
            let keep = if use_identity {
                item.truthy()
            } else {
                let test = _interp.call_function_expanded(
                    func.clone(),
                    &[ExpandedCallArg { name: None, value: item.clone() }],
                )?;
                test.truthy()
            };
            if keep {
                result.push(item);
            }
        }
        Ok(Value::list(result))
    }

    /// CPython: iter(obj) — return an iterator over obj.
    /// <https://docs.python.org/3/library/functions.html#iter>
    fn iter(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        let val = args[0].value.clone();
        match val.kind() {
            // Generators are their own iterators.
            ValueKind::Generator(_) => Ok(val),
            // User-defined objects: call __iter__().
            ValueKind::PyInstance(inst) => {
                let inst_rc = Rc::clone(inst);
                let class = Rc::clone(&inst_rc.borrow().class);
                if let Some(method_val) = lookup_class_attr(&class, "__iter__")
                    && let ValueKind::UserFunction(f) = method_val.kind()
                {
                    let func = Rc::clone(f);
                    _interp.call_user_function_expanded(
                        func,
                        &[],
                        &[Value::py_instance(inst_rc)],
                    )
                } else if lookup_class_attr(&class, "__next__").is_some() {
                    // Already an iterator (has __next__ but no separate __iter__).
                    Ok(val)
                } else {
                    Err(PyError::named(
                        "TypeError",
                        format!("'{}' object is not iterable", class.borrow().name),
                    ))
                }
            }
            // Built-in iterables: materialise into a NativeIterFrame so that
            // next() works on the returned value.
            _ => {
                let items = iter_values(val.clone()).map_err(|_| {
                    PyError::named(
                        "TypeError",
                        format!("'{}' object is not iterable", value_type_name_str(&val)),
                    )
                })?;
                Ok(Value::generator(Box::new(NativeIterFrame { items, pos: 0 })))
            }
        }
    }

    /// CPython: next(iterator[, default]) — fetch the next element.
    /// <https://docs.python.org/3/library/functions.html#next>
    fn next(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.is_empty() || args.len() > 2 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes 1 or 2 arguments")));
        }
        let gen_val = args[0].value.clone();
        let default_val = if args.len() == 2 {
            Some(args[1].value.clone())
        } else {
            None
        };
        _interp.call_next(gen_val, default_val)
    }

    /// CPython: issubclass(cls, classinfo) — true if `cls` is a subclass.
    /// <https://docs.python.org/3/library/functions.html#issubclass>
    fn issubclass(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly 2 arguments")));
        }
        let cls = match args[0].value.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            _ => return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() arg 1 must be a class"),
            )),
        };
        let result = match args[1].value.kind() {
            ValueKind::PyClass(expected) => class_is_subclass_of(&cls, expected),
            ValueKind::Tuple(items) => {
                let mut found = false;
                for item in items {
                    if let ValueKind::PyClass(expected) = item.kind()
                        && class_is_subclass_of(&cls, expected)
                    {
                        found = true;
                        break;
                    }
                }
                found
            }
            _ => return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() arg 2 must be a class or tuple of classes"),
            )),
        };
        Ok(Value::bool_(result))
    }

    /// CPython: delattr(obj, name) — delete an attribute.
    /// <https://docs.python.org/3/library/functions.html#delattr>
    fn delattr(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly 2 arguments")));
        }
        let name = match args[1].value.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}(): attribute name must be a string"),
            )),
        };
        match args[0].value.kind() {
            ValueKind::PyInstance(instance) => {
                let instance = Rc::clone(instance);
                if instance.borrow_mut().attrs.remove(&name).is_none() {
                    let class_name = instance.borrow().class.borrow().name.clone();
                    return Err(PyError::named(
                        "AttributeError",
                        format!("'{class_name}' object has no attribute '{name}'"),
                    ));
                }
                Ok(Value::none())
            }
            ValueKind::PyClass(class) => {
                let class = Rc::clone(class);
                if class.borrow_mut().attrs.remove(&name).is_none() {
                    let class_name = class.borrow().name.clone();
                    return Err(PyError::named(
                        "AttributeError",
                        format!("type object '{class_name}' has no attribute '{name}'"),
                    ));
                }
                Ok(Value::none())
            }
            _ => Err(PyError::named(
                "AttributeError",
                format!("{FN_NAME}() object has no writable attributes"),
            )),
        }
    }

    /// CPython: callable(object) — true if the object is callable.
    /// <https://docs.python.org/3/library/functions.html#callable>
    fn callable(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        let is_callable = match args[0].value.kind() {
            ValueKind::UserFunction(_)
            | ValueKind::BuiltinFunction(_)
            | ValueKind::BoundMethod { .. }
            | ValueKind::ClassBoundMethod { .. }
            | ValueKind::PyClass(_) => true,
            // Only accessor partials (intermediate results of
            // prop.setter / prop.getter / prop.deleter) are callable —
            // a plain property descriptor isn't.
            ValueKind::BuiltinObject { .. } => {
                pyrust_builtins::property::property_partial_slot(&args[0].value)
                    .is_some_and(|slot| slot.is_some())
            }
            ValueKind::PyInstance(inst) => {
                let class = Rc::clone(&inst.borrow().class);
                lookup_class_attr(&class, "__call__").is_some()
            }
            _ => false,
        };
        Ok(Value::bool_(is_callable))
    }
}
