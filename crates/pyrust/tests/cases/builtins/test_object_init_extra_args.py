# Parity fixture: object.__init__() rejects extra arguments.
#
# CPython 3.12 rule (Objects/typeobject.c, object_init):
#
#   Extra args are accepted (and ignored) only when BOTH:
#     (a) type(self) defines a custom __new__ (not object.__new__), AND
#     (b) type(self) does NOT define a custom __init__ (inherits object.__init__).
#
#   In all other cases a TypeError is raised.

# ── Error path: extra positional arg via super().__init__() ──────────────────

class Foo:
    def __init__(self):
        super().__init__(42)  # object.__init__ called with extra arg

try:
    Foo()
except TypeError as e:
    print(e)

# ── Error path: custom __new__ + custom __init__, extra arg in super() ───────

class WithBothCustom:
    def __new__(cls, x):
        return super().__new__(cls)
    def __init__(self, x):
        super().__init__(x)  # extra arg — should still raise

try:
    WithBothCustom(42)
except TypeError as e:
    print(e)

# ── Error path: extra keyword arg ───────────────────────────────────────────

class Plain:
    def __init__(self):
        super().__init__(extra=1)

try:
    Plain()
except TypeError as e:
    print(e)

# ── Happy path: no extra args ────────────────────────────────────────────────

class Bar:
    def __init__(self):
        super().__init__()

Bar()
print("super().__init__() with no extra args: ok")

# ── Happy path: no __init__ defined ─────────────────────────────────────────

class Baz:
    pass

Baz()
print("Baz() with no __init__: ok")

# ── Happy path: custom __new__ only, object.__init__ lenient ────────────────

class WithCustomNewOnly:
    def __new__(cls, x):
        return super().__new__(cls)

obj = WithCustomNewOnly.__new__(WithCustomNewOnly, 42)
result = object.__init__(obj, 42)  # lenient: custom __new__, no custom __init__
print("custom __new__ only, extra arg accepted:", result)
