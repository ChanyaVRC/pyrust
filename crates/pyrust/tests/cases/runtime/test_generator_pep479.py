# PEP 479: StopIteration raised inside a generator must be converted to
# RuntimeError("generator raised StopIteration"). Enforced unconditionally
# since Python 3.7.

# --- basic: raise StopIteration inside a generator ---
def gen_basic():
    yield 1
    raise StopIteration

g = gen_basic()
next(g)
try:
    next(g)
except RuntimeError as e:
    print("basic:", e)
except StopIteration:
    print("basic: wrong - StopIteration escaped")

# --- message from StopIteration is NOT propagated to RuntimeError ---
def gen_msg():
    yield
    raise StopIteration("inner message")

g = gen_msg()
next(g)
try:
    next(g)
except RuntimeError as e:
    print("msg:", e)
except StopIteration:
    print("msg: wrong - StopIteration escaped")

# --- subclass of StopIteration is also converted ---
class MyStop(StopIteration):
    pass

def gen_subclass():
    yield
    raise MyStop("custom")

g = gen_subclass()
next(g)
try:
    next(g)
except RuntimeError as e:
    print("subclass:", e)
except StopIteration:
    print("subclass: wrong - StopIteration escaped")

# --- StopIteration from next() called inside a generator is also converted ---
def gen_next_inside():
    yield
    next(iter([]))

g = gen_next_inside()
next(g)
try:
    next(g)
except RuntimeError as e:
    print("next_inside:", e)
except StopIteration:
    print("next_inside: wrong - StopIteration escaped")

# --- StopIteration in a regular (non-generator) function propagates normally ---
def regular_fn():
    raise StopIteration("from regular")

try:
    regular_fn()
except StopIteration as e:
    print("regular:", e)
except RuntimeError as e:
    print("regular: wrong - got RuntimeError:", e)

# --- RuntimeError inside a generator propagates unchanged ---
def gen_rterror():
    yield
    raise RuntimeError("plain runtime error")

g = gen_rterror()
next(g)
try:
    next(g)
except RuntimeError as e:
    print("rterror:", e)
except StopIteration:
    print("rterror: wrong - got StopIteration")

# --- ValueError inside a generator propagates unchanged ---
def gen_valueerror():
    yield
    raise ValueError("plain value error")

g = gen_valueerror()
next(g)
try:
    next(g)
except ValueError as e:
    print("valueerror:", e)
except StopIteration:
    print("valueerror: wrong - got StopIteration")

# --- generator.close() still works after the fix ---
def gen_close():
    try:
        yield 1
        yield 2
    finally:
        print("close cleanup")

g = gen_close()
next(g)
g.close()
print("close ok")

# --- generator.throw() still works after the fix ---
def gen_throw():
    try:
        yield 1
    except ValueError as e:
        print("throw caught:", e)
        yield 2

g = gen_throw()
next(g)
v = g.throw(ValueError("injected"))
print("throw yielded:", v)
