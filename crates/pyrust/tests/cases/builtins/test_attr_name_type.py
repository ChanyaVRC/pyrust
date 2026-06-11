# Parity fixture for the attribute-name type check shared by
# getattr / setattr / hasattr / delattr (issue #2350).
#
# CPython 3.12 raises, for a non-str name:
#   TypeError: attribute name must be string, not '<type>'
# No function-name prefix, "be string" (no article), and the offending
# type is named.  str subclasses are accepted as names.


class C:
    pass


obj = C()


class StrName(str):
    pass


names = [
    ("int", 1),
    ("None", None),
    ("bytes", b"x"),
    ("inst", C()),
]

# ── hasattr ───────────────────────────────────────────────────────────────────
for label, name in names:
    try:
        hasattr(obj, name)
    except TypeError as e:
        print("hasattr", label, type(e).__name__, str(e))

# ── getattr (2-arg) ───────────────────────────────────────────────────────────
for label, name in names:
    try:
        getattr(obj, name)
    except TypeError as e:
        print("getattr2", label, type(e).__name__, str(e))

# ── getattr (3-arg, default supplied) — name is still validated first ──────────
for label, name in names:
    try:
        getattr(obj, name, "default")
    except TypeError as e:
        print("getattr3", label, type(e).__name__, str(e))

# ── setattr ───────────────────────────────────────────────────────────────────
for label, name in names:
    try:
        setattr(obj, name, 1)
    except TypeError as e:
        print("setattr", label, type(e).__name__, str(e))

# ── delattr ───────────────────────────────────────────────────────────────────
for label, name in names:
    try:
        delattr(obj, name)
    except TypeError as e:
        print("delattr", label, type(e).__name__, str(e))

# ── str subclasses are ACCEPTED as attribute names ────────────────────────────
target = C()
setattr(target, StrName("alpha"), 10)
print("setattr str-subclass:", target.alpha)
print("hasattr str-subclass:", hasattr(target, StrName("alpha")))
print("getattr str-subclass:", getattr(target, StrName("alpha")))
delattr(target, StrName("alpha"))
print("delattr str-subclass removed:", hasattr(target, "alpha"))

# ── arity / keyword wording matches CPython for setattr / delattr too ──────────
arity_cases = [
    ("delattr 1arg", lambda: delattr(obj)),
    ("delattr 3arg", lambda: delattr(obj, "x", "y")),
    ("delattr kw", lambda: delattr(obj, name="x")),
    ("setattr 2arg", lambda: setattr(obj, "x")),
    ("setattr 4arg", lambda: setattr(obj, "x", 1, 2)),
    ("setattr kw", lambda: setattr(obj, name="x", value=1)),
]
for label, fn in arity_cases:
    try:
        fn()
    except TypeError as e:
        print(label, "->", str(e))

# ── deque.__setattr__ shares the same name-type wording (#2350) ────────────────
from collections import deque

dq = deque()
for label, name in names:
    try:
        dq.__setattr__(name, 1)
    except TypeError as e:
        print("deque.__setattr__", label, type(e).__name__, str(e))
# str subclass name reaches the writability check, not the type check
try:
    dq.__setattr__(StrName("maxlen"), 1)
except Exception as e:
    print("deque str-subclass maxlen ->", type(e).__name__, str(e))
