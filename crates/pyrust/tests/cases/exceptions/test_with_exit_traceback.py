# Issue #2359: a context manager's `__exit__(exc_type, exc_val, tb)` must
# receive the *real* traceback object for the in-flight exception (not None),
# and that object must be the same one a later `e.__traceback__` read in the
# same frame observes (#2351 deferred placeholder materialised through the
# get_attr interceptor).  Mirrors `async with`/`__aexit__` semantics.

import asyncio


# Unsuppressed: __exit__ sees a real traceback, walks tb_frame/tb_lineno, and
# returns False so the exception propagates.
class CM:
    def __enter__(self):
        return self

    def __exit__(self, t, v, tb):
        print("type:", type(tb).__name__)
        print("co_name:", tb.tb_frame.f_code.co_name)
        print("lineno:", tb.tb_lineno)
        return False


try:
    with CM():
        raise ValueError("x")
except ValueError:
    print("propagated")


# Identity: the tb passed to __exit__ is the same object as e.__traceback__
# read by an outer except in the same frame.
captured = []


class Capture:
    def __enter__(self):
        return self

    def __exit__(self, t, v, tb):
        captured.append(tb)
        return False


try:
    with Capture():
        raise KeyError("k")
except KeyError as e:
    print("identity:", e.__traceback__ is captured[0])


# Suppressed: returning True swallows the exception.
class Suppress:
    def __enter__(self):
        return self

    def __exit__(self, t, v, tb):
        print("suppress tb:", type(tb).__name__)
        return True


with Suppress():
    raise RuntimeError("swallowed")
print("after-suppress")


# Nested with: both __exit__s see a real traceback as it propagates outward.
class Outer:
    def __enter__(self):
        return self

    def __exit__(self, t, v, tb):
        print("outer:", type(tb).__name__)
        return False


class Inner:
    def __enter__(self):
        return self

    def __exit__(self, t, v, tb):
        print("inner:", type(tb).__name__)
        return False


try:
    with Outer():
        with Inner():
            raise IndexError("idx")
except IndexError:
    print("nested-done")


# No-exception path: __exit__ receives (None, None, None).
class NoExc:
    def __enter__(self):
        return self

    def __exit__(self, t, v, tb):
        print("noexc:", t, v, tb)
        return False


with NoExc():
    pass


# Deep call chain: __exit__ walks every frame from the catching frame inward.
def deep():
    raise ValueError("deep")


class Walker:
    def __enter__(self):
        return self

    def __exit__(self, t, v, tb):
        names = []
        cur = tb
        while cur is not None:
            names.append(cur.tb_frame.f_code.co_name)
            cur = cur.tb_next
        print("walk:", names)
        return True


with Walker():
    deep()


# __exit__ raising its own exception chains the original as __context__.
class Chainer:
    def __enter__(self):
        return self

    def __exit__(self, t, v, tb):
        raise TypeError("from exit")


try:
    with Chainer():
        raise ValueError("orig")
except TypeError as e:
    print("chain:", type(e.__context__).__name__)


# with inside a generator + throw: __exit__ sees the thrown exception's tb.
def gen():
    class G:
        def __enter__(self):
            return self

        def __exit__(self, t, v, tb):
            print("gen-exit:", type(tb).__name__)
            return False

    with G():
        yield 1
        yield 2


g = gen()
print(next(g))
try:
    g.throw(ValueError("thrown"))
except ValueError:
    print("gen-throw-propagated")


# async with: __aexit__ obeys the same triple semantics, including identity and
# the no-exception (None, None, None) path.
class ACM:
    async def __aenter__(self):
        return self

    async def __aexit__(self, t, v, tb):
        print("aexit:", type(tb).__name__)
        return False


seen = []


class ACapture:
    async def __aenter__(self):
        return self

    async def __aexit__(self, t, v, tb):
        seen.append(tb)
        return False


async def main():
    try:
        async with ACM():
            raise ValueError("av")
    except ValueError:
        print("async-propagated")

    try:
        async with ACapture():
            raise KeyError("ak")
    except KeyError as e:
        print("async-identity:", e.__traceback__ is seen[0])

    async with ACM():
        pass


asyncio.run(main())
