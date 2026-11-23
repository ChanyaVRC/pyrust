# Residual generator, coroutine, async-generator, and native-iterator surface
# parity checks (issue #2981).

import asyncio
import collections
import collections.abc as abc
import copy
import types


# --- 1. Immutable compile-time co_qualname -----------------------------------


def top_gen():
    yield 1


async def top_coro():
    return 1


async def top_agen():
    yield 1


def plain_fn():
    return 1


class QualHost:
    def gen_method(self):
        yield 1

    async def coro_method(self):
        return 1

    async def agen_method(self):
        yield 1

    def make_genexpr(self):
        return (x for x in [1])

    def make_lambda(self):
        return lambda x: x


def outer_scope():
    def inner_gen():
        yield 1

    async def inner_coro():
        return 1

    ge = (x for x in [1])
    lam = lambda x: x  # noqa: E731
    return inner_gen(), inner_coro(), ge, lam


g = top_gen()
g.__name__ = "N_gen"
g.__qualname__ = "Q_gen"
print("co_qualname_gen:", g.__qualname__, g.gi_frame.f_code.co_qualname, g.gi_frame.f_code.co_name)

gm = QualHost().gen_method()
gm.__qualname__ = "Q_gm"
print("co_qualname_gm:", gm.__qualname__, gm.gi_frame.f_code.co_qualname)

c = top_coro()
c.__qualname__ = "Q_coro"
print("co_qualname_coro:", c.__qualname__, c.cr_frame.f_code.co_qualname)
c.close()

cm = QualHost().coro_method()
cm.__qualname__ = "Q_cm"
print("co_qualname_cm:", cm.__qualname__, cm.cr_frame.f_code.co_qualname)
cm.close()

ag = top_agen()
ag.__qualname__ = "Q_agen"
print("co_qualname_agen:", ag.__qualname__, ag.ag_frame.f_code.co_qualname)

agm = QualHost().agen_method()
agm.__qualname__ = "Q_agm"
print("co_qualname_agm:", agm.__qualname__, agm.ag_frame.f_code.co_qualname)

top_ge = (x for x in [1])
top_ge.__qualname__ = "Q_top_ge"
print("co_qualname_top_ge:", top_ge.__qualname__, top_ge.gi_frame.f_code.co_qualname)

nested_ge = ((y for y in [x]) for x in [1])
inner_ge = next(nested_ge)
print("co_qualname_nested_ge:", nested_ge.gi_frame.f_code.co_qualname, inner_ge.gi_frame.f_code.co_qualname)

m_ge = QualHost().make_genexpr()
print("co_qualname_method_ge:", m_ge.__qualname__, m_ge.gi_frame.f_code.co_qualname)
m_ge.__qualname__ = "Q_m_ge"
print("co_qualname_method_ge_after:", m_ge.__qualname__, m_ge.gi_frame.f_code.co_qualname)

ig, ic, oge, olam = outer_scope()
ig.__qualname__ = "Q_ig"
ic.__qualname__ = "Q_ic"
oge.__qualname__ = "Q_oge"
olam.__qualname__ = "Q_olam"
print("co_qualname_inner_gen:", ig.__qualname__, ig.gi_frame.f_code.co_qualname)
print("co_qualname_inner_coro:", ic.__qualname__, ic.cr_frame.f_code.co_qualname)
ic.close()
print("co_qualname_outer_ge:", oge.__qualname__, oge.gi_frame.f_code.co_qualname)
print("co_qualname_outer_lam:", olam.__qualname__, olam.__code__.co_qualname)

mlam = QualHost().make_lambda()
mlam.__qualname__ = "Q_mlam"
print("co_qualname_method_lam:", mlam.__qualname__, mlam.__code__.co_qualname)

plain_fn.__qualname__ = "Q_plain"
print("co_qualname_fn:", plain_fn.__qualname__, plain_fn.__code__.co_qualname)


# --- 2. collections.abc registration ----------------------------------------

ABCS = [
    abc.Iterable,
    abc.Iterator,
    abc.Generator,
    abc.Awaitable,
    abc.Coroutine,
    abc.AsyncIterable,
    abc.AsyncIterator,
    abc.AsyncGenerator,
]


def abc_profile(obj):
    return [isinstance(obj, a) for a in ABCS]


def abc_subclass_profile(cls):
    return [issubclass(cls, a) for a in ABCS]


