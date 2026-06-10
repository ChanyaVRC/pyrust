# A coroutine object is awaitable, not iterable (issue #2314).  CPython
# exposes send/throw/close but NOT __iter__/__next__; iter()/next() raise
# TypeError.  Plain generators (iterable) and async generators (async
# protocol) are unaffected.
#
# Every coroutine created here is explicitly .close()d so CPython does not
# emit a "coroutine was never awaited" RuntimeWarning that would diverge.


async def coro():
    return 42


# --- coroutine: send/throw/close yes, __iter__/__next__ no ---
c = coro()
print("coro __iter__:", hasattr(c, "__iter__"))
print("coro __next__:", hasattr(c, "__next__"))
print("coro send:", hasattr(c, "send"))
print("coro throw:", hasattr(c, "throw"))
print("coro close:", hasattr(c, "close"))

try:
    iter(c)
except TypeError as e:
    print("iter(coro):", e)

try:
    next(c)
except TypeError as e:
    print("next(coro):", e)

try:
    for _ in c:
        pass
except TypeError as e:
    print("for in coro:", e)

c.close()


# --- plain generator: iterable, all five methods ---
def gen():
    yield 1


g = gen()
print("gen __iter__:", hasattr(g, "__iter__"))
print("gen __next__:", hasattr(g, "__next__"))
print("gen send:", hasattr(g, "send"))
print("gen first:", next(g))
g.close()


# --- async generator: async protocol, no sync iter ---
async def agen():
    yield 1


a = agen()
print("agen __iter__:", hasattr(a, "__iter__"))
print("agen __next__:", hasattr(a, "__next__"))
print("agen __aiter__:", hasattr(a, "__aiter__"))
print("agen __anext__:", hasattr(a, "__anext__"))
print("agen asend:", hasattr(a, "asend"))
print("agen send:", hasattr(a, "send"))
