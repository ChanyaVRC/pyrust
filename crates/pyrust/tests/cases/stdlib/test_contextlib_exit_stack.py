"""contextlib.ExitStack / ContextDecorator / contextmanager surface (issue #3388)."""

import contextlib
from contextlib import (
    AbstractContextManager,
    ContextDecorator,
    ExitStack,
    closing,
    contextmanager,
    nullcontext,
    suppress,
)

# ── module surface ──────────────────────────────────────────────────────────
print(contextlib.__doc__)
print(contextlib.__all__)
for name in contextlib.__all__:
    print(name, hasattr(contextlib, name))
print(ExitStack.__mro__ == (ExitStack, contextlib._BaseExitStack, AbstractContextManager, __import__("abc").ABC, object))
print(contextlib.suppress.__mro__[1] is AbstractContextManager)
print(AbstractContextManager.__doc__)
print(ContextDecorator.__doc__)
print(suppress.__module__, ExitStack.__module__, contextmanager.__name__)

# ── ABC enforcement ─────────────────────────────────────────────────────────
class NoExit(AbstractContextManager):
    pass


try:
    NoExit()
except TypeError as exc:
    print("abstract:", exc)


class WithExit(AbstractContextManager):
    def __exit__(self, *exc):
        return None


w = WithExit()
print(w.__enter__() is w)
print(isinstance(ExitStack(), AbstractContextManager), isinstance(object(), AbstractContextManager))
print(issubclass(nullcontext, AbstractContextManager))

# ── ExitStack basics ────────────────────────────────────────────────────────
order = []


class CM:
    def __init__(self, name, suppress_exc=False):
        self.name = name
        self.suppress_exc = suppress_exc

    def __enter__(self):
        order.append("enter " + self.name)
        return self.name.upper()

    def __exit__(self, typ, exc, tb):
        order.append(("exit " + self.name, typ.__name__ if typ else None))
        return self.suppress_exc


with ExitStack() as stack:
    print(stack.enter_context(CM("a")), stack.enter_context(CM("b")))
    print(len(stack._exit_callbacks))
print(order)

# callback returns its argument and supports args/kwargs
order.clear()
with ExitStack() as stack:
    def cb(*a, **k):
        order.append((a, k))
        return "cb-result"

    print(stack.callback(cb, 1, 2, x=3) is cb)
    print(stack.callback(cb) is cb)
    stack._exit_callbacks[-1][1].__wrapped__ is cb
print(order)


# push with exit-callable and with CM object
def exit_fn(typ, exc, tb):
    order.append(("exit_fn", typ))
    return True


order.clear()
with ExitStack() as stack:
    print(stack.push(exit_fn) is exit_fn)
    stack.push(CM("pushed"))
    raise ValueError("boom")
print(order)

# suppression semantics and received_exc
s = ExitStack()
s.push(lambda *a: True)
print(s.__exit__(None, None, None))
s = ExitStack()
s.push(lambda *a: True)
print(s.__exit__(ValueError, ValueError("x"), None))
s = ExitStack()
s.push(lambda *a: False)
print(s.__exit__(ValueError, ValueError("x"), None))

# pop_all transfers callbacks
order.clear()
with ExitStack() as stack:
    stack.callback(order.append, "kept")
    moved = stack.pop_all()
    print(len(stack._exit_callbacks), len(moved._exit_callbacks), type(moved) is ExitStack)
print(order)
moved.close()
print(order)


# a subclass keeps its type through pop_all
class MyStack(ExitStack):
    pass


print(type(MyStack().pop_all()).__name__)

# close() and re-use
st = ExitStack()
st.callback(order.append, "closed")
st.close()
st.close()
print(order[-1])

# error: not a context manager
try:
    ExitStack().enter_context(object())
except TypeError as exc:
    print(exc)


class Broken:
    def __enter__(self):
        raise RuntimeError("enter failed")

    def __exit__(self, *a):
        order.append("never")


stack = ExitStack()
try:
    stack.enter_context(Broken())
except RuntimeError as exc:
    print(exc, len(stack._exit_callbacks))

# exception chaining inside callbacks
def raise_in_exit(label):
    def inner(typ, exc, tb):
        raise KeyError(label)

    return inner


try:
    with ExitStack() as stack:
        stack.push(raise_in_exit("outer"))
        stack.push(raise_in_exit("inner"))
        raise ValueError("body")
except KeyError as exc:
    chain = []
    cur = exc
    while cur is not None:
        chain.append((type(cur).__name__, cur.args))
        cur = cur.__context__
    print(chain)

# suppress inside an ExitStack
with ExitStack() as stack:
    stack.enter_context(suppress(ValueError))
    raise ValueError("swallowed")
print("suppressed by stack")

