// Module attribute semantics.
impl Interpreter {
    /// Expose the one live dictionary owned by a filesystem-backed module.
    ///
    /// While the loader's child environment is alive, route through the root
    /// exposure helper so `LoadGlobal` caches and the module mutation token are
    /// permanently disabled before arbitrary dict aliases can mutate it. A
    /// data-only module may outlive that environment; its strongly-owned
    /// dictionary remains the complete namespace in that case.
    pub(crate) fn filesystem_module_globals(
        &self,
        module: &Rc<RefCell<PyModule>>,
    ) -> Option<Value> {
        let namespace = module.borrow().filesystem_namespace()?;
        Some(match namespace.environment() {
            Some(environment) => self.globals_for_environment(&environment),
            None => namespace.globals(),
        })
    }

    /// Resolve one name for `from module import name`, translating a missing
    /// module attribute to `ImportError` at the namespace boundary.
    pub(crate) fn import_from_attribute(&mut self, module: &Value, name: &str) -> Result<Value> {
        match self.get_attr(module, name) {
            Ok(value) => Ok(value),
            Err(error) if error.class_name_is("AttributeError") => {
                let module_name = match module.kind() {
                    ValueKind::PyModule(module) => module.borrow().name.clone(),
                    _ => "<unknown>".to_string(),
                };
                Err(PyError::import_error(
                    "ImportError",
                    format!("cannot import name '{name}' from '{module_name}' (unknown location)"),
                    Some(module_name),
                ))
            }
            Err(error) => Err(error),
        }
    }

    pub(super) fn get_module_attribute(
        &mut self,
        target: &Value,
        module: &Rc<RefCell<PyModule>>,
        name: &str,
    ) -> Result<Value> {
        let module = Rc::clone(module);
        let filesystem_namespace = module.borrow().filesystem_namespace();
        let live_namespace = module.borrow().live_namespace();
        let has_explicit_namespace = filesystem_namespace.is_some() || live_namespace.is_some();

        // Native modules that opt into an exposed namespace (currently the
        // canonical `sys` module) return their exact backing dict. Attribute
        // assignment and direct `module.__dict__` mutation therefore share
        // identity and cache-invalidation state.
        if name == "__dict__"
            && let Some(namespace) = live_namespace
        {
            return Ok(namespace);
        }

        // A source-backed module owns the exact root globals dict used by its
        // functions. Expose that same object, never a harvested snapshot.
        if name == "__dict__"
            && let Some(namespace) = filesystem_namespace.as_ref()
        {
            return Ok(match namespace.environment() {
                Some(environment) => self.globals_for_environment(&environment),
                None => namespace.globals(),
            });
        }

        // CPython 3.12 module_getattro builds the error message by
        // looking up __name__ in the module's __dict__.  If __name__
        // is absent (e.g. it was deleted), the error omits the module
        // name: "module has no attribute 'X'" rather than
        // "module 'foo' has no attribute 'X'".
        // Precompute this once for both error sites below.
        let stored_name = module.borrow().get_attr_value("__name__");
        let name_tombstoned = if has_explicit_namespace {
            stored_name.as_ref().is_none_or(Value::is_unset)
        } else {
            stored_name.as_ref().is_some_and(Value::is_unset)
        };
        if let Some(value) = module.borrow().get_attr_value(name) {
            // A stored Value::unset() is a deletion tombstone written by
            // delete_attr for synthetic dunders.  Treat it as absent.
            if value.is_unset() {
                let msg = if name_tombstoned {
                    format!("module has no attribute '{name}'")
                } else {
                    let mod_name = module.borrow().name.clone();
                    format!("module '{mod_name}' has no attribute '{name}'")
                };
                return Err(pyrust_core::py_err!("AttributeError", msg));
            }
            return Ok(value);
        }
        // Filesystem modules seed their real dunders directly into the shared
        // globals dictionary. Missing entries stay missing after deletion;
        // only built-in modules use the synthetic fallback below.
        if has_explicit_namespace {
            let mod_name = module.borrow().name.clone();
            let msg = if name_tombstoned {
                format!("module has no attribute '{name}'")
            } else {
                format!("module '{mod_name}' has no attribute '{name}'")
            };
            return Err(PyError::attribute_error(
                msg,
                Some(name.to_string()),
                Some(target.clone()),
            ));
        }

        // Synthetic dunder attributes for built-in modules.  These are
        // not stored in the attrs map (to avoid polluting vars(m)) but
        // are synthesised here, mirroring CPython 3.12 module object
        // slot behaviour:
        //   __name__    — the module's dotted name string.
        //   __package__ — empty string for all top-level builtin modules
        //                 (CPython 3.12: `sys.__package__ == ''`).
        //   __loader__  — None; a full BuiltinImporter object is out of
        //                 scope for this implementation.
        //   __spec__    — None; same reason.
        //   __doc__     — None; pyrust does not store module docstrings.
        // Note: __file__ is intentionally absent.  CPython 3.12 raises
        // AttributeError for `sys.__file__`; builtin modules have no
        // file path to report.
        let mod_name = module.borrow().name.clone();
        match name {
            "__name__" => return Ok(Value::string(mod_name)),
            "__package__" => return Ok(Value::string(String::new())),
            "__loader__" | "__spec__" => return Ok(Value::none()),
            "__doc__" => return Ok(Value::none()),
            "__dict__" => {
                // Build a snapshot dict of the module namespace.
                // Include both the stored attrs and the synthetic
                // dunder attributes that get_attr synthesises above.
                // Value::unset() is a deletion tombstone (written by
                // delete_attr for synthetic dunders); filter it out so
                // deleted dunders don't appear in __dict__.
                let attrs_snapshot: HashMap<String, Value> = module.borrow().attrs.clone();
                let mut d: PyDict = attrs_snapshot
                    .iter()
                    .filter(|(_, v)| !v.is_unset())
                    .map(|(k, v)| (PyKey::str_from(k), v.clone()))
                    .collect();
                // Synthetic dunders: add only if the key is neither
                // already present in attrs (user override) nor
                // tombstoned (explicitly deleted by the user).
                let is_absent = |key: &str| !attrs_snapshot.contains_key(key);
                let name_key = PyKey::str_from("__name__");
                if is_absent("__name__") {
                    d.insert(name_key, Value::string(mod_name));
                }
                let pkg_key = PyKey::str_from("__package__");
                if is_absent("__package__") {
                    d.insert(pkg_key, Value::string(String::new()));
                }
                let spec_key = PyKey::str_from("__spec__");
                if is_absent("__spec__") {
                    d.insert(spec_key, Value::none());
                }
                let loader_key = PyKey::str_from("__loader__");
                if is_absent("__loader__") {
                    d.insert(loader_key, Value::none());
                }
                let doc_key = PyKey::str_from("__doc__");
                if is_absent("__doc__") {
                    d.insert(doc_key, Value::none());
                }
                return Ok(Value::dict(d));
            }
            _ => {}
        }
        let msg = if name_tombstoned {
            format!("module has no attribute '{name}'")
        } else {
            format!("module '{mod_name}' has no attribute '{name}'")
        };
        Err(PyError::attribute_error(
            msg,
            Some(name.to_string()),
            Some(target.clone()),
        ))
    }
}
