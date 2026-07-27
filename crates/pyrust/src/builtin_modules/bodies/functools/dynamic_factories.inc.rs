// Per-import CacheInfo and singledispatch support.

fn build_cache_info_class(interp: &mut Interpreter) -> Result<Value> {
    let namespace = Value::dict(PyDict::default());
    interp.exec_source(CACHE_INFO_SOURCE, Some(namespace.clone()), None)?;
    let class = namespace
        .as_dict()
        .and_then(|dict| dict.get(&PyKey::str_from("CacheInfo")).cloned())
        .ok_or_else(|| internal("cache_info"))?;
    if let ValueKind::PyClass(class_ref) = class.kind() {
        class_ref
            .borrow_mut()
            .attrs
            .insert("__module__".to_string(), Value::string("functools"));
    }
    Ok(class)
}

const CACHE_INFO_SOURCE: &str = "\
class CacheInfo(tuple):
    __slots__ = ()
    _fields = ('hits', 'misses', 'maxsize', 'currsize')
    def __new__(cls, hits, misses, maxsize, currsize):
        return tuple.__new__(cls, (hits, misses, maxsize, currsize))
    @classmethod
    def _make(cls, iterable):
        return cls(*iterable)
    def _asdict(self):
        return {f: self[i] for i, f in enumerate(self._fields)}
    def _replace(self, **kwds):
        vals = list(self)
        for i, f in enumerate(self._fields):
            if f in kwds:
                vals[i] = kwds.pop(f)
        if kwds:
            raise ValueError(f'Got unexpected field names: {list(kwds)!r}')
        return self.__class__(*vals)
    @property
    def hits(self):
        return self[0]
    @property
    def misses(self):
        return self[1]
    @property
    def maxsize(self):
        return self[2]
    @property
    def currsize(self):
        return self[3]
    def __repr__(self):
        return f'CacheInfo(hits={self[0]}, misses={self[1]}, maxsize={self[2]}, currsize={self[3]})'
";

fn build_singledispatch_factory(interp: &mut Interpreter, update_wrapper: Value) -> Result<Value> {
    let namespace = Value::dict(PyDict::default());
    namespace.dict_insert(PyKey::str_from("_update_wrapper"), update_wrapper)?;
    interp.exec_source(SINGLEDISPATCH_SOURCE, Some(namespace.clone()), None)?;
    namespace
        .as_dict()
        .and_then(|dict| dict.get(&PyKey::str_from("singledispatch")).cloned())
        .ok_or_else(|| internal("singledispatch"))
}

/// Resolve the pure-Python factory owned by one import generation.
///
/// `update_wrapper` is a real module global in CPython, so recompile only when
/// that attribute's identity changes. The cache is held by the generation
/// context rather than a thread-global singleton.
fn singledispatch_factory(interp: &mut Interpreter, generation: &Value) -> Result<Value> {
    let update_wrapper = generation_member(generation, "update_wrapper")?;
    let state = generation_state(generation)?;
    {
        let borrow = state.borrow();
        let generation = borrow
            .downcast_ref::<FunctoolsGeneration>()
            .ok_or_else(|| internal("singledispatch"))?;
        if let Some((cached_update_wrapper, factory)) = &generation.singledispatch_factory
            && cached_update_wrapper == &update_wrapper
        {
            return Ok(factory.clone());
        }
    }

    let factory = build_singledispatch_factory(interp, update_wrapper.clone())?;
    let mut borrow = state.borrow_mut();
    let generation = borrow
        .downcast_mut::<FunctoolsGeneration>()
        .ok_or_else(|| internal("singledispatch"))?;
    generation.singledispatch_factory = Some((update_wrapper, factory.clone()));
    Ok(factory)
}

/// Pure-Python `singledispatch`, transcribed from CPython's implementation.
const SINGLEDISPATCH_SOURCE: &str = "\
def singledispatch(func):
    registry = {}
    dispatch_cache = {}

    def dispatch(cls):
        try:
            return dispatch_cache[cls]
        except KeyError:
            pass
        try:
            impl = registry[cls]
        except KeyError:
            impl = registry[object]
            for t in cls.__mro__:
                if t in registry:
                    impl = registry[t]
                    break
        dispatch_cache[cls] = impl
        return impl

    def register(cls, func=None):
        if func is None:
            if isinstance(cls, type):
                return lambda f: register(cls, f)
            ann = getattr(cls, '__annotations__', {})
            func = cls
            argname, cls = next(iter(ann.items()))
            if not isinstance(cls, type):
                raise TypeError(
                    f'Invalid annotation for {argname!r}. '
                    f'{cls!r} is not a class.'
                )
        registry[cls] = func
        dispatch_cache.clear()
        return func

    def wrapper(*args, **kw):
        if not args:
            raise TypeError(
                f'{funcname} requires at least 1 positional argument'
            )
        return dispatch(args[0].__class__)(*args, **kw)

    funcname = getattr(func, '__name__', 'singledispatch function')
    registry[object] = func
    wrapper.register = register
    wrapper.dispatch = dispatch
    wrapper.registry = registry
    _update_wrapper(wrapper, func)
    return wrapper
";
