/// Returns the Python built-in type name (e.g. `"list"`, `"str"`) for a
/// `Value`.  Used by error messages (`'X' object is not iterable`, attribute
/// errors), built-in method repr strings (`<built-in method append of list
/// object>`), and similar diagnostics.
///
/// This is the canonical implementation — every crate in the workspace
/// routes type-name lookup through this function so naming stays consistent.
/// The match is exhaustive over [`ValueKind`]; new variants must be added
/// here, not in per-crate copies.
///
/// Python-visible type name for a value, used in error messages and `type(x).__name__`.
///
/// Returns `Cow<'static, str>` so the common builtin arms stay zero-allocation
/// (`Cow::Borrowed`), while `PyInstance` can honestly report its runtime class
/// name (`Cow::Owned`) instead of the placeholder `"object"` (issue #437).
pub fn builtin_type_name(value: &Value) -> Cow<'static, str> {
    match value.kind() {
        ValueKind::None => Cow::Borrowed("NoneType"),
        ValueKind::Bool(_) => Cow::Borrowed("bool"),
        ValueKind::Int(_) | ValueKind::BigInt(_) => Cow::Borrowed("int"),
        ValueKind::Float(_) => Cow::Borrowed("float"),
        ValueKind::Str(_) => Cow::Borrowed("str"),
        ValueKind::List(_) => Cow::Borrowed("list"),
        ValueKind::Tuple(_) => Cow::Borrowed("tuple"),
        ValueKind::Dict(_) => Cow::Borrowed("dict"),
        ValueKind::Set(_) => Cow::Borrowed("set"),
        ValueKind::Range { .. } | ValueKind::BigRange { .. } => Cow::Borrowed("range"),
        ValueKind::Bytes(_) => Cow::Borrowed("bytes"),
        ValueKind::Complex(_, _) => Cow::Borrowed("complex"),
        ValueKind::BuiltinFunction(name) => {
            Cow::Borrowed(builtin_callable_presentation(name).type_name())
        }
        ValueKind::BoundMethod { .. } | ValueKind::ClassBoundMethod { .. } => {
            Cow::Borrowed("method")
        }
        ValueKind::UserFunction(f) => match f.kind {
            UserFunctionKind::StaticMethod => Cow::Borrowed("staticmethod"),
            UserFunctionKind::ClassMethod => Cow::Borrowed("classmethod"),
            _ => Cow::Borrowed("function"),
        },
        ValueKind::PyClass(class) => {
            // A class object is an instance of its metaclass. Ordinary classes
            // store no explicit metatype and therefore report `type`; classes
            // created through a custom metaclass must report that metaclass's
            // live Python-visible name in diagnostics and `type`-name queries.
            // Clone the optional owner before borrowing its name so neither
            // RefCell borrow spans the returned allocation.
            let metatype = class.borrow().metatype.clone();
            metatype.map_or(Cow::Borrowed("type"), |metatype| {
                Cow::Owned(metatype.borrow().name.clone())
            })
        }
        ValueKind::PyInstance(inst) => Cow::Owned(inst.borrow().class.borrow().name.clone()),
        ValueKind::PyModule(_) => Cow::Borrowed("module"),
        ValueKind::SuperProxy { .. }
        | ValueKind::SuperProxyClass { .. }
        | ValueKind::SuperProxyUnbound { .. } => Cow::Borrowed("super"),
        // Built-in iterators share this tag and have no single name here; the
        // interpreter's `full_type_name_str` refines them.  The three frame
        // kinds are distinguished by their immutable tag, which stays readable
        // while the frame is running (#2978).
        ValueKind::Generator(cell) => {
            Cow::Borrowed(cell.kind().frame_type_name().unwrap_or("generator"))
        }
        ValueKind::NotImplemented => Cow::Borrowed("NotImplementedType"),
        ValueKind::Ellipsis => Cow::Borrowed("ellipsis"),
        ValueKind::BuiltinObject { ops, .. } => Cow::Borrowed(ops.display_type_name()),
    }
}

/// Display name for a value used in error messages of the form `"not <name>"`.
///
/// CPython special-cases `None` in certain TypeError contexts: it prints the
/// singleton display name (`"None"`) rather than the type name (`"NoneType"`).
/// Affected methods include `bytes.decode()`, `str.encode()`, `str.replace()`,
/// and similar argument-type checks where the message reads
/// `"argument '<param>' must be str, not <name>"`.
///
/// Use this function instead of [`builtin_type_name`] when constructing those
/// messages. Use [`builtin_type_name`] everywhere else (e.g. `"'NoneType' object
/// is not iterable"`, `"bad operand type for abs(): 'NoneType'"`) where CPython
/// still uses the class name.
pub fn py_value_display_name(value: &Value) -> Cow<'static, str> {
    if matches!(value.kind(), ValueKind::None) {
        Cow::Borrowed("None")
    } else {
        builtin_type_name(value)
    }
}
