use pyrust_core::{
    PyBigInt, PyBigIntSign, PyDict, PyError, PyKey, PyToPrimitive, Result, Value, ValueKind,
};

/// Canonical list of method names dispatched by `call`.
/// Single source of truth for `has_method` and the drift-guard test.
/// Note: `real`, `imag`, `numerator`, `denominator` are read-only properties
/// (not methods) and are intercepted in `get_attr` before `has_method` is
/// reached.  `conjugate` is a true zero-arg method and lives here.
pub const METHODS: &[&str] = &[
    "bit_length",
    "bit_count",
    "conjugate",
    "is_integer",
    "to_bytes",
    "from_bytes",
    "as_integer_ratio",
];

/// Returns `true` if `method` is the name of a built-in `int` method.
/// Used by `hasattr` / `getattr` to validate attribute names without
/// invoking the method.
pub fn has_method(method: &str) -> bool {
    METHODS.contains(&method)
}

pub fn call(method: &str, receiver: &Value, args: &[Value], kw: &PyDict) -> Result<Value> {
    match method {
        "conjugate" => {
            if !kw.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    "int.conjugate() takes no keyword arguments".to_string(),
                ));
            }
            if !args.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    format!("int.conjugate() takes no arguments ({} given)", args.len()),
                ));
            }
            // int.conjugate() returns self (int has no imaginary part).
            // CPython returns an int, not a bool, even for True/False.
            match receiver.kind() {
                ValueKind::Int(_) | ValueKind::BigInt(_) => Ok(receiver.clone()),
                // bool.conjugate() returns the int equivalent (True -> 1, False -> 0).
                ValueKind::Bool(b) => Ok(Value::int(b as i64)),
                _ => Err(PyError::named(
                    "TypeError",
                    format!(
                        "descriptor 'conjugate' for 'int' objects doesn't apply to a '{}' object",
                        pyrust_core::builtin_type_name(receiver)
                    ),
                )),
            }
        }
        "bit_length" => {
            if !kw.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    "int.bit_length() takes no keyword arguments".to_string(),
                ));
            }
            if !args.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    format!("int.bit_length() takes no arguments ({} given)", args.len()),
                ));
            }
            let result = match receiver.kind() {
                // CPython: bit_length uses abs(n), so -1 and 1 both give 1.
                ValueKind::Int(n) => {
                    let abs_n = n.unsigned_abs();
                    if abs_n == 0 {
                        0i64
                    } else {
                        (64 - abs_n.leading_zeros()) as i64
                    }
                }
                // bool is a subclass of int in CPython; True==1, False==0.
                ValueKind::Bool(b) => b as i64,
                ValueKind::BigInt(b) => {
                    // num_bigint::BigInt::bits() returns the number of bits in
                    // the magnitude (equivalent to CPython's abs(n).bit_length()).
                    b.bits() as i64
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "descriptor 'bit_length' for 'int' objects doesn't apply to a '{}' object",
                            pyrust_core::builtin_type_name(receiver)
                        ),
                    ));
                }
            };
            Ok(Value::int(result))
        }
        "bit_count" => {
            if !kw.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    "int.bit_count() takes no keyword arguments".to_string(),
                ));
            }
            if !args.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    format!("int.bit_count() takes no arguments ({} given)", args.len()),
                ));
            }
            let result = match receiver.kind() {
                // CPython: bit_count counts 1-bits in abs(n).
                // (-1).bit_count() == 1 because abs(-1) == 1 == 0b1.
                ValueKind::Int(n) => n.unsigned_abs().count_ones() as i64,
                // bool is a subclass of int; True.bit_count() == 1, False.bit_count() == 0.
                ValueKind::Bool(b) => b as i64,
                ValueKind::BigInt(b) => {
                    // magnitude() gives the absolute value as BigUint,
                    // which has count_ones() via num_bigint.
                    b.magnitude().count_ones() as i64
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "descriptor 'bit_count' for 'int' objects doesn't apply to a '{}' object",
                            pyrust_core::builtin_type_name(receiver)
                        ),
                    ));
                }
            };
            Ok(Value::int(result))
        }
        "is_integer" => {
            if !kw.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    "int.is_integer() takes no keyword arguments".to_string(),
                ));
            }
            if !args.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    format!("int.is_integer() takes no arguments ({} given)", args.len()),
                ));
            }
            // int (and bool, which subclasses int) is always an integer.
            // This method exists for duck-typing parity with float.is_integer().
            Ok(Value::bool_(true))
        }
        "to_bytes" => int_to_bytes(receiver, args, kw),
        "from_bytes" => {
            // from_bytes is a classmethod; the receiver here is either the int
            // class (PyClass) or an int instance (when called as (5).from_bytes(...)).
            // In both cases the actual conversion ignores the receiver and uses
            // the first positional arg as the bytes input.
            int_from_bytes(args, kw)
        }
        "as_integer_ratio" => {
            if !kw.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    "as_integer_ratio() takes no keyword arguments".to_string(),
                ));
            }
            if !args.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "as_integer_ratio() takes no arguments ({} given)",
                        args.len()
                    ),
                ));
            }
            // Return (self, 1) as a tuple.
            let self_val = match receiver.kind() {
                ValueKind::Bool(b) => Value::int(b as i64),
                ValueKind::Int(_) | ValueKind::BigInt(_) => receiver.clone(),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "descriptor 'as_integer_ratio' for 'int' objects doesn't apply to a '{}' object",
                            pyrust_core::builtin_type_name(receiver)
                        ),
                    ));
                }
            };
            Ok(Value::tuple(vec![self_val, Value::int(1)]))
        }
        _ => Err(PyError::named(
            "AttributeError",
            format!("'int' object has no attribute '{method}'"),
        )),
    }
}

