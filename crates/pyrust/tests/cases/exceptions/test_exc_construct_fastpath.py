# Exercise the exception-construction path optimised in the
# "reduce exception construction overhead" change: plain built-in exceptions
# (no user __new__/__init__) take a fast path that skips the dedicated
# __new__/__init__ MRO lookups, while user-defined __new__/__init__ subclasses
# must still be dispatched correctly.

# --- plain built-in: fast path, no user __new__/__init__ ---
try:
    raise ValueError("x")
except ValueError as e:
    print("plain", e.args, type(e.__traceback__).__name__)

# --- attribute-only subclass: still fast path ---
class Plain(ValueError):
    pass


try:
    raise Plain("y")
except Plain as e:
    print("subclass", e.args, type(e).__name__)

# --- user __init__ must run and may override args via super().__init__ ---
class WithInit(ValueError):
    def __init__(self, a, b):
        super().__init__(a)
        self.extra = b


try:
    raise WithInit("m", 7)
except WithInit as e:
    print("init", e.args, e.extra)

# --- user __new__ must run and its result is used ---
class WithNew(Exception):
    def __new__(cls, *a):
        inst = super().__new__(cls, *a)
        inst.tag = "tagged"
        return inst


try:
    raise WithNew("hi")
except WithNew as e:
    print("new", e.args, e.tag)

# --- user __new__ AND __init__ together ---
class Both(Exception):
    def __new__(cls, *a):
        inst = super().__new__(cls, *a)
        inst.created = True
        return inst

    def __init__(self, *a):
        super().__init__(*a)
        self.inited = True


try:
    raise Both(1, 2)
except Both as e:
    print("both", e.args, e.created, e.inited)

# --- special-attr exceptions still classified correctly on the new path ---
try:
    raise StopIteration(99)
except StopIteration as e:
    print("stop", e.value)

try:
    raise OSError(2, "missing")
except FileNotFoundError as e:
    print("os", type(e).__name__, e.errno, e.strerror)


# --- user subclass of a special exception still inherits special handling ---
class MySE(SystemExit):
    pass


try:
    raise MySE(5)
except MySE as e:
    print("se", e.code)
