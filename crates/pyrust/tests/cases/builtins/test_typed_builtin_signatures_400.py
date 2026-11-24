"""CPython 3.12 parity tests for #400 typed-signature builtin migrations.

Exercises valid calls (positional, keyword, omitted vs explicit None) and
exact TypeError parity across all builtins migrated in #400.
"""


def show(label, fn):
    try:
        r = fn()
        print(f"{label}: OK -> {r!r}")
    except BaseException as e:
        print(f"{label}: ERR -> {type(e).__name__}: {e}")


# ── Tier 2: numeric & scalar constructors ────────────────────────────────────
show("round(3.5)", lambda: round(3.5))
show("round(number=3.14159, ndigits=2)", lambda: round(number=3.14159, ndigits=2))
show("round(3.5, None)", lambda: round(3.5, None))
show("round()", lambda: round())
show("round(1, 2, 3)", lambda: round(1, 2, 3))
show("round(foo=1)", lambda: round(foo=1))
show("round(1, foo=1)", lambda: round(1, foo=1))
show("round(1, number=2)", lambda: round(1, number=2))

show("pow(2, 10)", lambda: pow(2, 10))
show("pow(base=2, exp=3)", lambda: pow(base=2, exp=3))
show("pow(2, 3, mod=5)", lambda: pow(2, 3, mod=5))
show("pow(2, 3, None)", lambda: pow(2, 3, None))
show("pow(2, 3, mod=None)", lambda: pow(2, 3, mod=None))
show("pow()", lambda: pow())
show("pow(2)", lambda: pow(2))
show("pow(2, 3, 4, 5)", lambda: pow(2, 3, 4, 5))
show("pow(foo=1)", lambda: pow(foo=1))
show("pow(2, 3, foo=5)", lambda: pow(2, 3, foo=5))
show("pow(2, 3, base=4)", lambda: pow(2, 3, base=4))

show("int()", lambda: int())
show("int('ff', base=16)", lambda: int("ff", base=16))
show("int(base=2)", lambda: int(base=2))
show("int(1, 2, 3)", lambda: int(1, 2, 3))
show("int(x=1)", lambda: int(x=1))
show("int(1, base=10, foo=1)", lambda: int(1, base=10, foo=1))

show("float()", lambda: float())
show("float('3.5')", lambda: float("3.5"))
show("float(None)", lambda: float(None))
show("float(1, 2)", lambda: float(1, 2))
show("float(x=1)", lambda: float(x=1))

show("str()", lambda: str())
show("str(object=42)", lambda: str(object=42))
show("str(b'hi', encoding='utf-8', errors='strict')", lambda: str(b"hi", encoding="utf-8", errors="strict"))
show("str(1, 2, 3, 4)", lambda: str(1, 2, 3, 4))
show("str(foo=1)", lambda: str(foo=1))
show("str(1, object=2)", lambda: str(1, object=2))

show("complex()", lambda: complex())
show("complex(imag=5)", lambda: complex(imag=5))
show("complex(real=1, imag=2)", lambda: complex(real=1, imag=2))
show("complex(1, 2, 3)", lambda: complex(1, 2, 3))
show("complex(foo=1)", lambda: complex(foo=1))
show("complex(1, real=2)", lambda: complex(1, real=2))

show("bytes()", lambda: bytes())
show("bytes(source='hi', encoding='utf-8')", lambda: bytes(source="hi", encoding="utf-8"))
show("bytes(1, 2, 3, 4)", lambda: bytes(1, 2, 3, 4))
show("bytes(foo=1)", lambda: bytes(foo=1))
show("bytes(1, source=2)", lambda: bytes(1, source=2))

show("bytearray()", lambda: bytearray())
show("bytearray(source='hi', encoding='utf-8')", lambda: bytearray(source="hi", encoding="utf-8"))
show("bytearray(1, 2, 3, 4)", lambda: bytearray(1, 2, 3, 4))
show("bytearray(foo=1)", lambda: bytearray(foo=1))
show("bytearray(1, source=2)", lambda: bytearray(1, source=2))