/// Implements `int.to_bytes(length=1, byteorder='big', *, signed=False)`.
fn int_to_bytes(receiver: &Value, args: &[Value], kw: &PyDict) -> Result<Value> {
    if args.len() > 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "to_bytes() takes at most 2 positional arguments ({} given)",
                args.len()
            ),
        ));
    }

    let mut length_val: Option<Value> = args.first().cloned();
    let mut byteorder_val: Option<Value> = args.get(1).cloned();
    let mut signed_val: Option<Value> = None;

    for (k, v) in kw {
        let name = match k {
            PyKey::Str(s) => s.as_str().unwrap_or("").to_owned(),
            _ => continue,
        };
        match name.as_str() {
            "length" => {
                if length_val.is_some() {
                    return Err(PyError::named(
                        "TypeError",
                        "argument for to_bytes() given by name ('length') and position (1)"
                            .to_string(),
                    ));
                }
                length_val = Some(v.clone());
            }
            "byteorder" => {
                if byteorder_val.is_some() {
                    return Err(PyError::named(
                        "TypeError",
                        "argument for to_bytes() given by name ('byteorder') and position (2)"
                            .to_string(),
                    ));
                }
                byteorder_val = Some(v.clone());
            }
            "signed" => {
                signed_val = Some(v.clone());
            }
            other => {
                return Err(PyError::named(
                    "TypeError",
                    format!("to_bytes() got an unexpected keyword argument '{other}'"),
                ));
            }
        }
    }

    // Defaults: length=1, byteorder='big', signed=False
    let length: usize = match length_val {
        None => 1,
        Some(v) => match v.kind() {
            ValueKind::Int(n) => {
                if n < 0 {
                    return Err(PyError::named(
                        "ValueError",
                        "length argument must be non-negative".to_string(),
                    ));
                }
                n as usize
            }
            ValueKind::BigInt(b) => {
                if b.sign() == PyBigIntSign::Minus {
                    return Err(PyError::named(
                        "ValueError",
                        "length argument must be non-negative".to_string(),
                    ));
                }
                // A positive BigInt length is astronomically large — refuse.
                return Err(PyError::named(
                    "OverflowError",
                    "int too big to convert".to_string(),
                ));
            }
            ValueKind::Bool(b) => b as usize,
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "'{}' object cannot be interpreted as an integer",
                        pyrust_core::builtin_type_name(&v)
                    ),
                ));
            }
        },
    };

    let big_endian: bool = match byteorder_val {
        None => true,
        Some(v) => match v.kind() {
            ValueKind::Str(s) => {
                let s = s.to_string();
                if s == "big" {
                    true
                } else if s == "little" {
                    false
                } else {
                    return Err(PyError::named(
                        "ValueError",
                        "byteorder must be either 'little' or 'big'".to_string(),
                    ));
                }
            }
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "to_bytes() argument 'byteorder' must be str, not {}",
                        pyrust_core::builtin_type_name(&v)
                    ),
                ));
            }
        },
    };

    let signed: bool = match signed_val {
        None => false,
        Some(v) => match v.kind() {
            ValueKind::Bool(b) => b,
            ValueKind::Int(n) => n != 0,
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "to_bytes() argument 'signed' must be a bool, not '{}'",
                        pyrust_core::builtin_type_name(&v)
                    ),
                ));
            }
        },
    };

    match receiver.kind() {
        ValueKind::Bool(b) => int_i64_to_bytes(b as i64, length, big_endian, signed),
        ValueKind::Int(n) => int_i64_to_bytes(n, length, big_endian, signed),
        ValueKind::BigInt(b) => bigint_to_bytes(&b.clone(), length, big_endian, signed),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "descriptor 'to_bytes' for 'int' objects doesn't apply to a '{}' object",
                pyrust_core::builtin_type_name(receiver)
            ),
        )),
    }
}

