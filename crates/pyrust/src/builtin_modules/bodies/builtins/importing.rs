use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: __import__(name, globals=None, locals=None, fromlist=(), level=0)
    /// <https://docs.python.org/3/library/functions.html#import__>
    ///
    /// Migrated to the typed-signature dialect (#400): `#[arity_style(clinic)]`
    /// binds all five parameters positionally or by keyword with CPython 3.12's
    /// exact error wording. Empty or absent fromlist returns the top-level
    /// package (e.g. os for "os.path"); non-empty fromlist loads any missing
    /// submodules on packages and returns the leaf module directly.
    #[arity_style(clinic)]
    fn __import__(
        name: PyValueRef,
        #[default(OptionalArg::Omitted)] globals: OptValueRef,
        #[default(OptionalArg::Omitted)] locals: OptValueRef,
        #[default(OptionalArg::Omitted)] fromlist: OptValueRef,
        #[default(OptionalArg::Omitted)] level: OptValueRef,
    ) -> Result<Value> {
        let _ = locals;
        let name = match name.0.as_str() {
            Some(s) => s.to_string(),
            None => {
                return Err(PyError::named(
                    "TypeError",
                    "module name must be a string".to_string(),
                ));
            }
        };
        let level_i64: i64 = match level.as_value() {
            None => 0,
            Some(v) => match v.kind() {
                ValueKind::Int(n) => n,
                ValueKind::Bool(b) => i64::from(b),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "'{}' object cannot be interpreted as an integer",
                            pyrust_core::builtin_type_name(v)
                        ),
                    ));
                }
            },
        };
        if level_i64 < 0 {
            return Err(PyError::named(
                "ValueError",
                "level must be >= 0".to_string(),
            ));
        }
        let level_usize = level_i64 as usize;
        // CPython raises ValueError for an empty module name when level == 0.
        if level_usize == 0 && name.is_empty() {
            return Err(PyError::named("ValueError", "Empty module name".to_string()));
        }
        let target_name = if level_usize > 0 {
            let globals_val = globals.as_value().filter(|v| !v.is_none());
            _interp.resolve_relative_module_name(&name, level_usize, globals_val)?
        } else {
            name.clone()
        };
        let fromlist_items: Option<Vec<Value>> = match fromlist.as_value() {
            None => None,
            Some(v) if v.is_none() => None,
            Some(v) => {
                let items = _interp.collect_iterable(v)?;
                if items.is_empty() {
                    None
                } else {
                    Some(items)
                }
            }
        };
        // Load the full dotted module (triggers caching of parent packages & submodules).
        let leaf = _interp.load_module(&target_name)?;
        if let Some(items) = fromlist_items {
            // Non-empty fromlist: for packages, load missing submodules named
            // in fromlist (matching CPython `_handle_fromlist`) and return leaf.
            if _interp.module_has_package_path(&leaf)? {
                let pkg_name = match _interp.get_attr(&leaf, "__name__") {
                    Ok(v) if v.as_str().is_some() => v.as_str().unwrap().to_string(),
                    _ => target_name.clone(),
                };
                for item in items {
                    let Some(attr) = item.as_str() else {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "Item in ``from list'' must be str, not {}",
                                pyrust_core::builtin_type_name(&item)
                            ),
                        ));
                    };
                    if attr == "*" {
                        continue;
                    }
                    match _interp.get_attr(&leaf, attr) {
                        Ok(_) => {}
                        Err(err) if err.class_name_is("AttributeError") => {
                            let sub_name = format!("{pkg_name}.{attr}");
                            match _interp.load_module(&sub_name) {
                                Ok(_) => {}
                                Err(load_err)
                                    if _interp.is_missing_module_error(&load_err, &sub_name) => {}
                                Err(load_err) => return Err(load_err),
                            }
                        }
                        Err(err) => return Err(err),
                    }
                }
            }
            Ok(leaf)
        } else if level_usize == 0 {
            // Empty fromlist with absolute import: return the top-level package.
            let top_name = target_name.split('.').next().unwrap_or(&target_name);
            if top_name == target_name {
                Ok(leaf)
            } else {
                _interp.load_module(top_name)
            }
        } else if name.is_empty() {
            Ok(leaf)
        } else {
            let first_rel = name.split('.').next().unwrap_or(&name);
            let base = target_name
                .strip_suffix(&format!(".{name}"))
                .unwrap_or(&target_name);
            let front = format!("{base}.{first_rel}");
            if front == target_name {
                Ok(leaf)
            } else {
                _interp.load_module(&front)
            }
        }
    }
}
