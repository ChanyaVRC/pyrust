// ─────────────────────────────────────────────────────────────────────────────
// Name resolution — the env-lookup rule (issue #452)
// ─────────────────────────────────────────────────────────────────────────────
//
// `Environment` (pyrust-core) carries TWO parallel stores for the same
// conceptual entity (a scope's bindings):
//
//   * `values: HashMap<String, Value>` — the name-keyed slow path. Holds
//     module/class-body bindings, closure/`nonlocal` cells, and any function
//     local the compiler chose NOT to put in a register.
//   * fastlocals (register file) — index-keyed fast path. NOT a field of
//     `Environment`; it lives in the active VM frame (`VmFrameView` /
//     `regs_ptr`). The compiler assigns each fastlocal a slot via
//     scope analysis (`local_index`). Most function locals live here.
//
// The compiler decides at compile time which store each name uses; the
// runtime never has to guess. The dispatch is keyed by the per-scope name
// sets on `Environment` (`global_names`, `nonlocal_names`, `local_names`)
// plus whether the env is the module/root env (`parent.is_none()`).
//
// THE RULE (single source of truth — keep this and `Interpreter::lookup_name`
// / `Interpreter::assign_name` in `runtime/namespaces/names.rs` in agreement):
//
//   1. Name in `global_names` (a `global x` declaration in scope):
//        read  -> `lookup_name_in_module`: module-env HashMap, then builtin
//                 exception classes.
//        write -> module-env HashMap (+ live globals dict / module fastlocal
//                 register mirror). See `assign_name`'s `is_global` arm.
//
//   2. Name in `nonlocal_names` (a `nonlocal x` declaration in scope):
//        read  -> `lookup_name_in_enclosing_local_env`: walk to the nearest
//                 ENCLOSING function scope that declares `x` local, read there.
//        write -> `env_assign_local` into that same enclosing env.
//
//   3. Otherwise (ordinary local / free-variable read):
//        read  -> `lookup_name_in_env`: this env's HashMap, raising
//                 `UnboundLocalError` if `x` is a declared local of THIS scope
//                 but currently unset, otherwise recursing into parents
//                 (free-variable capture), bottoming out at the module env.
//        write -> `env_assign_local` into the current env (module-scope writes
//                 also mirror into the globals dict / bump the LoadGlobal
//                 inline-cache version).
//
// The fastlocal register path is orthogonal: when a name HAS a fastlocal slot
// in the active frame, the compiler addresses it directly as a register
// operand (`Insn::Move`, `Insn::BinOp`, … with the slot index) and never
// reaches these helpers at all. These helpers are the env-HashMap (slow) side
// of the duality; the name-keyed opcodes `Insn::LoadGlobal` / `Insn::StoreGlobal`
// fall through to them only for names without a register slot.
//
// Parent-chain scanning for rule 2 shares one body,
// `find_function_scope_with_local` (below). The `nonlocal` READ path
// (`lookup_name_in_enclosing_local_env`) skips the current env
// (`include_self == false`), while the `nonlocal` binding-existence CHECK
// performed at function-definition time (`has_local_binding_in_current_or_ancestor`,
// rejecting `nonlocal x` with no enclosing binding) includes it
// (`include_self == true`), since the defining env may itself be the binding scope.
//
// #384 (class body cannot resolve module-scope names) would integrate here:
// a class-body env currently follows rule 3 and bottoms out at the module env,
// but class bodies should resolve FREE names directly against module scope
// (skipping intervening function locals). That fix belongs in
// `lookup_name_in_env`'s parent-walk / a class-body-specific arm; it is OUT OF
// SCOPE for #452 and intentionally not changed here.
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
        .values
        .get(name)
        .cloned()
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
        .is_some_and(|view| view.kind == FrameKind::Script);
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

