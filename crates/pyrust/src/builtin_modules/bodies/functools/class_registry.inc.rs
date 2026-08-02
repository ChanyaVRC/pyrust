// Imported-generation ownership and internal instance construction.

const INTERNAL_CLASS_NAMES: [&str; 5] = [
    "_lru_cache_wrapper",
    "_lru_cache_factory",
    "_wraps_partial",
    "_cmp_to_key",
    "_cmp_key",
];

const GENERATION_BOUND_CALLABLES: [&str; 5] = [
    "lru_cache",
    "cache",
    "wraps",
    "cmp_to_key",
    "singledispatch",
];

/// State retained by the public factory callables from one `functools` import.
///
/// The weak module reference allows live module attribute replacements to be
/// observed without forming a `module -> callable -> module` reference cycle.
/// The fallback values keep a retained callable useful after its module was
/// removed and dropped.
struct FunctoolsGeneration {
    module: Weak<RefCell<PyModule>>,
    module_state: ModuleMutationState,
    fallback: IndexMap<String, Value>,
    cache_info_class: Option<Value>,
    lru_classes: Option<ResolvedLruClasses>,
    /// `(update_wrapper identity, compiled factory)` for this generation.
    singledispatch_factory: Option<(Value, Value)>,
}

/// Concrete implementation classes visible in one `functools` generation.
///
/// The mutation state makes the cached resolution observationally equivalent
/// to looking in the module dictionary on every call: replacing one of the
/// private implementation classes invalidates the snapshot. A retained
/// decorator from a reloaded/dropped module still owns its exact old snapshot.
#[derive(Clone)]
struct ResolvedLruClasses {
    module_state: ModuleMutationState,
    module_version: u64,
    wrapper: Rc<RefCell<PyClass>>,
    factory: Rc<RefCell<PyClass>>,
    cache_info: Value,
}

impl ResolvedLruClasses {
    #[inline]
    fn is_current(&self) -> bool {
        self.module_state.matches_cache_version(self.module_version)
    }
}

struct FunctoolsGenerationOps;
const FUNCTOOLS_GENERATION_OPS: &FunctoolsGenerationOps = &FunctoolsGenerationOps;

impl BuiltinTypeOps for FunctoolsGenerationOps {
    fn type_name(&self) -> &'static str {
        "_functools_generation"
    }
}

/// Tag the concrete classes exported by one imported module generation.
///
/// Construction does not use a process/thread-wide "newest class" registry.
/// Public factories receive an opaque generation owner, while callable
/// instances retain the exact sibling class they need.
pub(crate) fn prepare_module_classes(module: &Value) {
    let ValueKind::PyModule(module) = module.kind() else {
        return;
    };
    for (name, value) in &module.borrow().attrs {
        if let ValueKind::PyClass(class) = value.kind() {
            let mut class = class.borrow_mut();
            class
                .attrs
                .insert("__module__".to_string(), Value::string("functools"));
            if name == "partial" {
                class.error_name = Some("functools.partial");
            }
        }
    }
}

fn generation_state(generation: &Value) -> Result<&pyrust_core::BuiltinState> {
    let ValueKind::BuiltinObject { ops, state } = generation.kind() else {
        return Err(internal("generation"));
    };
    if !pyrust_core::builtin_ops_is::<FunctoolsGenerationOps>(ops) {
        return Err(internal("generation"));
    }
    Ok(state)
}

/// Read a generation-owned value, preferring a still-live module's current
/// attribute so deliberate monkey-patching has normal module-global semantics.
fn generation_member(generation: &Value, name: &str) -> Result<Value> {
    let state = generation_state(generation)?;
    let module = {
        let borrow = state.borrow();
        borrow
            .downcast_ref::<FunctoolsGeneration>()
            .and_then(|generation| generation.module.upgrade())
    };
    if let Some(module) = module {
        return module
            .borrow()
            .attrs
            .get(name)
            .cloned()
            .ok_or_else(|| internal(name));
    }
    let borrow = state.borrow();
    borrow
        .downcast_ref::<FunctoolsGeneration>()
        .and_then(|generation| generation.fallback.get(name).cloned())
        .ok_or_else(|| internal(name))
}

fn split_generation_arg<'a>(
    args: &'a [ExpandedCallArg],
    fn_name: &str,
) -> Result<(&'a Value, &'a [ExpandedCallArg])> {
    let Some(first) = args.first() else {
        return Err(internal(fn_name));
    };
    generation_state(&first.value)?;
    Ok((&first.value, &args[1..]))
}

fn generation_class(generation: &Value, name: &str) -> Result<Rc<RefCell<PyClass>>> {
    match generation_member(generation, name)?.kind() {
        ValueKind::PyClass(class) => Ok(Rc::clone(class)),
        _ => Err(internal(name)),
    }
}

fn class_value(value: Value, name: &str) -> Result<Rc<RefCell<PyClass>>> {
    match value.kind() {
        ValueKind::PyClass(class) => Ok(Rc::clone(class)),
        _ => Err(internal(name)),
    }
}

