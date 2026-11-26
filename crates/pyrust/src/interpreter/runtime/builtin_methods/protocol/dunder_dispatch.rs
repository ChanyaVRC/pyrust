// Top-level builtin protocol dunder dispatch and slot-wrapper arity validation.

/// Validate positional arity for fixed-arity protocol slot wrappers, matching
/// CPython 3.12's slot-dependent error wording.
fn validate_protocol_dunder_arity(method: &str, type_name: &str, given: usize) -> Result<()> {
    // Arity check up front so the error matches CPython 3.12's slot-wrapper
    // messages rather than a downstream operator error.  CPython's wording
    // is slot-dependent (verified against `python3.12`):
    //   - `mp_subscript` (dict/list `__getitem__`) and `sq_contains`
    //     (dict/set/frozenset `__contains__`) are *named* method-wrappers:
    //     `{type}.{name}() takes exactly one argument ({n} given)`.
    //   - the anonymous sequence slots (`sq_item`/`sq_concat`/`sq_ass_item`
    //     /…) use `expected N argument(s), got M`; `sq_repeat` (`__mul__`)
    //     and `sq_ass_item` (`__setitem__`) carry a leading space.
    let want: usize = match method {
        "__len__" | "__neg__" | "__pos__" | "__abs__" | "__invert__" | "__hash__" | "__str__"
        | "__repr__" | "__bool__" | "__reversed__" | "__iter__" | "__float__" | "__int__"
        // Issue #2297: `int.__index__`/`__trunc__`/`__floor__`/`__ceil__` are
        // zero-arg slot wrappers.
        | "__index__" | "__trunc__" | "__floor__" | "__ceil__" => 0,
        // Issue #3326: `complex.__complex__` / `bytes.__bytes__` are named
        // no-arg method_descriptors (checked just below).
        "__complex__" | "__bytes__" => 0,
        "__setitem__" => 2,
        _ => 1,
    };
    if given == want {
        return Ok(());
    }
    // `dict.__reversed__()` is a named no-arg method-wrapper in CPython
    // 3.12: `dict.__reversed__() takes no arguments (N given)` (#2093).
    // Checked before the generic named-wrapper arm below because
    // `__reversed__` is in `is_named_protocol_wrapper` (its kwarg
    // rejection is named, #2398) but its *arity* wording is the no-arg
    // form, not "takes exactly one argument".
    if method == "__reversed__" {
        return Err(pyrust_core::type_err!(
            "{type_name}.__reversed__() takes no arguments ({given} given)"
        ));
    }
    // Issue #2297/#2481: `int`/`float`.`__trunc__`/`__floor__`/`__ceil__`
    // are named no-arg method-wrappers in CPython 3.12.  The int slots
    // report the owning type `int` even for a `bool` receiver
    // (`int.__trunc__() takes no arguments (N given)`); the float slots
    // report `float`.  `__index__` (just below, anonymous int slot)
    // keeps the "expected 0 arguments, got N" form instead.
    if matches!(method, "__trunc__" | "__floor__" | "__ceil__") {
        let owner = if type_name == "float" { "float" } else { "int" };
        return Err(pyrust_core::type_err!(
            "{owner}.{method}() takes no arguments ({given} given)"
        ));
    }
    if matches!(method, "__complex__" | "__bytes__") {
        return Err(pyrust_core::type_err!(
            "{type_name}.{method}() takes no arguments ({given} given)"
        ));
    }
    if is_named_protocol_wrapper(method, type_name) {
        return Err(pyrust_core::type_err!(
            "{type_name}.{method}() takes exactly one argument ({given} given)"
        ));
    }
    // `__mul__` (sq_repeat), `__imul__` (sq_inplace_repeat) and
    // `__setitem__` (sq_ass_item) print a leading space before
    // "expected" in CPython 3.12.
    let lead = if matches!(method, "__mul__" | "__imul__" | "__setitem__") {
        " "
    } else {
        ""
    };
    let plural = if want == 1 { "argument" } else { "arguments" };
    Err(pyrust_core::type_err!(
        "{lead}expected {want} {plural}, got {given}"
    ))
}

