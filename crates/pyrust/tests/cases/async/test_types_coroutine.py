import types


def show_error(label, call):
    try:
        call()
    except Exception as exc:
        print(label, type(exc).__name__, str(exc))


# The decorator validates its input before doing any function inspection.
show_error("non-callable", lambda: types.coroutine(42))


async def native():
    return 5


native_decorated = types.coroutine(native)
print("native function identity", native_decorated is native)
native_result = native_decorated()
plain_native_result = native()
native_result.close()
plain_native_result.close()


def legacy():
    "legacy doc"
    if False:
        yield
    return 42


ordinary = legacy()
legacy_decorated = types.coroutine(legacy)
print("generator function identity", legacy_decorated is legacy)
print("metadata", legacy_decorated.__name__, legacy_decorated.__doc__)
print("iterable flag", bool(legacy.__code__.co_flags & 0x100))


async def await_legacy():
    return await legacy()


awaiting = await_legacy()
try:
    awaiting.send(None)
except StopIteration as exc:
    print("legacy awaited", exc.value)


@types.coroutine
def keyword_legacy(value):
    if False:
        yield
    return value


@types.coroutine
def variadic_legacy(*values, **kwargs):
    if False:
        yield
    return values[0] + kwargs["extra"]


async def await_call_paths():
    return (
        await keyword_legacy(value=20),
        await variadic_legacy(20, extra=22),
    )


call_paths = await_call_paths()
try:
    call_paths.send(None)
except StopIteration as exc:
    print("call paths", exc.value)


async def await_ordinary():
    return await ordinary


show_error("ordinary rejected", lambda: await_ordinary().send(None))
ordinary.close()


# Decorating one generator function must not make unrelated generators
# awaitable or mark their code objects.
def unrelated():
    if False:
        yield
    return 7


print("no flag leak", bool(unrelated.__code__.co_flags & 0x100))
unrelated_result = unrelated()


async def await_unrelated():
    return await unrelated_result


show_error("unrelated rejected", lambda: await_unrelated().send(None))
unrelated_result.close()


# Separate function objects produced from one nested definition share compiled
# code in PyRust. Marking one must still leave its sibling ordinary.
def make_shared():
    def shared():
        if False:
            yield
        return 8

    return shared


marked_shared = make_shared()
ordinary_shared = make_shared()
types.coroutine(marked_shared)
print(
    "shared code isolation",
    bool(marked_shared.__code__.co_flags & 0x100),
    bool(ordinary_shared.__code__.co_flags & 0x100),
)
ordinary_shared_result = ordinary_shared()


async def await_ordinary_shared():
    return await ordinary_shared_result


show_error("shared rejected", lambda: await_ordinary_shared().send(None))
ordinary_shared_result.close()


# The non-native path returns a metadata-preserving wrapper function. Native
# coroutine and already-marked generator results pass through unchanged.
native_products = []


def native_factory():
    result = native()
    native_products.append(result)
    return result


wrapped_native_factory = types.coroutine(native_factory)
native_from_wrapper = wrapped_native_factory()
print(
    "factory metadata",
    wrapped_native_factory.__name__,
    wrapped_native_factory.__wrapped__ is native_factory,
)
print(
    "native passthrough",
    native_from_wrapper is native_products[-1],
    type(native_from_wrapper) is types.CoroutineType,
)
native_from_wrapper.close()


marked_products = []


def marked_factory():
    result = legacy()
    marked_products.append(result)
    return result


wrapped_marked_factory = types.coroutine(marked_factory)
marked_result = wrapped_marked_factory()
print(
    "marked passthrough",
    marked_result is marked_products[-1],
    type(marked_result) is types.GeneratorType,
)
marked_result.close()


events = []


class GeneratorLike:
    def __init__(self):
        self.state = 0
        self.closed = False
        self.__name__ = "generator_like"
        self.__qualname__ = "GeneratorLike.generator_like"

    def __iter__(self):
        return self

    def __next__(self):
        return self.send(None)

    def send(self, value):
        events.append(("send", value))
        self.state += 1
        if self.state == 1:
            return "pause"
        raise StopIteration(99)

    def throw(self, typ, *rest):
        events.append(("throw", typ.__name__, rest))
        return "thrown"

    def close(self):
        self.closed = True
        events.append(("close",))
        return "closed"


protocol_products = []


def protocol_factory():
    "protocol doc"
    result = GeneratorLike()
    protocol_products.append(result)
    return result


wrapped_factory = types.coroutine(protocol_factory)
wrapper = wrapped_factory()
underlying = protocol_products[-1]
print("wrapper identity", wrapper is underlying, type(wrapper).__name__)
print("wrapper class identity", type(wrapper) is types._GeneratorWrapper)
print(
    "native marker hidden",
    hasattr(types, "_mark_iterable_coroutine"),
    hasattr(types, "_is_iterable_coroutine"),
    hasattr(types, "_is_generator_wrapper_candidate"),
)
print(
    "wrapper metadata",
    wrapped_factory.__name__,
    wrapped_factory.__doc__,
    wrapped_factory.__wrapped__ is protocol_factory,
)
print("wrapper object metadata", wrapper.__name__, wrapper.__qualname__)
print("await identity", wrapper.__await__() is wrapper)
print("iter identity", iter(wrapper) is wrapper)
print("send result", wrapper.send("value"))
print("throw result", wrapper.throw(ValueError, "detail"))
print("close result", wrapper.close(), underlying.closed)
print("events", events)


events.clear()


async def await_protocol_wrapper():
    return await wrapped_factory()


driven_wrapper = await_protocol_wrapper()
print("wrapper await yielded", driven_wrapper.send(None))
try:
    driven_wrapper.send(None)
except StopIteration as exc:
    print("wrapper await returned", exc.value)
print("wrapper await events", events)


# A real generator is returned from wrapper.__iter__/__await__, while the
# wrapper object remains distinct from the generator-protocol object.
def raw_generator_factory():
    return (x for x in (1, 2))


wrapped_raw_factory = types.coroutine(raw_generator_factory)
raw = wrapped_raw_factory()
raw_await = raw.__await__()
print(
    "raw wrapper",
    type(raw).__name__,
    raw_await is raw,
    type(raw_await) is types.GeneratorType,
)
raw.close()


# PyRust stores several native iterator/coroutine families behind one internal
# value variant.  The wrapper selection must still follow their distinct
# Python types: async generators and ordinary built-in iterators are not
# collections.abc.Generator instances in CPython and pass through unchanged.
async def async_generator():
    yield 1


async_generator_product = async_generator()
wrapped_async_generator_factory = types.coroutine(lambda: async_generator_product)
async_generator_result = wrapped_async_generator_factory()
print(
    "async generator passthrough",
    async_generator_result is async_generator_product,
    type(async_generator_result) is types.AsyncGeneratorType,
)


for label, product in (
    ("list iterator", iter([1, 2])),
    ("map iterator", map(lambda value: value, [1, 2])),
    ("enumerate iterator", enumerate([1, 2])),
):
    wrapped_iterator_factory = types.coroutine(lambda product=product: product)
    iterator_result = wrapped_iterator_factory()
    print(label, iterator_result is product, type(iterator_result).__name__)


# Objects outside the generator protocol are passed through. Await performs
# the stable validation and reports the result's concrete type.
def invalid_factory():
    return 17


wrapped_invalid = types.coroutine(invalid_factory)
print("invalid passthrough", wrapped_invalid())


async def await_invalid():
    return await wrapped_invalid()


show_error("invalid await", lambda: await_invalid().send(None))