/// Resolve all classes needed by one LRU construction in one generation.
///
/// The steady state validates a typed snapshot without hashing Python names.
/// The slow path deliberately reads the live module so replacing private
/// implementation classes keeps normal Python module-global semantics.
fn with_lru_classes<R>(
    interp: &mut Interpreter,
    generation: &Value,
    use_classes: impl FnOnce(&ResolvedLruClasses) -> R,
) -> Result<R> {
    let state = generation_state(generation)?;
    let (module, module_state, fallback, retained_cache_info) = {
        let borrow = state.borrow();
        let generation = borrow
            .downcast_ref::<FunctoolsGeneration>()
            .ok_or_else(|| internal("lru_cache"))?;
        if let Some(classes) = &generation.lru_classes
            && classes.is_current()
        {
            return Ok(use_classes(classes));
        }
        (
            generation.module.upgrade(),
            generation.module_state.clone(),
            generation.fallback.clone(),
            generation.cache_info_class.clone(),
        )
    };

    let member = |name: &str| {
        module
            .as_ref()
            .and_then(|module| module.borrow().attrs.get(name).cloned())
            .or_else(|| fallback.get(name).cloned())
            .ok_or_else(|| internal(name))
    };
    let wrapper = class_value(member("_lru_cache_wrapper")?, "_lru_cache_wrapper")?;
    let factory = class_value(member("_lru_cache_factory")?, "_lru_cache_factory")?;
    let cache_info = if let Some(class) = module
        .as_ref()
        .and_then(|module| module.borrow().attrs.get("_CacheInfo").cloned())
    {
        class
    } else if let Some(class) = retained_cache_info {
        class
    } else {
        let class = build_cache_info_class(interp)?;
        {
            let mut borrow = state.borrow_mut();
            let generation = borrow
                .downcast_mut::<FunctoolsGeneration>()
                .ok_or_else(|| internal("cache_info"))?;
            generation.cache_info_class = Some(class.clone());
        }
        if let Some(module) = &module {
            module
                .borrow_mut()
                .insert_attr("_CacheInfo".to_string(), class.clone());
        }
        class
    };

    let module_version = module_state.cache_version().unwrap_or(u64::MAX);
    let classes = ResolvedLruClasses {
        module_version,
        module_state,
        wrapper,
        factory,
        cache_info,
    };
    let result = use_classes(&classes);
    let mut borrow = state.borrow_mut();
    let generation = borrow
        .downcast_mut::<FunctoolsGeneration>()
        .ok_or_else(|| internal("lru_cache"))?;
    if module_version != u64::MAX {
        generation.lru_classes = Some(classes.clone());
    } else {
        generation.lru_classes = None;
    }
    Ok(result)
}

#[inline]
fn lru_factory_class(interp: &mut Interpreter, generation: &Value) -> Result<Rc<RefCell<PyClass>>> {
    with_lru_classes(interp, generation, |classes| Rc::clone(&classes.factory))
}

#[inline]
fn lru_wrapper_classes(
    interp: &mut Interpreter,
    generation: &Value,
) -> Result<(Rc<RefCell<PyClass>>, Value)> {
    with_lru_classes(interp, generation, |classes| {
        (Rc::clone(&classes.wrapper), classes.cache_info.clone())
    })
}

/// Construct an internal instance from one exact imported generation.
fn make_instance(generation: &Value, name: &str, attrs: InstanceAttrs) -> Result<Value> {
    let class = generation_class(generation, name)?;
    Ok(make_instance_with_class(class, attrs))
}

fn make_instance_with_class(class: Rc<RefCell<PyClass>>, attrs: InstanceAttrs) -> Value {
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

/// Install per-import dynamic classes/factories and replace only the public
/// factory functions with generation-bound native callables.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let _ = interp;

    let mut fallback = IndexMap::new();
    {
        let module = module.borrow();
        for &name in &INTERNAL_CLASS_NAMES {
            let value = module
                .attrs
                .get(name)
                .cloned()
                .ok_or_else(|| internal(name))?;
            fallback.insert(name.to_string(), value);
        }
        let update_wrapper = module
            .attrs
            .get("update_wrapper")
            .cloned()
            .ok_or_else(|| internal("update_wrapper"))?;
        fallback.insert("update_wrapper".to_string(), update_wrapper);
    }

    let state: Box<dyn Any> = Box::new(FunctoolsGeneration {
        module: Rc::downgrade(module),
        module_state: module.borrow().mutation_state(),
        fallback,
        cache_info_class: None,
        lru_classes: None,
        singledispatch_factory: None,
    });
    let generation = Value::builtin_object(FUNCTOOLS_GENERATION_OPS, state);

    let replacements = {
        let module = module.borrow();
        GENERATION_BOUND_CALLABLES
            .iter()
            .map(|&name| {
                let wrapped = module
                    .attrs
                    .get(name)
                    .cloned()
                    .ok_or_else(|| internal(name))?;
                Ok((
                    name.to_string(),
                    pyrust_builtins::native_builtin_callable::native_generation_builtin(
                        wrapped,
                        generation.clone(),
                        name,
                        "functools",
                    ),
                ))
            })
            .collect::<Result<Vec<_>>>()?
    };
    let mut module = module.borrow_mut();
    for (name, callable) in replacements {
        module.insert_attr(name, callable);
    }
    Ok(())
}
