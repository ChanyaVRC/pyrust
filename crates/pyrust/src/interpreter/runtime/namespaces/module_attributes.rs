/// CPython 3.12 `types.ModuleType.__doc__` string; also the fallback returned
/// when reading `mod.__doc__` after `del mod.__doc__` (issue #2971).
const MODULE_TYPE_DOC: &str = "Create a module object.\n\nThe name must be a string; the optional doc argument can have any type.";

// Module attribute semantics.
impl Interpreter {
    /// Flush live script fastlocals into the shared globals dictionary before
    /// reading/mutating a lazily-attached `__main__` module (#2957).
    ///
    /// Imported filesystem modules already mirror eagerly
    /// (`namespace_globals_require_mirroring() == true`), so this is a cheap
    /// single-flag check for them and leaves their `LoadGlobal` caches intact.
    pub(crate) fn ensure_module_namespace_synced(&self, module: &Rc<RefCell<PyModule>>) {
        let Some(namespace) = module.borrow().filesystem_namespace() else {
            return;
        };
        if let Some(environment) = namespace.environment()
            && !environment.borrow().namespace_globals_require_mirroring()
        {
            let _ = self.globals_for_environment(&environment);
        }
    }

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

        // Native modules that already hold a live namespace (`sys`,
        // `types.ModuleType` instances, or built-in modules upgraded on first
        // `__dict__` read) return their exact backing dict.  Saturate the
        // mutation token whenever an alias is exposed so caches never observe
        // stale values after direct dict mutation (#2970).
        if name == "__dict__"
            && let Some(namespace) = live_namespace
        {
            module.borrow().disable_cache();
            return Ok(namespace);
        }

        // A source-backed module owns the exact root globals dict used by its
        // functions. Expose that same object, never a harvested snapshot.
        if name == "__dict__"
            && let Some(globals) = self.filesystem_module_globals(&module)
        {
            return Ok(globals);
        }

        if filesystem_namespace.is_some() {
            self.ensure_module_namespace_synced(&module);
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
            // delete_attr for synthetic dunders.  Treat it as absent — except
            // for `__doc__`, which falls back to `type(module).__doc__` in
            // CPython 3.12 (issue #2971).
            if value.is_unset() {
                if name == "__doc__" {
                    return Ok(Value::string(MODULE_TYPE_DOC));
                }
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
        // CPython 3.12 (`types.ModuleType.__annotations__` getset descriptor):
        // reading `__annotations__` on a module whose `__dict__` does not yet
        // contain it lazily creates `{}` and appends `'__annotations__'` to
        // `module.__dict__` (issues #2919, #2971).
        if name == "__annotations__" {
            let empty = Value::dict(PyDict::default());
            module
                .borrow_mut()
                .insert_attr("__annotations__".to_string(), empty.clone());
            return Ok(empty);
        }
        // Filesystem modules and live-namespace modules seed their real dunders
        // directly into the shared dictionary. Missing entries stay missing
        // after deletion — except `__doc__`, which falls back to
        // `type(module).__doc__` (issue #2971).
        if has_explicit_namespace {
            if name == "__doc__" {
                return Ok(Value::string(MODULE_TYPE_DOC));
            }
            if let Some(bound) = bind_module_class_attr(target, name) {
                return Ok(bound);
            }
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
                // Lazily upgrade this built-in module into one live Python
                // dictionary (issue #2970), preserving PR #2969's five-slot
                // CPython head and declaration-ordered body so `m.__dict__ is
                // vars(m)` and mutations through `__dict__` are immediately
                // visible through attribute access.
                return Ok(module.borrow_mut().attach_live_namespace_with_dunders());
            }
            _ => {}
        }
        if let Some(bound) = bind_module_class_attr(target, name) {
            return Ok(bound);
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

/// Resolve an inherited attribute from `types.ModuleType` (or `object`) on a
/// `PyModule` receiver, binding method descriptors to `target` (#2970).
fn bind_module_class_attr(target: &Value, name: &str) -> Option<Value> {
    let attr = lookup_class_attr(&module_type_singleton(), name)?;
    if name == "__new__" {
        return Some(attr);
    }
    if let Some(wrapped) = pyrust_builtins::classmethod::as_class_method_any(&attr) {
        return pyrust_builtins::classmethod::bind_wrapped_class_method(
            wrapped,
            module_type_singleton(),
        )
        .ok();
    }
    if let ValueKind::BuiltinFunction(fn_name) = attr.kind() {
        return Some(pyrust_builtins::super_bound_builtin::super_bound_builtin(
            fn_name.to_string(),
            target.clone(),
        ));
    }
    Some(attr)
}
