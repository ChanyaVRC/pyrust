use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: __import__(name, globals=None, locals=None, fromlist=(), level=0)
    /// <https://docs.python.org/3/library/functions.html#import__>
    ///
    /// The hook behind the import statement. Empty or absent fromlist returns
    /// the top-level package (e.g. os for "os.path"); non-empty fromlist
    /// returns the leaf module directly. globals, locals, and level are
    /// accepted but ignored.
    fn __import__(args) -> Result<Value> {
        if args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() missing required argument 'name' (pos 1)"),
            ));
        }
        let name = match args[0].value.as_str() {
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
        // Arg index 3 is `fromlist`; also accept as a keyword arg.
        let fromlist: Option<&Value> = args.get(3).map(|a| &a.value).or_else(|| {
            args.iter()
                .find(|a| a.name.as_deref() == Some("fromlist"))
                .map(|a| &a.value)
        });
        let fromlist_nonempty = match fromlist {
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