g2 = top_gen()
c2 = top_coro()
ag2 = top_agen()
for label, obj in [
    ("gen", g2),
    ("coro", c2),
    ("agen", ag2),
    ("map", map(int, [1])),
    ("filter", filter(None, [1])),
    ("zip", zip([1])),
    ("enumerate", enumerate([1])),
    ("list_iter", iter([1])),
    ("range_iter", iter(range(3))),
    ("reversed", reversed([1, 2])),
]:
    print("abc_inst:", label, abc_profile(obj))
    print("abc_sub:", label, abc_subclass_profile(type(obj)))
c2.close()

for label, cls in [
    ("GeneratorType", types.GeneratorType),
    ("CoroutineType", types.CoroutineType),
    ("AsyncGeneratorType", types.AsyncGeneratorType),
]:
    print("abc_types:", label, abc_subclass_profile(cls))

print(
    "abc_hooks:",
    abc.Coroutine.__subclasshook__(types.CoroutineType),
    abc.Generator.__subclasshook__(types.CoroutineType),
    abc.AsyncGenerator.__subclasshook__(types.AsyncGeneratorType),
    abc.Generator.__subclasshook__(types.GeneratorType),
)


# Check ABC classification while running inside the body too.
def running_gen_abc():
    yield abc_profile(rg)


rg = running_gen_abc()
print("abc_running_gen:", next(rg))


async def running_coro_abc():
    return abc_profile(rc)


rc = running_coro_abc()
print("abc_running_coro:", asyncio.run(rc))


async def running_agen_abc():
    yield abc_profile(rag)


rag = running_agen_abc()


async def drive_rag():
    res = await rag.__anext__()
    await rag.aclose()
    return res


print("abc_running_agen:", asyncio.run(drive_rag()))


# --- 3. __reduce__ and __reduce_ex__ on gen / coro / agen -------------------


def probe_call(fn):
    try:
        fn()
        return "NO_ERROR"
    except Exception as e:
        return f"{type(e).__name__}: {e}"


for label, factory in [("gen", top_gen), ("coro", top_coro), ("agen", top_agen)]:
    obj = factory()
    d = dir(obj)
    print(
        "reduce_surface:",
        label,
        "__reduce__" in d,
        "__reduce_ex__" in d,
        hasattr(obj, "__reduce__"),
        hasattr(obj, "__reduce_ex__"),
    )
    red = getattr(obj, "__reduce__")
    red_ex = getattr(obj, "__reduce_ex__")
    print("reduce_direct:", label, probe_call(lambda: obj.__reduce__()))
    print("reduce_bound:", label, probe_call(lambda: red()))
    print("reduce_1arg:", label, probe_call(lambda: obj.__reduce__(1)))
    print("reduce_kw:", label, probe_call(lambda: obj.__reduce__(k=1)))
    print("reduce_bound_kw:", label, probe_call(lambda: red(k=1)))
    print("reduce_ex_4:", label, probe_call(lambda: obj.__reduce_ex__(4)))
    print("reduce_ex_bound:", label, probe_call(lambda: red_ex(4)))
    print("reduce_ex_0arg:", label, probe_call(lambda: obj.__reduce_ex__()))
    print("reduce_ex_2arg:", label, probe_call(lambda: obj.__reduce_ex__(4, 5)))
    print("reduce_ex_kw:", label, probe_call(lambda: obj.__reduce_ex__(protocol=4)))
    print("reduce_ex_bound_kw:", label, probe_call(lambda: red_ex(protocol=4)))
    print("reduce_ex_str:", label, probe_call(lambda: obj.__reduce_ex__("bad")))
    print("reduce_ex_overflow:", label, probe_call(lambda: obj.__reduce_ex__(1 << 100)))
    print("copy:", label, probe_call(lambda: copy.copy(obj)))
    print("deepcopy:", label, probe_call(lambda: copy.deepcopy(obj)))
    if label == "coro":
        obj.close()


# --- 4 & 6. Native iterator attr leak and dir() surface ---------------------

FORBIDDEN_ITER_ATTRS = (
    "gi_frame",
    "gi_running",
    "gi_code",
    "gi_yieldfrom",
    "gi_suspended",
    "send",
    "throw",
    "close",
    "__name__",
    "__qualname__",
)

