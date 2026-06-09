# PEP 695 (#2274): a TypeVar's __name__, __bound__, __constraints__ and the
# variance flags are read-only getset descriptors in CPython 3.12.  Assigning or
# deleting them raises AttributeError (with two distinct messages); reads still
# work and arbitrary attributes remain writable.

def f[T: int](x):
    return x

tv = f.__type_params__[0]

# ── Reads are unaffected ─────────────────────────────────────────────────────

print(tv.__name__)          # T
print(tv.__bound__)         # <class 'int'>
print(tv.__constraints__)   # ()

def g[U: (int, str)](x):
    return x

print(g.__type_params__[0].__constraints__)  # (<class 'int'>, <class 'str'>)

# ── Assignment to each read-only descriptor raises AttributeError ────────────
# CPython uses "attribute '<name>' of 'typing.TypeVar' objects is not writable"
# for __bound__ / __constraints__, and the generic "readonly attribute" for
# __name__ and the variance flags.

readonly = [
    "__name__",
    "__bound__",
    "__constraints__",
    "__covariant__",
    "__contravariant__",
    "__infer_variance__",
]

for attr in readonly:
    try:
        setattr(tv, attr, None)
    except AttributeError as e:
        print("SET", attr, "->", str(e))

for attr in readonly:
    try:
        delattr(tv, attr)
    except AttributeError as e:
        print("DEL", attr, "->", str(e))

# ── Attribute-assignment syntax raises the same error ────────────────────────

try:
    tv.__bound__ = str
except AttributeError as e:
    print("attr-set __bound__ ->", str(e))

try:
    del tv.__bound__
except AttributeError as e:
    print("attr-del __bound__ ->", str(e))

# A failed write does not mutate the value.
print(tv.__bound__)  # <class 'int'>

# ── Arbitrary (non-descriptor) attributes remain writable ────────────────────

tv.custom = 42
print(tv.custom)  # 42
del tv.custom
print(hasattr(tv, "custom"))  # False