# ── Tier 3: iterables & collection constructors ──────────────────────────────
show("sum([1, 2, 3])", lambda: sum([1, 2, 3]))
show("sum([1, 2], start=10)", lambda: sum([1, 2], start=10))
show("sum([], None)", lambda: sum([], None))
show("sum()", lambda: sum())
show("sum([], 0, 1)", lambda: sum([], 0, 1))
show("sum(iterable=[])", lambda: sum(iterable=[]))
show("sum([], foo=1)", lambda: sum([], foo=1))

show("sorted([3, 1, 2])", lambda: sorted([3, 1, 2]))
show("sorted(['bb', 'a', 'ccc'], key=len, reverse=True)", lambda: sorted(["bb", "a", "ccc"], key=len, reverse=True))
show("sorted()", lambda: sorted())
show("sorted([], [])", lambda: sorted([], []))

show("list()", lambda: list())
show("list((1, 2))", lambda: list((1, 2)))
show("list(None)", lambda: list(None))
show("list(1, 2)", lambda: list(1, 2))
show("list(iterable=[])", lambda: list(iterable=[]))

show("tuple()", lambda: tuple())
show("tuple([1, 2])", lambda: tuple([1, 2]))
show("tuple(None)", lambda: tuple(None))
show("tuple(1, 2)", lambda: tuple(1, 2))
show("tuple(iterable=[])", lambda: tuple(iterable=[]))

show("set()", lambda: set())
show("set([1, 1, 2])", lambda: sorted(set([1, 1, 2])))
show("set(None)", lambda: set(None))
show("set(1, 2)", lambda: set(1, 2))
show("set(iterable=[])", lambda: set(iterable=[]))

show("frozenset()", lambda: frozenset())
show("frozenset([1, 2])", lambda: sorted(frozenset([1, 2])))
show("frozenset(None)", lambda: frozenset(None))
show("frozenset(1, 2)", lambda: frozenset(1, 2))
show("frozenset(iterable=[])", lambda: frozenset(iterable=[]))

# ── Tier 3: reflection & namespace helpers ───────────────────────────────────
class Box:
    pass


b = Box()
setattr(b, "x", 99)
show("hasattr(b, 'x')", lambda: hasattr(b, "x"))
show("hasattr(b, 'y')", lambda: hasattr(b, "y"))
show("getattr(b, 'x')", lambda: getattr(b, "x"))
show("getattr(b, 'y', None)", lambda: getattr(b, "y", None))
show("getattr(b, 'y', 'def')", lambda: getattr(b, "y", "def"))
show("getattr(b, 'y')", lambda: getattr(b, "y"))
delattr(b, "x")
show("hasattr after delattr", lambda: hasattr(b, "x"))

show("issubclass(bool, int)", lambda: issubclass(bool, int))
show("issubclass()", lambda: issubclass())
show("issubclass(int)", lambda: issubclass(int))
show("issubclass(int, int, int)", lambda: issubclass(int, int, int))
show("issubclass(cls=bool, classinfo=int)", lambda: issubclass(cls=bool, classinfo=int))

show("delattr()", lambda: delattr())
show("delattr(b, 'x', 1)", lambda: delattr(b, "x", 1))
show("delattr(b, 123)", lambda: delattr(b, 123))
show("delattr(object=b, name='x')", lambda: delattr(object=b, name="x"))

show("hasattr()", lambda: hasattr())
show("hasattr(b, 'x', 1)", lambda: hasattr(b, "x", 1))
show("hasattr(b, 123)", lambda: hasattr(b, 123))
show("hasattr(object=b, name='x')", lambda: hasattr(object=b, name="x"))

show("getattr()", lambda: getattr())
show("getattr(b)", lambda: getattr(b))
show("getattr(b, 'x', 1, 2)", lambda: getattr(b, "x", 1, 2))
show("getattr(b, 123)", lambda: getattr(b, 123))
show("getattr(object=b, name='x')", lambda: getattr(object=b, name="x"))

show("setattr()", lambda: setattr())
show("setattr(b, 'x')", lambda: setattr(b, "x"))
show("setattr(b, 'x', 1, 2)", lambda: setattr(b, "x", 1, 2))
show("setattr(b, 123, 1)", lambda: setattr(b, 123, 1))
show("setattr(object=b, name='x', value=1)", lambda: setattr(object=b, name="x", value=1))

show("vars(b)", lambda: vars(b))
show("vars(1, 2)", lambda: vars(1, 2))
show("vars(object=b)", lambda: vars(object=b))