impl Interpreter {
    /// Issue #1909: execute a container/sequence protocol dunder on a built-in
    /// primitive receiver, routing through the same operator machinery the
    /// implicit operators use so results and error messages match CPython 3.12
    /// exactly.  `method` must be one of the names in
    /// [`builtin_protocol_dunders`] for `receiver`'s type; `args` are the
    /// positional arguments after the receiver (so `l.__getitem__(0)` arrives
    /// as `["__getitem__", l, [0]]`).  `__iter__` is handled separately by the
    /// callers (it is in each type's `METHODS` slice, not the dunder set).
    pub(crate) fn dispatch_builtin_protocol_dunder(
        &mut self,
        method: &str,
        receiver: Value,
        mut args: Vec<Value>,
    ) -> Result<Value> {
        // Issue #2953: primitive protocol dispatch routes back through builtin
        // operations (`len`, `iter`, `eval_index`, …). A `PyInstance` receiver
        // here would re-enter the exact same slot and overflow the native stack.
        if matches!(receiver.kind(), ValueKind::PyInstance(_)) {
            let type_name = pyrust_core::builtin_type_name(&receiver);
            return Err(PyError::runtime(format!(
                "'{type_name}' object is missing internal builtin backing for '{method}'"
            )));
        }
        let type_name = pyrust_core::builtin_type_name(&receiver);
        // Issue #2297: `int.__round__(ndigits=None)` accepts 0 or 1 positional
        // argument — handle its arity separately from the fixed-arity slots below
        // (CPython's C-clinic wording: `__round__ expected at most 1 argument,
        // got N`).  Routed before the fixed `want` check so the optional second
        // operand does not trip the generic "expected 1 argument" path.
        if method == "__round__" {
            if args.len() > 1 {
                return Err(pyrust_core::type_err!(
                    "__round__ expected at most 1 argument, got {}",
                    args.len()
                ));
            }
            // Issue #2481: `float.__round__([ndigits])` returns an `int` when
            // `ndigits` is omitted (`(1.5).__round__()` → `2`, banker's
            // rounding) and a `float` when given (`(1.5).__round__(0)` →
            // `2.0`).  Unlike `int.__round__`, the float slot treats an explicit
            // `ndigits=None` as omitted (`(1.7).__round__(None)` → `2`), exactly
            // as the `round()` builtin does — so route straight through it
            // rather than the int-only `int_round_dunder` (which rejects None).
            if type_name == "float" {
                let dispatch = crate::builtin_registry::lookup("round")
                    .expect("round must be in the registry");
                let mut call_args = vec![ExpandedCallArg {
                    name: None,
                    value: receiver.clone(),
                }];
                if let Some(n) = args.pop() {
                    call_args.push(ExpandedCallArg {
                        name: None,
                        value: n,
                    });
                }
                return dispatch(self, &call_args);
            }
            return self.int_round_dunder(&receiver, args.pop());
        }
        // `int`/`bool`/`float`/`complex`.`__pow__(exp[, mod])` accepts an
        // optional modulus argument (CPython's `ternaryfunc` `nb_power`).
        // With 1 arg (or `mod=None`) fall through to the 2-arg binary path;
        // with a non-None modulus, check numeric operand acceptance and
        // delegate to the `pow` builtin so `(2).__pow__(5, 7)` and
        // `int.__pow__(MyInt(2), 5, 7)` match CPython 3.12.
        if method == "__pow__"
            && matches!(&*type_name, "int" | "bool" | "float" | "complex")
            && args.len() == 2
        {
            let mod_val = args.pop().unwrap();
            if !mod_val.is_none() {
                let exp_val = args.pop().unwrap();
                if !numeric_operand_accepted(&receiver, &exp_val)
                    || !numeric_operand_accepted(&receiver, &mod_val)
                {
                    return Ok(Value::not_implemented());
                }
                let dispatch =
                    crate::builtin_registry::lookup("pow").expect("pow must be in the registry");
                return dispatch(
                    self,
                    &[
                        ExpandedCallArg {
                            name: None,
                            value: receiver,
                        },
                        ExpandedCallArg {
                            name: None,
                            value: exp_val,
                        },
                        ExpandedCallArg {
                            name: None,
                            value: mod_val,
                        },
                    ],
                );
            }
        }
        validate_protocol_dunder_arity(method, &type_name, args.len())?;

        // Issue #2070: scalar/object dunders that route to a registry builtin or
        // an interpreter helper, exposed on every primitive type.  These never
        // collide with the container `__add__`/`__mul__`/… arms below.
        match method {
            "__getattribute__" => {
                let attribute = args.pop().unwrap();
                let ValueKind::Str(name) = attribute.kind() else {
                    return Err(pyrust_core::type_err!(
                        "attribute name must be string, not '{}'",
                        pyrust_core::builtin_type_name(&attribute)
                    ));
                };
                let name = name.to_string();
                return self.get_attr(&receiver, &name);
            }
            "__hash__" => {
                let h = hash_value_with_interp(self, &receiver)?;
                return Ok(Value::int(h));
            }
            "__str__" | "__repr__" => {
                let name = if method == "__str__" { "str" } else { "repr" };
                let arg = ExpandedCallArg {
                    name: None,
                    value: receiver,
                };
                let dispatch = crate::builtin_registry::lookup(name)
                    .unwrap_or_else(|| panic!("{name} must be in the registry"));
                return dispatch(self, &[arg]);
            }
            "__bool__" => {
                let b = self.truthy_value(&receiver)?;
                return Ok(Value::bool_(b));
            }
            // Issue #2433: `int.__float__`/`int.__int__` (exposed unbound and
            // bound on `int`).  CPython exposes these as int-owned slot wrappers;
            // `(5).__float__()` → `5.0`, `(5).__int__()` → `5`.  Route through
            // the `float`/`int` constructors so int-subclass receivers coerce
            // exactly as the builtins do.
            // Issue #3326: `(1.5).__int__()`, `(1j).__complex__()`, and
            // `b"x".__bytes__()` convert through their constructors in the same
            // way (an exact-type copy of a subclass receiver).
            "__float__" | "__int__" | "__complex__" | "__bytes__" => {
                let ctor = match method {
                    "__float__" => "float",
                    "__complex__" => "complex",
                    "__bytes__" => "bytes",
                    _ => "int",
                };
                let arg = ExpandedCallArg {
                    name: None,
                    value: receiver,
                };
                let dispatch = crate::builtin_registry::lookup(ctor)
                    .unwrap_or_else(|| panic!("{ctor} must be in the registry"));
                return dispatch(self, &[arg]);
            }
            // Issue #2481: `float.__trunc__`/`__floor__`/`__ceil__` round the
            // float to an `int` toward zero / -inf / +inf respectively
            // (`(-1.7).__floor__()` → `-2`, `(-1.7).__ceil__()` → `-1`) — they
            // are *not* identity like the int slots, so route through the
            // dedicated `float.__X__` registry bodies (which already handle
            // NaN/inf → ValueError/OverflowError, BigInt promotion and the
            // float-subclass receiver).
            "__trunc__" | "__floor__" | "__ceil__" if type_name == "float" => {
                let arg = ExpandedCallArg {
                    name: None,
                    value: receiver,
                };
                let dispatch = crate::builtin_registry::lookup(&format!("float.{method}"))
                    .expect("float.__trunc__/__floor__/__ceil__ must be in the registry");
                return dispatch(self, &[arg]);
            }
            // Issue #2297: `int.__index__`/`__trunc__`/`__floor__`/`__ceil__`
            // all return the integer value of the receiver unchanged
            // (`(5).__trunc__()` → `5`, `(7).__floor__()` → `7`).  Like CPython,
            // a `bool` receiver normalises to plain `int`
            // (`True.__index__()` → `1`, not `True`); route through the `int`
            // constructor, which preserves `BigInt` and performs that coercion.
            "__index__" | "__trunc__" | "__floor__" | "__ceil__" => {
                let arg = ExpandedCallArg {
                    name: None,
                    value: receiver,
                };
                let dispatch =
                    crate::builtin_registry::lookup("int").expect("int must be in the registry");
                return dispatch(self, &[arg]);
            }
            // #2093: `dict.__reversed__()` yields keys in reverse insertion
            // order.  Routes through the `reversed` builtin, which handles the
            // dict case directly.
            "__reversed__" => {
                let arg = ExpandedCallArg {
                    name: None,
                    value: receiver,
                };
                let dispatch = crate::builtin_registry::lookup("reversed")
                    .expect("reversed must be in the registry");
                return dispatch(self, &[arg]);
            }
            // Issue #2387: `__iter__` reached here via the type-level unbound
            // (`list.__iter__([1])`) or builtin-subclass (`LI([1]).__iter__()`)
            // paths — the bound primitive-instance form is intercepted earlier.
            // Wrap the (already-unwrapped) backing in the same `NativeIterFrame`
            // generator the `iter()` builtin produces.
            "__iter__" => {
                let arg = ExpandedCallArg {
                    name: None,
                    value: receiver,
                };
                let dispatch =
                    crate::builtin_registry::lookup("iter").expect("iter must be in the registry");
                return dispatch(self, &[arg]);
            }
            "__eq__" | "__ne__" | "__lt__" | "__le__" | "__gt__" | "__ge__" => {
                let other = args.pop().unwrap();
                return self.dispatch_richcmp_protocol_dunder(method, receiver, other);
            }
            _ => {}
        }
        // Issue #2070: scalar-numeric forward dunders (int/float/complex/bool).
        // Gated on the scalar types so the container `__add__`/`__mul__`/`__or__`
        // arms below keep their sequence/set semantics.  Returns `NotImplemented`
        // when the operand type is outside the receiver's accepted set.
        if matches!(&*type_name, "int" | "float" | "complex" | "bool")
            && let Some(result) = self.primitive_numeric_dunder(method, &receiver, &mut args)?
        {
            return Ok(result);
        }
        self.dispatch_container_protocol_dunder(method, &type_name, receiver, args)
    }
}
