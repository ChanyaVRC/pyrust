// Per-import CacheInfo, singledispatch, singledispatchmethod, and partialmethod support.

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

fn build_descriptor_helpers(
    interp: &mut Interpreter,
    partial: Value,
    singledispatch: Value,
    update_wrapper: Value,
) -> Result<(Value, Value)> {
    let namespace = Value::dict(PyDict::default());
    namespace.dict_insert(PyKey::str_from("partial"), partial)?;
    namespace.dict_insert(PyKey::str_from("singledispatch"), singledispatch)?;
    namespace.dict_insert(PyKey::str_from("update_wrapper"), update_wrapper)?;
    interp.exec_source(DESCRIPTOR_HELPERS_SOURCE, Some(namespace.clone()), None)?;
    let dict = namespace.as_dict().ok_or_else(|| internal("functools"))?;
    let sdm = dict
        .get(&PyKey::str_from("singledispatchmethod"))
        .cloned()
        .ok_or_else(|| internal("singledispatchmethod"))?;
    let pm = dict
        .get(&PyKey::str_from("partialmethod"))
        .cloned()
        .ok_or_else(|| internal("partialmethod"))?;
    for val in [&sdm, &pm] {
        if let ValueKind::PyClass(class_ref) = val.kind() {
            class_ref
                .borrow_mut()
                .attrs
                .insert("__module__".to_string(), Value::string("functools"));
        }
    }
    Ok((sdm, pm))
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

/// Pure-Python `singledispatch`, faithfully supporting CPython 3.11+ UnionType
/// (`int | str`), `typing.Union[int, str]`, `None` annotations, descriptors
/// (`classmethod`/`staticmethod`), and `MappingProxyType` registry views.
const SINGLEDISPATCH_SOURCE: &str = "\
import types as _types

def _is_valid_dispatch_type(cls):
    return isinstance(cls, type)

def _is_union_type(cls):
    if isinstance(cls, _types.UnionType):
        return True
    origin = getattr(cls, '__origin__', None)
    return (
        origin is not None
        and getattr(origin, '__name__', None) == 'Union'
        and getattr(origin, '__module__', None) == 'typing'
    )

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

    def _is_valid_arg(cls):
        return _is_valid_dispatch_type(cls) or _is_union_type(cls)

    def register(cls, func=None):
        if _is_valid_arg(cls):
            if func is None:
                return lambda f: register(cls, f)
        else:
            if func is not None:
                raise TypeError(
                    f'Invalid first argument to `register()`. '
                    f'{cls!r} is not a class or union type.'
                )
            target = getattr(cls, '__func__', cls)
            ann = getattr(target, '__annotations__', None)
            if not ann:
                raise TypeError(
                    f'Invalid first argument to `register()`: {cls!r}. '
                    f'Use either `@register(some_class)` or plain `@register` '
                    f'on an annotated function.'
                )
            func = cls
            argname, cls = next(iter(ann.items()))
            if cls is None:
                cls = type(None)
            if not _is_valid_arg(cls):
                if _is_union_type(cls):
                    pass
                else:
                    raise TypeError(
                        f'Invalid annotation for {argname!r}. '
                        f'{cls!r} is not a class.'
                    )
        if _is_union_type(cls):
            args = getattr(cls, '__args__', ())
            for arg in args:
                if not _is_valid_dispatch_type(arg):
                    raise TypeError(
                        f'Invalid annotation in union: {arg!r} is not a class.'
                    )
            for arg in args:
                registry[arg] = func
        else:
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
    wrapper.registry = _types.MappingProxyType(registry)
    wrapper._clear_cache = dispatch_cache.clear
    _update_wrapper(wrapper, func)
    return wrapper
";

const DESCRIPTOR_HELPERS_SOURCE: &str = "\
import types as _types

def _bind_descriptor(func, obj, cls):
    get = getattr(func, '__get__', None)
    if get is not None:
        return get(obj, cls)
    if isinstance(func, _types.FunctionType) and obj is not None:
        return partial(func, obj)
    return func

class singledispatchmethod:
    '''Single-dispatch generic method descriptor (Python 3.8+/3.11+).'''

    def __init__(self, func):
        if not callable(func) and not hasattr(func, '__get__'):
            raise TypeError(f'{func!r} is not callable or a descriptor')
        self.dispatcher = singledispatch(func)
        self.func = func

    def register(self, cls, method=None):
        return self.dispatcher.register(cls, func=method)

    def __get__(self, obj, cls=None):
        def _method(*args, **kwargs):
            if not args:
                funcname = getattr(self.func, '__name__', 'singledispatchmethod method')
                raise TypeError(f'{funcname} requires at least 1 positional argument')
            method = self.dispatcher.dispatch(args[0].__class__)
            bound = _bind_descriptor(method, obj, cls)
            return bound(*args, **kwargs)

        _method.__isabstractmethod__ = self.__isabstractmethod__
        _method.register = self.register
        update_wrapper(_method, self.func)
        return _method

    @property
    def __isabstractmethod__(self):
        return getattr(self.func, '__isabstractmethod__', False)


class partialmethod:
    '''Method descriptor with partial application of the given arguments and keywords.'''

    def __init__(self, func, /, *args, **keywords):
        if not callable(func) and not hasattr(func, '__get__'):
            raise TypeError('the first argument must be callable')
        if isinstance(func, partialmethod):
            self.func = func.func
            self.args = func.args + args
            self.keywords = {**func.keywords, **keywords}
        else:
            self.func = func
            self.args = args
            self.keywords = keywords

    def __repr__(self):
        args = ', '.join(map(repr, self.args))
        keywords = ', '.join(f'{k}={v!r}' for k, v in self.keywords.items())
        format_string = '{module}.{cls}({func}, {args}, {keywords})'
        return format_string.format(
            module=self.__class__.__module__,
            cls=self.__class__.__qualname__,
            func=self.func,
            args=args,
            keywords=keywords,
        )

    def _make_unbound_method(self):
        def _method(cls_or_self, /, *args, **keywords):
            call_keywords = {**self.keywords, **keywords}
            return self.func(cls_or_self, *self.args, *args, **call_keywords)
        _method.__isabstractmethod__ = self.__isabstractmethod__
        _method._partialmethod = self
        return _method

    def __get__(self, obj, cls=None):
        get = getattr(self.func, '__get__', None)
        if get is not None:
            new_func = get(obj, cls)
            if new_func is not self.func:
                result = partial(new_func, *self.args, **self.keywords)
                try:
                    result.__self__ = new_func.__self__
                except AttributeError:
                    pass
                return result
        if isinstance(self.func, _types.FunctionType) and obj is not None:
            result = partial(self.func, obj, *self.args, **self.keywords)
            result.__self__ = obj
            return result
        unbound = self._make_unbound_method()
        if obj is not None:
            return partial(unbound, obj)
        return unbound

    @property
    def __isabstractmethod__(self):
        return getattr(self.func, '__isabstractmethod__', False)

    __class_getitem__ = classmethod(_types.GenericAlias)
";