show("dir(1, 2)", lambda: dir(1, 2))
show("dir(object=b)", lambda: dir(object=b))

show("globals(1)", lambda: globals(1))
show("globals(x=1)", lambda: globals(x=1))
show("locals(1)", lambda: locals(1))
show("locals(x=1)", lambda: locals(x=1))

ns = {}
exec("val = 123", ns)
show("exec ns['val']", lambda: ns["val"])
show("exec()", lambda: exec())
show("exec('1', foo=1)", lambda: exec("1", foo=1))
show("exec('1', globals={})", lambda: exec("1", globals={}))

show("eval('2 + 3')", lambda: eval("2 + 3"))
show("eval()", lambda: eval())
show("eval('1', None, None, None)", lambda: eval("1", None, None, None))
show("eval('1', foo=1)", lambda: eval("1", foo=1))

code_obj = compile("40 + 2", "<test>", "eval", flags=0)
show("eval(compile(...))", lambda: eval(code_obj))
show("compile()", lambda: compile())
show("compile('1', 'a')", lambda: compile("1", "a"))
show("compile('1', 'a', 'eval', foo=1)", lambda: compile("1", "a", "eval", foo=1))

show("__import__(name='math').__name__", lambda: __import__(name="math").__name__)
show("__import__()", lambda: __import__())
show("__import__(foo=1)", lambda: __import__(foo=1))
show("__import__('math', foo=1)", lambda: __import__("math", foo=1))

# ── Tier 3: descriptors, range, slice, super, input ──────────────────────────
show("classmethod()", lambda: classmethod())
show("classmethod(1, 2)", lambda: classmethod(1, 2))
show("classmethod(function=1)", lambda: classmethod(function=1))

show("staticmethod()", lambda: staticmethod())
show("staticmethod(1, 2)", lambda: staticmethod(1, 2))
show("staticmethod(function=1)", lambda: staticmethod(function=1))

p = property(fget=lambda self: 7, doc="seven")
show("property doc", lambda: p.__doc__)
show("property(1, 2, 3, 4, 5)", lambda: property(1, 2, 3, 4, 5))
show("property(foo=1)", lambda: property(foo=1))
show("property(1, fget=1)", lambda: property(1, fget=1))

show("super(int, None)", lambda: repr(super(int, None)))
show("super(1, 2, 3)", lambda: super(1, 2, 3))
show("super(t=int)", lambda: super(t=int))

show("slice(5)", lambda: slice(5))
show("slice(1, 5, 2)", lambda: slice(1, 5, 2))
show("slice(None)", lambda: slice(None))
show("slice()", lambda: slice())
show("slice(1, 2, 3, 4)", lambda: slice(1, 2, 3, 4))
show("slice(stop=5)", lambda: slice(stop=5))

show("list(range(4))", lambda: list(range(4)))
show("list(range(1, 6, 2))", lambda: list(range(1, 6, 2)))
show("range()", lambda: range())
show("range(1, 2, 3, 4)", lambda: range(1, 2, 3, 4))
show("range(stop=5)", lambda: range(stop=5))

show("input(1, 2)", lambda: input(1, 2))
show("input(prompt='x')", lambda: input(prompt="x"))

# ── Tier 4: hot-path iterator builtins ───────────────────────────────────────
it = iter([10, 20])
show("next(it) #1", lambda: next(it))
show("next(it) #2", lambda: next(it))
show("next(it, None)", lambda: next(it, None))
show("next(it, 'done')", lambda: next(it, "done"))
show("next(it) exhausted", lambda: next(it))
show("next()", lambda: next())
show("next(it, 1, 2)", lambda: next(it, 1, 2))
show("next(iterator=it)", lambda: next(iterator=it))

calls = iter([1, 2, None, 3])
show("list(iter(callable, None))", lambda: list(iter(lambda: next(calls), None)))
show("iter()", lambda: iter())
show("iter(1, 2, 3)", lambda: iter(1, 2, 3))
show("iter(object=[])", lambda: iter(object=[]))

show("anext()", lambda: anext())
show("anext(1, 2, 3)", lambda: anext(1, 2, 3))
show("anext(a=1)", lambda: anext(a=1))