/// Walk the env parent chain for the first **function scope**
/// (`parent.is_some()`, i.e. not the module/root env) that declares `name`
/// as one of its locals (`local_names.contains(name)`), and return that env.
///
/// `include_self` controls the start point: `true` begins the scan at `env`
/// itself, `false` begins at `env`'s parent.  The module/root env can never
/// match because it has no parent — module-scope names live in the module-env
/// HashMap and are reached via [`lookup_name_in_module`], not this walk.
///
/// This is the single shared scan body for both the `nonlocal`-resolution
/// READ path ([`find_enclosing_local_env_for_name`], `include_self == false`,
/// skips the current env to find the enclosing binding scope) and the
/// `nonlocal` binding-existence CHECK at function-definition time
/// ([`has_local_binding_in_current_or_ancestor`], `include_self == true`,
/// since the defining env may itself declare the binding).
/// The two callers differ only in start point and return shape; the matching
/// predicate (`parent.is_some() && local_names.contains(name)`) is identical.
fn find_function_scope_with_local(env: &EnvRef, name: &str, include_self: bool) -> Option<EnvRef> {
    let mut current = if include_self {
        Some(Rc::clone(env))
    } else {
        env.borrow().parent.clone()
    };
    while let Some(candidate) = current {
        let (is_function_scope, has_name, next) = {
            let borrowed = candidate.borrow();
            (
                borrowed.parent.is_some(),
                borrowed.local_names.contains(name),
                borrowed.parent.clone(),
            )
        };
        if is_function_scope && has_name {
            return Some(candidate);
        }
        current = next;
    }
    None
}

fn has_local_binding_in_current_or_ancestor(env: &EnvRef, name: &str) -> bool {
    find_function_scope_with_local(env, name, true).is_some()
}

/// Resolve `name` to its captured value in the **non-module** portion of the
/// `env` chain (issue #2106).  Walks from `env` (the function's captured
/// enclosing scope) outward, returning the first `values` entry for `name`
/// found in a function scope (`parent.is_some()`); the module/root env is never
/// consulted, so a true module global returns `None` and is not reported as a
/// closure free variable.  Used by `closure_free_vars` to build `__closure__`
/// cells and `co_freevars`.
pub(crate) fn lookup_enclosing_function_value(env: &EnvRef, name: &str) -> Option<Value> {
    let mut current = Some(Rc::clone(env));
    while let Some(candidate) = current {
        let (is_function_scope, value, next) = {
            let borrowed = candidate.borrow();
            (
                borrowed.parent.is_some(),
                borrowed.values.get(name).cloned(),
                borrowed.parent.clone(),
            )
        };
        if is_function_scope
            && let Some(v) = value
            && !v.is_unset()
        {
            return Some(v);
        }
        current = next;
    }
    None
}

fn find_enclosing_local_env_for_name(env: &EnvRef, name: &str) -> Option<EnvRef> {
    find_function_scope_with_local(env, name, false)
}

fn lookup_name_in_enclosing_local_env(env: &EnvRef, name: &str) -> Result<Option<Value>> {
    let Some(target_env) = find_enclosing_local_env_for_name(env, name) else {
        return Err(PyError::Runtime(format!(
            "no binding for nonlocal '{}' found",
            name
        )));
    };
    // `target_env` is an *enclosing* function scope (the `nonlocal` binding
    // site), never the reading function's own env — so an unbound binding here
    // is a captured free variable, not a local.  CPython 3.12 raises `NameError`
    // ("cannot access free variable ... in enclosing scope") for this case
    // (issue #2340).
    lookup_name_in_env_as_free(&target_env, name)
}

// Write `value` into `env` for `name`.
#[inline]
fn env_assign_local(env: &EnvRef, name: &str, value: Value) {
    let mut environment = env.borrow_mut();
    if environment.parent.is_none() {
        environment.record_namespace_env_binding(name);
    }
    environment.values.insert(name, value);
}

fn lookup_name_in_env(env: &EnvRef, name: &str) -> Result<Option<Value>> {
    lookup_name_in_env_impl(env, name, false)
}

/// Like [`lookup_name_in_env`] but reports an unbound binding as a captured
/// **free variable** (`NameError`) rather than a local (`UnboundLocalError`).
/// Used for the `nonlocal` read path, where the resolved env is always an
/// enclosing scope.
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
    let value = borrowed.values.get(name).cloned();
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
fn unbound_binding_error(name: &str, as_free: bool) -> PyError {
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
