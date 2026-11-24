// ─────────────────────────────────────────────────────────────────────────────
// Name resolution — the env-lookup rule (issue #452)
// ─────────────────────────────────────────────────────────────────────────────
//
// Function frames no longer use `Environment` for locals, own cells, or
// captured `nonlocal` / free variables:
//
//   * Function fastlocals live in the active VM frame's register file
//     (`RegSlice` / `VmFrameView::regs_ptr`).
//   * Function cells and captured free variables live in the active VM frame's
//     indexed `CellSlot` array (`CellSlice` / `UserFunction::closure_cells` /
//     `GeneratorFrame::cells`) and are accessed in O(1) via `Insn::LoadCell`,
//     `Insn::StoreCell`, `Insn::DeleteCell`, and `Insn::LoadClassDeref`.
//   * `Environment` (specifically `EnvironmentStorage::Namespace(EnvValues)`)
//     is reserved for name-keyed namespaces: module globals, class-body
//     `__class__` / annotation scopes, PEP 695 type-parameter scopes, and
//     explicit `exec`/`eval` namespaces.
//
// THE RULE for namespace lookups (`Insn::LoadGlobal` / `StoreGlobal` /
// `DeleteName`, kept in sync with `runtime/namespaces/names.rs`):
//
//   1. Active frame declares `global x` (`active_frame_declares_global(name)`):
//        read  -> `lookup_name_in_module`: root module `Namespace`, then builtin
//                 exception classes / builtins provider.
//        write -> root module `Namespace` (+ live globals dict / module
//                 fastlocal mirror).
//
//   2. Otherwise (ordinary namespace read/write):
//        read  -> `lookup_name_in_env`: walk `Namespace`-backed `self.env`
//                 chain (class annotation provider -> type-param / class
//                 bindings -> module root).
//        write -> `env_assign_local` into `self.env` (mirroring into globals
//                 dict and bumping `LoadGlobal` caches at module root).
pub(crate) fn module_env(env: &EnvRef) -> EnvRef {
    let mut current = Rc::clone(env);
    loop {
        let parent = current.borrow().parent.clone();
        match parent {
            Some(parent) => current = parent,
            None => return current,
        }
    }
}

pub(crate) fn lookup_name_in_module(env: &EnvRef, name: &str) -> Option<Value> {
    module_env(env)
        .borrow()
        .try_namespace_values()
        .and_then(|values| values.get(name).cloned())
        .or_else(|| lookup_exc_class(name).map(Value::py_class))
}

/// Sync all module-env values into the root namespace's globals backing and
/// permanently disable its opcode caches. Called by `globals()` and `locals()`
/// at module scope so the returned live dict is fully up to date.
///
/// After this call, `assign_name` will also mirror every new assignment
/// into the dict, keeping it live for the rest of execution.
pub(crate) fn sync_module_env_to_globals_dict(interp: &mut Interpreter) {
    let explicit_globals = interp.explicit_globals_for_environment(&interp.env);
    let at_script_scope = interp
        .vm_frame_views
        .last()
        .is_some_and(|view| view.kind() == FrameKind::Script);
    if explicit_globals.is_some() && !at_script_scope {
        // A function created by explicit exec already has a live authoritative
        // globals dict. Copying its captured EnvValues snapshot back here would
        // overwrite mutations performed through that dict after exec returned.
        return;
    }
    if explicit_globals.is_none() {
        let me = module_env(&interp.env);
        let already_exposed = me.borrow().namespace_globals_exposed();
        let target = me.borrow().expose_namespace_globals();
        if already_exposed {
            return;
        }
        // One ordered walk of this root: env-only bindings (dunders, names
        // stored via StoreGlobal or assign_name) followed by every name the
        // live Script layouts declare, in register order. The dict this fills
        // is Python-visible, so its order is the module's binding order
        // (issue #2903).
        let pairs = me.borrow().namespace_materialization_snapshot();
        for (name, value) in pairs {
            let _ = target.dict_insert(PyKey::str_from(&name), value);
        }
        me.borrow().activate_namespace_globals_alias(&target);
        return;
    }
    // With exec(code, globals, locals), ordinary module-code bindings live in
    // locals while declared-global stores are already mirrored directly to
    // globals by assign_name.  Never copy the merged root EnvValues snapshot
    // into globals: it cannot distinguish those two providers.
    let target_namespace = interp
        .explicit_locals_for_environment(&interp.env)
        .expect("explicit globals registration also defines active locals");
    // Merge every live Script register file owned by this root. A nested exec
    // is registered after its outer frame, so its bound values win. Keeping
    // this snapshot in core also covers first exposure from a child
    // Interpreter executing an imported helper.
    let fastlocals = module_env(&interp.env)
        .borrow()
        .namespace_fastlocals_snapshot();
    for (name, value) in fastlocals {
        let _ = target_namespace.dict_insert(PyKey::str_from(&name), value);
    }
}

