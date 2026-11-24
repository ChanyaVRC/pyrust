use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: __import__(name, globals=None, locals=None, fromlist=(), level=0)
    /// <https://docs.python.org/3/library/functions.html#import__>
    ///
    /// Migrated to the typed-signature dialect (#400): `#[arity_style(clinic)]`
    /// binds all five parameters positionally or by keyword with CPython 3.12's
    /// exact error wording. Empty or absent fromlist returns the top-level
    /// package (e.g. os for "os.path"); non-empty fromlist returns the leaf
    /// module directly. globals, locals, and level are accepted but ignored.
    #[arity_style(clinic)]
    fn __import__(
        name: PyValueRef,
        #[default(OptionalArg::Omitted)] globals: OptValueRef,
        #[default(OptionalArg::Omitted)] locals: OptValueRef,
        #[default(OptionalArg::Omitted)] fromlist: OptValueRef,
        #[default(OptionalArg::Omitted)] level: OptValueRef,
    ) -> Result<Value> {
        let _ = (globals, locals, level);
        let name = match name.0.as_str() {
            Some(s) => s.to_string(),
            None => {
                return Err(PyError::named(
                    "TypeError",
                    "module name must be a string".to_string(),
                ));
            }
        };
        // CPython raises ValueError for an empty module name.
        if name.is_empty() {
            return Err(PyError::named("ValueError", "Empty module name".to_string()));
        }
        let fromlist_nonempty = match fromlist.as_value() {
            None => false,
            Some(v) => match v.kind() {
                ValueKind::None => false,
                ValueKind::Tuple(items) => !items.is_empty(),
                _ => v.as_list().map(|l| !l.is_empty()).unwrap_or(true),
            },
        };
        // Load the full dotted module (triggers caching of submodules).
        let leaf = _interp.load_module(&name)?;
        if fromlist_nonempty {
            // Non-empty fromlist: return the leaf (rightmost component).
            Ok(leaf)
        } else {
            // Empty fromlist: return the top-level package.
            let top_name = name.split('.').next().unwrap_or(&name);
            if top_name == name {
                Ok(leaf)
            } else {
                _interp.load_module(top_name)
            }
        }
    }
}
