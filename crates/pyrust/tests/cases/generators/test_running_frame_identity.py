# A *running* frame object can still say what it is (issue #2978).
#
# Asking a generator / coroutine / async generator for its identity from inside
# its own body — `type(g)`, `isinstance`, `repr`, `__class__`, `__name__`,
# `gi_running` — used to abort the process, because the classifier read the
# execution state that the resume path has checked out for the whole of the
# body.  Those facts are object-model facts, not execution state, so they are
# answered without touching the frame at all.

import asyncio
import operator


# --- 1. The issue's repro ---------------------------------------------------


def g(cb):
    yield cb()


gen = g(lambda: type(gen).__name__)
print(next(gen))


# --- 2. The full identity surface of a running generator --------------------


def identity(obj, prefix):
    return [
        type(obj).__name__,
        obj.__class__.__name__,
        isinstance(obj, type(obj)),
        repr(obj).split(" at 0x")[0],
        obj.__name__,
        obj.__qualname__,
        getattr(obj, prefix + "_running"),
        getattr(obj, prefix + "_yieldfrom" if prefix == "gi" else prefix + "_await"),
    ]


def running_generator():
    yield identity(self_gen, "gi")


self_gen = running_generator()
print(next(self_gen))
print(self_gen.gi_running)


# The gen-drive trampoline parks the frame elsewhere while a `for` drives it;
# the same answers must come back on that path.
def looped():
    yield identity(loop_gen, "gi")


loop_gen = looped()
for row in loop_gen:
    print(row)
    break


# A genexpr is a generator too, and its qualname is the synthetic one.
def genexpr_probe():
    return type(ge).__name__, ge.gi_running


ge = (genexpr_probe() for _ in [1])
print(next(ge))


# --- 3. Coroutines and async generators -------------------------------------


async def coro():
    return identity(the_coro, "cr")


the_coro = coro()
print(asyncio.run(the_coro))


async def agen():
    yield identity(the_agen, "ag")


the_agen = agen()


async def drive_agen():
    return await the_agen.__anext__()


print(asyncio.run(drive_agen()))


# --- 4. The protocol surface is picked by kind, not by reading the frame -----
#
# A running coroutine must still advertise the coroutine surface (`cr_*`,
# `send`/`throw`/`close`) and not the plain-generator one.


async def surface():
    # Which introspection prefix is advertised, not the exact member list —
    # CPython grows members like `cr_origin` / `cr_suspended` between versions.
    names = dir(the_surface)
    return [
        any(n.startswith("cr_") for n in names),
        any(n.startswith("gi_") for n in names),
        "send" in names,
        "__next__" in names,
        hasattr(the_surface, "cr_running"),
        hasattr(the_surface, "gi_running"),
    ]


the_surface = surface()
print(asyncio.run(the_surface))


async def bad_attr():
    try:
        the_bad.nope
    except AttributeError as e:
        return str(e)
    return "no-error"


the_bad = bad_attr()
print(asyncio.run(the_bad))


# --- 5. The writable name pair is writable while running --------------------


def renames_itself():
    renamed.__name__ = "renamed-name"
    renamed.__qualname__ = "renamed-qualname"
    yield (renamed.__name__, renamed.__qualname__, repr(renamed).split(" at 0x")[0])


renamed = renames_itself()
print(next(renamed))
print(renamed.__name__, renamed.__qualname__)

try:
    renamed.__name__ = 42
except TypeError as e:
    print(e)


# Reassigning the Python-visible name leaves the *code object's* name alone —
# `co_name` is fixed at compile time, and the traceback keeps reporting it.
def named():
    yield 1


suspended = named()
next(suspended)
suspended.__name__ = "not-co-name"
print(suspended.__name__, suspended.gi_frame.f_code.co_name)


# --- 6. Re-entrancy still reports itself as re-entrancy ----------------------
#
# Identity questions are answerable mid-run; *resuming* the same frame is not.


def reenters():
    try:
        next(reentrant)
    except ValueError as e:
        yield (type(reentrant).__name__, type(e).__name__, str(e))


reentrant = reenters()
print(next(reentrant))


# --- 7. Built-in iterators are unaffected -----------------------------------
#
# They carry no frame kind, so they are still classified from their concrete
# cursor state.
for obj in (
    map(str, [1]),
    filter(None, [1]),
    zip([1], [2]),
    enumerate([1]),
    iter(range(3)),
    reversed([1, 2]),
    iter([1, 2]),
    iter({"a": 1}),
):
    print(type(obj).__name__, isinstance(obj, type(obj)))


# --- 8. Slots a frame does not have are absent, not busy --------------------
#
# `__length_hint__` is a built-in-iterator slot; a frame has none, so
# `operator.length_hint` returns its default rather than reporting the frame's
# run state.  Probing the checked-out cell instead answered with a re-entrancy
# error that CPython never raises here.


def hints():
    yield (operator.length_hint(hinted, 7), hasattr(hinted, "__length_hint__"))


hinted = hints()
print(next(hinted))


# Materialising a coroutine or an async generator is a type error under the
# object's own noun — the running frame is refused for what it is, not for
# being busy.
def materialize(obj):
    out = []
    for fn in (list, tuple):
        try:
            fn(obj)
            out.append("materialized")
        except TypeError as e:
            out.append(str(e))
    return out


async def coro_materializes_itself():
    return (operator.length_hint(the_coro, 7), materialize(the_coro))


the_coro = coro_materializes_itself()
print(asyncio.run(the_coro))


async def agen_materializes_itself():
    yield (operator.length_hint(the_agen, 7), materialize(the_agen))


the_agen = agen_materializes_itself()


async def drive_materializer():
    return await the_agen.__anext__()


print(asyncio.run(drive_materializer()))


# The same nouns off the running path, where the frame is merely suspended.
async def suspended_coro():
    return 1


sc = suspended_coro()
print(materialize(sc))
asyncio.run(sc)


async def suspended_agen():
    yield 1


print(materialize(suspended_agen()))

print("ok")