/// Resolve `name` to its value in a **non-module** enclosing `Namespace` env
/// (e.g. `__class__` in a class env or a PEP 695 type parameter in a
/// `TypeParamEnv`). Function-scope closure variables live in indexed `CellSlot`s
/// rather than `Environment`, so they do not pass through here.
pub(crate) fn lookup_enclosing_function_value(env: &EnvRef, name: &str) -> Option<Value> {
    let mut current = Some(Rc::clone(env));
    while let Some(candidate) = current {
        let (is_non_root, value, next) = {
            let borrowed = candidate.borrow();
            (
                borrowed.parent.is_some(),
                borrowed
                    .try_namespace_values()
                    .and_then(|values| values.get(name).cloned()),
                borrowed.parent.clone(),
            )
        };
        if is_non_root
            && let Some(v) = value
            && !v.is_unset()
        {
            return Some(v);
        }
        current = next;
    }
    None
}

// Write `value` into `env` for `name`.
#[inline]
fn env_assign_local(env: &EnvRef, name: &str, value: Value) {
    let mut environment = env.borrow_mut();
    if environment.parent.is_none() {
        environment.record_namespace_env_binding(name);
    }
    environment.namespace_values_mut().insert(name, value);
}

fn lookup_name_in_env(env: &EnvRef, name: &str) -> Result<Option<Value>> {
    lookup_name_in_env_impl(env, name, false)
}

/// Like [`lookup_name_in_env`] but reports an unbound binding as a captured
/// **free variable** (`NameError`) rather than a local (`UnboundLocalError`).
fn lookup_name_in_env_as_free(env: &EnvRef, name: &str) -> Result<Option<Value>> {
    lookup_name_in_env_impl(env, name, true)
}

/// Resolve `name` against the `env` chain.
///
/// `as_free` selects the CPython 3.12 error class for an unbound binding
/// (issue #2340):
///   * `false` → `UnboundLocalError` ("cannot access local variable '<name>'
///     where it is not associated with a value") — a plain local referenced
///     before assignment.
///   * `true`  → `NameError` ("cannot access free variable '<name>' where it is
///     not associated with a value in enclosing scope") — a captured free
///     variable whose cell was never bound (or was `del`-eted) in the enclosing
///     scope.
fn lookup_name_in_env_impl(env: &EnvRef, name: &str, as_free: bool) -> Result<Option<Value>> {
    let borrowed = env.borrow();
    let value = borrowed
        .try_namespace_values()
        .and_then(|values| values.get(name).cloned());
    let class_annotation_value = borrowed.class_annotation_binding(name);
    let is_local_name = borrowed.local_names.contains(name);
    let parent = borrowed.parent.clone();
    drop(borrowed);
    if class_annotation_value.is_some() {
        return Ok(class_annotation_value);
    }
    if value.is_some() {
        return Ok(value);
    }
    if is_local_name {
        return Err(unbound_binding_error(name, as_free));
    }
    match parent {
        Some(parent) => lookup_name_in_env_impl(&parent, name, as_free),
        None => Ok(None),
    }
}

/// Build the CPython 3.12 unbound-binding exception: `NameError` for a captured
/// free variable, `UnboundLocalError` for a plain local (issue #2340).
pub(crate) fn unbound_binding_error(name: &str, as_free: bool) -> PyError {
    if as_free {
        PyError::named(
            "NameError",
            format!(
                "cannot access free variable '{}' where it is not associated with a value in enclosing scope",
                name
            ),
        )
    } else {
        PyError::named(
            "UnboundLocalError",
            format!(
                "cannot access local variable '{}' where it is not associated with a value",
                name
            ),
        )
    }
}
