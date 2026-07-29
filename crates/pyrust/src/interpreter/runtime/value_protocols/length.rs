// Canonical validation for values returned by Python's `__len__` slot.

/// Outcome of looking up and calling `type(obj).__length_hint__`.
enum LengthHintSlot {
    /// The type has no such slot.
    Missing,
    /// The slot raised `TypeError`, which CPython swallows in favour of the
    /// caller's default.
    RaisedTypeError,
    Value(Value),
}

impl Interpreter {
    /// Normalize a user `__len__` result to the interpreter's Py_ssize_t-sized
    /// representation.
    ///
    /// Both `len(value)` and truth-value testing route through this boundary so
    /// they accept and reject the same protocol results:
    ///
    /// - integers, integer subclasses, and values implementing `__index__`
    ///   are accepted;
    /// - every negative integer raises `ValueError`, including a negative
    ///   bigint too small to fit in `i64`;
    /// - a non-negative integer larger than `i64::MAX` raises `OverflowError`;
    /// - non-index values retain the canonical index-protocol `TypeError`.
    pub(crate) fn normalize_len_result(&mut self, result: &Value) -> Result<i64> {
        let resolved = self.value_to_index(result, |value| {
            pyrust_core::type_err!(
                "'{}' object cannot be interpreted as an integer",
                pyrust_core::builtin_type_name(value),
            )
        })?;

        match resolved.kind() {
            ValueKind::Int(value) if value < 0 => {
                Err(pyrust_core::value_err!("__len__() should return >= 0"))
            }
            ValueKind::Int(value) => Ok(value),
            ValueKind::Bool(value) => Ok(if value { 1 } else { 0 }),
            ValueKind::BigInt(value) if value.sign() == num_bigint::Sign::Minus => {
                Err(pyrust_core::value_err!("__len__() should return >= 0"))
            }
            ValueKind::BigInt(value) => {
                use crate::value::PyToPrimitive;

                value.to_i64().ok_or_else(|| {
                    pyrust_core::overflow_err!("cannot fit 'int' into an index-sized integer")
                })
            }
            _ => unreachable!("value_to_index guarantees an integer"),
        }
    }

    /// CPython's `PyObject_LengthHint` — an estimate of how many items `value`
    /// will produce (issue #2920).
    ///
    /// The one shared implementation of the hint protocol:
    ///
    /// 1. an exact `len()` wins whenever the object has one; a `TypeError` from
    ///    that slot means "no length" and falls through, while any other
    ///    exception propagates;
    /// 2. otherwise `__length_hint__` is looked up on the *type* (built-in
    ///    iterators answer from their cursor, see
    ///    [`Interpreter::builtin_iterator_length_hint_value`]); a missing slot
    ///    yields `default`;
    /// 3. a `TypeError` raised by the slot, or a `NotImplemented` result, also
    ///    yields `default`;
    /// 4. a non-integer result is a `TypeError`, a negative one a `ValueError`,
    ///    and one too large for an index-sized integer an `OverflowError`.
    pub(crate) fn object_length_hint(&mut self, value: &Value, default: i64) -> Result<i64> {
        match self.object_len(value) {
            Ok(length) => return Ok(length),
            Err(error) if error.class_name_is("TypeError") => {}
            Err(error) => return Err(error),
        }

        let hint = match self.length_hint_slot_result(value)? {
            LengthHintSlot::Value(hint) => hint,
            LengthHintSlot::Missing | LengthHintSlot::RaisedTypeError => return Ok(default),
        };
        if hint.is_not_implemented() {
            return Ok(default);
        }
        // CPython gates on `PyLong_Check`, so a `bool` and an `int` subclass
        // both pass and `__index__` is never consulted; anything else names
        // its own type in the error.
        let Some(count) = normalize_int_slot_result(&hint) else {
            return Err(pyrust_core::type_err!(
                "__length_hint__ must be an integer, not {}",
                value_type_name_str(&hint)
            ));
        };
        Self::length_hint_to_isize(&count)
    }

    /// Invoke `type(value).__length_hint__`.
    ///
    /// A built-in iterator answers from its own cursor; every other object goes
    /// through the ordinary type-level special-method lookup, so an instance
    /// attribute of the same name is ignored exactly as CPython's
    /// `_PyObject_LookupSpecial` ignores it.
    fn length_hint_slot_result(&mut self, value: &Value) -> Result<LengthHintSlot> {
        // A `TypeError` out of the slot — including one raised by the live
        // `__len__` a legacy sequence iterator consults — is the caller's
        // signal to fall back on its default, not an error to propagate.
        if matches!(value.kind(), ValueKind::Generator(_)) {
            return match self.builtin_iterator_length_hint_value(value) {
                Ok(Some(hint)) => Ok(LengthHintSlot::Value(hint)),
                Ok(None) => Ok(LengthHintSlot::Missing),
                Err(error) if error.class_name_is("TypeError") => {
                    Ok(LengthHintSlot::RaisedTypeError)
                }
                Err(error) => Err(error),
            };
        }
        let Some(method) = lookup_value_special_method(value, "__length_hint__") else {
            return Ok(LengthHintSlot::Missing);
        };
        match invoke_class_method(self, method, value.clone(), &[]) {
            Ok(hint) => Ok(LengthHintSlot::Value(hint)),
            Err(error) if error.class_name_is("TypeError") => Ok(LengthHintSlot::RaisedTypeError),
            Err(error) => Err(error),
        }
    }

    /// Narrow a validated `__length_hint__` result to `Py_ssize_t`.
    ///
    /// CPython converts before range-checking, so a magnitude beyond
    /// `Py_ssize_t` is an `OverflowError` whichever sign it carries; only a
    /// representable negative reaches the `>= 0` check.
    fn length_hint_to_isize(hint: &Value) -> Result<i64> {
        use crate::value::PyToPrimitive;

        let hint = match hint.kind() {
            ValueKind::Int(hint) => hint,
            ValueKind::BigInt(hint) => hint.to_i64().ok_or_else(|| {
                pyrust_core::overflow_err!("Python int too large to convert to C ssize_t")
            })?,
            _ => unreachable!("normalize_int_slot_result yields a plain integer"),
        };
        if hint < 0 {
            return Err(pyrust_core::value_err!(
                "__length_hint__() should return >= 0"
            ));
        }
        Ok(hint)
    }

    /// `len(value)` through the registered builtin, so the hint protocol and
    /// the `len()` builtin cannot disagree about what has a length.
    fn object_len(&mut self, value: &Value) -> Result<i64> {
        let dispatch = crate::builtin_registry::lookup("len").expect("len must be in the registry");
        let result = dispatch(
            self,
            &[ExpandedCallArg {
                name: None,
                value: value.clone(),
            }],
        )?;
        self.normalize_len_result(&result)
    }
}