NATIVE_ITERS = [
    ("map", map(int, [1])),
    ("filter", filter(None, [1])),
    ("zip", zip([1], [2])),
    ("enumerate", enumerate([1])),
    ("reversed", reversed([1, 2])),
    ("list_iter", iter([1])),
    ("tuple_iter", iter((1,))),
    ("str_iter", iter("ab")),
    ("dict_iter", iter({1: 2})),
    ("range_iter", iter(range(3))),
    ("set_iter", iter({1})),
    ("bytes_iter", iter(b"a")),
    ("bytearray_iter", iter(bytearray(b"a"))),
    ("deque_iter", iter(collections.deque([1]))),
]

for label, it in NATIVE_ITERS:
    d = dir(it)
    leaked = [name for name in FORBIDDEN_ITER_ATTRS if name in d]
    assert not leaked, f"{label} leaked in dir(): {leaked}"
    assert "__iter__" in d and "__next__" in d, f"{label} missing iter protocol in dir()"
    cls = type(it)
    tname = cls.__name__ if cls.__module__ == "builtins" else f"{cls.__module__}.{cls.__name__}"
    for attr in ("__name__", "__qualname__", "gi_frame", "gi_running", "send", "throw", "close"):
        expected = f"AttributeError: '{tname}' object has no attribute '{attr}'"
        got_get = probe_call(lambda it=it, attr=attr: getattr(it, attr))
        got_set = probe_call(lambda it=it, attr=attr: setattr(it, attr, "x"))
        got_del = probe_call(lambda it=it, attr=attr: delattr(it, attr))
        assert got_get == expected, f"{label} getattr({attr}): {got_get!r} != {expected!r}"
        assert got_set == expected, f"{label} setattr({attr}): {got_set!r} != {expected!r}"
        assert got_del == expected, f"{label} delattr({attr}): {got_del!r} != {expected!r}"
    # Direct CallMethod paths too.
    assert probe_call(lambda it=it: it.send(None)) == f"AttributeError: '{tname}' object has no attribute 'send'"
    assert probe_call(lambda it=it: it.throw(ValueError("x"))) == f"AttributeError: '{tname}' object has no attribute 'throw'"
    assert probe_call(lambda it=it: it.close()) == f"AttributeError: '{tname}' object has no attribute 'close'"
print("native_iter_surface: ok")


# --- 7. athrow() / aclose() re-entrancy error message nouns -----------------


async def test_reentry():
    async def ag_async():
        try:
            yield 0
            await asyncio.sleep(0)
            yield 1
        except ValueError:
            await asyncio.sleep(0)
            yield 2
        except GeneratorExit:
            await asyncio.sleep(0)
            return

    async def drive(aw):
        try:
            return await aw
        except StopAsyncIteration:
            return None

    for first in ("anext", "asend", "athrow", "aclose"):
        for second in ("anext", "asend", "athrow", "aclose"):
            a = ag_async()
            assert await a.__anext__() == 0
            if first == "anext":
                t = asyncio.create_task(drive(a.__anext__()))
            elif first == "asend":
                t = asyncio.create_task(drive(a.asend(None)))
            elif first == "athrow":
                t = asyncio.create_task(drive(a.athrow(ValueError("e"))))
            else:
                t = asyncio.create_task(drive(a.aclose()))
            await asyncio.sleep(0)
            try:
                if second == "anext":
                    await a.__anext__()
                elif second == "asend":
                    await a.asend(None)
                elif second == "athrow":
                    await a.athrow(ValueError("x"))
                else:
                    await a.aclose()
                print("reentry_noerr:", first, second)
            except RuntimeError as e:
                print("reentry:", first, second, str(e))
            await t
            await a.aclose()


asyncio.run(test_reentry())


# Synchronous re-entry from inside the running async generator body.
async def ag_sync_reentry():
    for second in ("anext", "asend", "athrow", "aclose"):
        if second == "anext":
            aw = ag_sync_ref.__anext__()
        elif second == "asend":
            aw = ag_sync_ref.asend(None)
        elif second == "athrow":
            aw = ag_sync_ref.athrow(ValueError("x"))
        else:
            aw = ag_sync_ref.aclose()
        try:
            aw.send(None)
            print("sync_reentry_noerr:", second)
        except RuntimeError as e:
            print("sync_reentry:", second, str(e))
    yield 1


ag_sync_ref = ag_sync_reentry()


async def drive_sync_reentry():
    assert await ag_sync_ref.__anext__() == 1
    await ag_sync_ref.aclose()


asyncio.run(drive_sync_reentry())
print("done")