# exceptions in callbacks run all remaining callbacks
order.clear()
try:
    with ExitStack() as stack:
        stack.callback(order.append, "first")
        stack.callback(lambda: 1 / 0)
        stack.callback(order.append, "third")
except ZeroDivisionError:
    print("zde", order)

# ── ContextDecorator ────────────────────────────────────────────────────────
log = []


class Tracker(ContextDecorator):
    def __init__(self, tag):
        self.tag = tag

    def __enter__(self):
        log.append("enter " + self.tag)
        return self

    def __exit__(self, *exc):
        log.append("exit " + self.tag)
        return False


@Tracker("f")
def decorated(a, b=2, *, c=3):
    """decorated doc"""
    log.append(("body", a, b, c))
    return a + b + c


print(decorated(1, c=10), decorated.__name__, decorated.__doc__, decorated.__wrapped__.__name__)
print(log)


@Tracker("raises")
def failing():
    raise ValueError("inside")


try:
    failing()
except ValueError as exc:
    print("raised", exc, log[-1])


class Owner:
    @Tracker("method")
    def method(self, x):
        return (type(self).__name__, x)

    @classmethod
    @Tracker("classmethod")
    def cm(cls):
        return cls.__name__


print(Owner().method(5), Owner.cm())


class Suppressor(ContextDecorator):
    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return True


@Suppressor()
def quiet():
    raise ValueError("hidden")


print(quiet())

# ── @contextmanager ─────────────────────────────────────────────────────────
@contextmanager
def managed(x, *, y=0):
    """managed doc"""
    log.append(("setup", x, y))
    try:
        yield x + y
    finally:
        log.append("teardown")


print(managed.__name__, managed.__doc__, managed.__wrapped__.__name__, managed.__module__)
log.clear()
with managed(1, y=2) as value:
    print(value)
print(log)
cm = managed(5)
print(type(cm).__name__, cm.__doc__, isinstance(cm, ContextDecorator), isinstance(cm, AbstractContextManager))


# usable as a decorator (recreated for every call)
@managed(7)
def deco_target(n):
    return n * 2


log.clear()
print(deco_target(2), deco_target(3), log)


class Holder:
    @contextmanager
    def ctx(self, tag):
        log.append(("ctx", type(self).__name__, tag))
        yield tag

    @staticmethod
    @contextmanager
    def static_ctx():
        yield "static"


log.clear()
with Holder().ctx("t") as tag:
    print(tag)
with Holder.static_ctx() as tag:
    print(tag)
print(log)


# generator misuse
@contextmanager
def never_yields():
    return
    yield


try:
    with never_yields():
        pass
except RuntimeError as exc:
    print(exc, exc.__cause__, exc.__suppress_context__)


@contextmanager
def yields_twice():
    yield 1
    yield 2


try:
    with yields_twice():
        pass
except RuntimeError as exc:
    print(exc)


@contextmanager
def yields_twice_after_throw():
    try:
        yield 1
    except ValueError:
        yield 2


try:
    with yields_twice_after_throw():
        raise ValueError("x")
except RuntimeError as exc:
    print(exc)


@contextmanager
def replaces_exception():
    try:
        yield
    except ValueError as exc:
        raise KeyError("replaced") from exc


try:
    with replaces_exception():
        raise ValueError("orig")
except KeyError as exc:
    print("replaced", repr(exc.__cause__))


# StopIteration raised in the body is not suppressed (PEP 479 wrapping)
@contextmanager
def plain():
    yield


try:
    with plain():
        raise StopIteration("body")
except StopIteration as exc:
    print("StopIteration propagated", exc)

# re-entering the same one-shot manager fails
cm = managed(1)
with cm:
    pass
try:
    with cm:
        pass
except AttributeError as exc:
    print("AttributeError", exc)

# ── closing / nullcontext / suppress ────────────────────────────────────────
class Closable:
    closed = 0

    def close(self):
        Closable.closed += 1


thing = Closable()
with closing(thing) as got:
    print(got is thing)
print(Closable.closed, closing.__enter__(closing(3)))
print(nullcontext(5).__enter__(), nullcontext().__enter__(), nullcontext(enter_result=6).enter_result)
with nullcontext():
    pass
print(suppress().__exit__(ValueError, ValueError(), None))
print(suppress(ValueError, KeyError).__exit__(KeyError, KeyError(), None))
print(suppress(ValueError).__exit__(None, None, None))
try:
    suppress(ValueError).__exit__(5, None, None)
except TypeError as exc:
    print("TypeError", exc)

try:
    with suppress(ValueError):
        raise ExceptionGroup("eg", [ValueError("a"), ValueError("b")])
    print("group fully suppressed")
    with suppress(ValueError):
        raise ExceptionGroup("eg", [ValueError("a"), TypeError("b")])
except ExceptionGroup as eg:
    print("rest", [type(e).__name__ for e in eg.exceptions])