fn int_i64_to_bytes(n: i64, length: usize, big_endian: bool, signed: bool) -> Result<Value> {
    if !signed && n < 0 {
        return Err(PyError::named(
            "OverflowError",
            "can't convert negative int to unsigned".to_string(),
        ));
    }

    let mut buf = vec![0u8; length];

    if n == 0 {
        return Ok(Value::bytes(buf));
    }

    if signed {
        if length == 0 {
            return Err(PyError::named(
                "OverflowError",
                "int too big to convert".to_string(),
            ));
        }
        // Two's complement: reinterpret n as u64 for LE byte extraction.
        let n_bytes: [u8; 8] = (n as u64).to_le_bytes();
        // Check n fits in `length` signed bytes.
        if length < 8 {
            let max_pos = (1i64 << (8 * length - 1)) - 1;
            let min_neg = -(1i64 << (8 * length - 1));
            if n > max_pos || n < min_neg {
                return Err(PyError::named(
                    "OverflowError",
                    "int too big to convert".to_string(),
                ));
            }
        }
        // length >= 8: i64 always fits in 8 signed bytes.
        let sign_fill = if n < 0 { 0xffu8 } else { 0u8 };
        for (i, dst) in buf.iter_mut().enumerate() {
            *dst = if i < 8 { n_bytes[i] } else { sign_fill };
        }
        if big_endian {
            buf.reverse();
        }
    } else {
        // Unsigned: n >= 0 guaranteed above.
        let n = n as u64;
        let n_bytes: [u8; 8] = n.to_le_bytes();
        // Check n fits in `length` unsigned bytes.
        if length == 0 || (length < 8 && n >= (1u64 << (8 * length))) {
            return Err(PyError::named(
                "OverflowError",
                "int too big to convert".to_string(),
            ));
        }
        for (i, dst) in buf.iter_mut().enumerate() {
            *dst = if i < 8 { n_bytes[i] } else { 0 };
        }
        if big_endian {
            buf.reverse();
        }
    }

    Ok(Value::bytes(buf))
}

fn bigint_to_bytes(n: &PyBigInt, length: usize, big_endian: bool, signed: bool) -> Result<Value> {
    if !signed && n.sign() == PyBigIntSign::Minus {
        return Err(PyError::named(
            "OverflowError",
            "can't convert negative int to unsigned".to_string(),
        ));
    }

    if n.sign() == PyBigIntSign::NoSign {
        return Ok(Value::bytes(vec![0u8; length]));
    }

    let mut buf = vec![0u8; length];

    if signed {
        let le_bytes = n.to_signed_bytes_le();
        if le_bytes.len() > length {
            return Err(PyError::named(
                "OverflowError",
                "int too big to convert".to_string(),
            ));
        }
        let sign_byte = if n.sign() == PyBigIntSign::Minus {
            0xffu8
        } else {
            0u8
        };
        for (i, dst) in buf.iter_mut().enumerate() {
            *dst = if i < le_bytes.len() {
                le_bytes[i]
            } else {
                sign_byte
            };
        }
        if big_endian {
            buf.reverse();
        }
    } else {
        let (_, le_bytes) = n.to_bytes_le();
        if le_bytes.len() > length {
            return Err(PyError::named(
                "OverflowError",
                "int too big to convert".to_string(),
            ));
        }
        for (i, dst) in buf.iter_mut().enumerate() {
            *dst = if i < le_bytes.len() { le_bytes[i] } else { 0 };
        }
        if big_endian {
            buf.reverse();
        }
    }

    Ok(Value::bytes(buf))
}

