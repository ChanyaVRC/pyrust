# Companion to #2338: drive a multi-level `yield from` chain through the full
# generator protocol (send / throw / close) and confirm re-entrancy errors stay
# correct.  These paths go through `next()`/`send()` rather than the for-loop
# gen-drive trampoline, but the fix must not regress them.


def echo():
    while True:
        x = yield
        print("echo", x)


def mid_send():
    yield from echo()


def top_send():
    yield from mid_send()


g = top_send()
next(g)
g.send(1)
g.send(2)


# throw() forwarded down a chain; the leaf catches and resumes.
def leaf_throw():
    try:
        yield 1
        yield 2
    except ValueError as e:
        print("leaf caught", e)
        yield 99


def mid_throw():
    yield from leaf_throw()


def top_throw():
    yield from mid_throw()


g = top_throw()
print(next(g))
print(g.throw(ValueError("boom")))


# close() runs the leaf's finally through the chain.
def leaf_close():
    try:
        yield 1
        yield 2
    finally:
        print("leaf cleanup")


def mid_close():
    yield from leaf_close()


def top_close():
    yield from mid_close()


g = top_close()
print(next(g))
g.close()
print("closed")


# Re-entrancy: a generator delegating to itself raises ValueError
# ("generator already executing") rather than panicking.
def selfref():
    yield from g_self


g_self = selfref()
try:
    next(g_self)
except ValueError as e:
    print("reentry", e)
