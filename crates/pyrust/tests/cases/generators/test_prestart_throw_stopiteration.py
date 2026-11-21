import asyncio


def show(label, fn):
    try:
        res = fn()
        print(f"{label}: OK {res!r}")
    except BaseException as exc:
        print(f"{label}: EXC {type(exc).__name__} {exc.args!r}")


class MyStop(StopIteration):
    pass


def simple_gen():
    yield 1
    yield 2


def gen_with_try():
    try:
        yield 1
    except StopIteration as exc:
        yield ("caught_si", exc.args)
    except ValueError as exc:
        yield ("caught_ve", exc.args)
        raise StopIteration("from_handler")


# --- Pre-start generator: throw(StopIteration) raises directly and closes ---
g1 = simple_gen()
show("prestart throw(StopIteration)", lambda: g1.throw(StopIteration))
show("prestart g1 next after", lambda: next(g1))

g2 = simple_gen()
try:
    g2.throw(StopIteration("boom"))
except StopIteration as exc:
    print("prestart throw(StopIteration('boom')):", type(exc).__name__, exc.args, exc.value)
show("prestart g2 next after", lambda: next(g2))

g3 = simple_gen()
show("prestart throw(MyStop('sub'))", lambda: g3.throw(MyStop("sub")))
show("prestart g3 next after", lambda: next(g3))

g4 = gen_with_try()
show("prestart try-body throw(StopIteration('s'))", lambda: g4.throw(StopIteration("s")))
show("prestart g4 next after", lambda: next(g4))

g5 = gen_with_try()
show("prestart try-body throw(ValueError('v'))", lambda: g5.throw(ValueError("v")))
show("prestart g5 next after", lambda: next(g5))

# --- Already-exhausted generator ---
g_done = simple_gen()
print("exhaust:", list(g_done))
show("exhausted throw(StopIteration('done'))", lambda: g_done.throw(StopIteration("done")))

# --- Suspended generator: PEP 479 still applies when uncaught or raised in body ---
g_susp = simple_gen()
print("susp first:", next(g_susp))
try:
    g_susp.throw(StopIteration("susp_si"))
except RuntimeError as exc:
    print(
        "susp uncaught SI:",
        type(exc).__name__,
        exc.args,
        type(exc.__cause__).__name__,
        exc.__cause__.args,
        exc.__context__ is exc.__cause__,
        exc.__suppress_context__,
    )
show("susp next after", lambda: next(g_susp))

g_catch = gen_with_try()
print("catch first:", next(g_catch))
print("catch throw(SI):", g_catch.throw(StopIteration("caught")))
show("catch next after", lambda: next(g_catch))

g_reraise = gen_with_try()
print("reraise first:", next(g_reraise))
print("reraise throw(VE):", g_reraise.throw(ValueError("v")))
show("reraise next (raises SI in body)", lambda: next(g_reraise))


# --- Pre-start coroutine: throw(StopIteration) raises directly and closes ---
async def simple_coro():
    return 42


c1 = simple_coro()
show("coro prestart throw(SI)", lambda: c1.throw(StopIteration("csi")))
show("coro after prestart throw send", lambda: c1.send(None))
show("coro after prestart throw throw", lambda: c1.throw(ValueError("x")))

c2 = simple_coro()
show("coro prestart throw(MyStop)", lambda: c2.throw(MyStop("csub")))


# --- Pre-start async generator: athrow(StopIteration / StopAsyncIteration) ---
async def simple_agen():
    try:
        yield 1
    except BaseException as exc:
        yield ("ag_caught", type(exc).__name__, exc.args)


ag1 = simple_agen()
show("agen prestart athrow(SI).send(None)", lambda: ag1.athrow(StopIteration("agsi")).send(None))

ag2 = simple_agen()
show("agen prestart athrow(VE).send(None)", lambda: ag2.athrow(ValueError("agve")).send(None))


async def test_agen_athrow():
    ag3 = simple_agen()
    print("await agen prestart athrow(SI):", repr(await ag3.athrow(StopIteration("agsi2"))))
    try:
        await ag3.__anext__()
    except StopAsyncIteration:
        print("ag3 closed after prestart athrow(SI): True")

    ag4 = simple_agen()
    try:
        await ag4.athrow(StopAsyncIteration("agsai"))
    except StopAsyncIteration as exc:
        print("await agen prestart athrow(SAI):", type(exc).__name__, exc.args)


asyncio.run(test_agen_athrow())