/// Implements `int.from_bytes(bytes, byteorder='big', *, signed=False)`.
/// Called both as classmethod (`int.from_bytes(...)`) and as instance method
/// (`(5).from_bytes(...)`); the receiver is always ignored.
pub fn int_from_bytes(args: &[Value], kw: &PyDict) -> Result<Value> {
    if args.len() > 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "from_bytes() takes at most 2 positional arguments ({} given)",
                args.len()
            ),
        ));
    }

    let mut bytes_val: Option<Value> = args.first().cloned();
    let mut byteorder_val: Option<Value> = args.get(1).cloned();
    let mut signed_val: Option<Value> = None;

    for (k, v) in kw {
        let name = match k {
            PyKey::Str(s) => s.as_str().unwrap_or("").to_owned(),
            _ => continue,
        };
        match name.as_str() {
            "bytes" => {
                if bytes_val.is_some() {
                    return Err(PyError::named(
                        "TypeError",
                        "argument for from_bytes() given by name ('bytes') and position (1)"
                            .to_string(),
                    ));
                }
                bytes_val = Some(v.clone());
            }
            "byteorder" => {
                if byteorder_val.is_some() {
                    return Err(PyError::named(
                        "TypeError",
                        "argument for from_bytes() given by name ('byteorder') and position (2)"
                            .to_string(),
                    ));
                }
                byteorder_val = Some(v.clone());
            }
            "signed" => {
                signed_val = Some(v.clone());
            }
            other => {
                return Err(PyError::named(
                    "TypeError",
                    format!("from_bytes() got an unexpected keyword argument '{other}'"),
                ));
            }
        }
    }

    let bytes_v = match bytes_val {
        None => {
            return Err(PyError::named(
                "TypeError",
                "from_bytes() missing required argument: 'bytes'".to_string(),
            ));
        }
        Some(v) => v,
    };

    let data: Vec<u8> = match bytes_v.kind() {
        ValueKind::Bytes(b) => b.to_vec(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "from_bytes() argument 'bytes' must be a bytes-like object, not '{}'",
                    pyrust_core::builtin_type_name(&bytes_v)
                ),
            ));
        }
    };

    let big_endian = match byteorder_val {
        None => true,
        Some(v) => match v.kind() {
            ValueKind::Str(s) => {
                let s = s.to_string();
                if s == "big" {
                    true
                } else if s == "little" {
                    false
                } else {
                    return Err(PyError::named(
                        "ValueError",
                        "byteorder must be either 'little' or 'big'".to_string(),
                    ));
                }
            }
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "from_bytes() argument 'byteorder' must be str, not {}",
                        pyrust_core::builtin_type_name(&v)
                    ),
                ));
            }
        },
    };

    let signed: bool = match signed_val {
        None => false,
        Some(v) => match v.kind() {
            ValueKind::Bool(b) => b,
            ValueKind::Int(n) => n != 0,
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "from_bytes() argument 'signed' must be a bool, not '{}'",
                        pyrust_core::builtin_type_name(&v)
                    ),
                ));
            }
        },
    };

    if data.is_empty() {
        return Ok(Value::int(0));
    }

    let n: PyBigInt = if signed {
        if big_endian {
            PyBigInt::from_signed_bytes_be(&data)
        } else {
            PyBigInt::from_signed_bytes_le(&data)
        }
    } else {
        if big_endian {
            PyBigInt::from_bytes_be(PyBigIntSign::Plus, &data)
        } else {
            PyBigInt::from_bytes_le(PyBigIntSign::Plus, &data)
        }
    };

    // Try to fit in i64, otherwise keep as BigInt.
    if let Some(i) = PyToPrimitive::to_i64(&n) {
        Ok(Value::int(i))
    } else {
        Ok(Value::bigint(n))
    }
}
