# PEP 654 `except*` recurses into nested ExceptionGroups (#1999 sibling: #2206).
# A matching leaf at any nesting depth must be caught, with the matched and
# unmatched partitions each preserving the nested group structure.  Previously
# `except*` only inspected the raised group's direct children as leaves.


def shape(eg, depth=0):
    if isinstance(eg, BaseExceptionGroup):
        print("  " * depth + f"GROUP {eg.message!r} ({len(eg.exceptions)})")
        for e in eg.exceptions:
            shape(e, depth + 1)
    else:
        print("  " * depth + f"{type(eg).__name__}({eg.args[0]!r})")


# --- 1-level (regression guard for flat direct-leaf behaviour) ---
try:
    raise ExceptionGroup("g", [KeyError("k"), ValueError("v"), TypeError("t")])
except* KeyError as eg:
    print("flat KeyError:", [type(e).__name__ for e in eg.exceptions])
except* (ValueError, TypeError) as eg:
    print("flat ValueError/TypeError:", sorted(type(e).__name__ for e in eg.exceptions))

# --- a leaf nested one level deep is caught ---
try:
    raise ExceptionGroup("outer", [ExceptionGroup("inner", [KeyError("k")]), ValueError("v")])
except* KeyError as eg:
    print("nested KeyError caught")
    shape(eg)
except* ValueError:
    print("ValueError clause")

# --- 3-level nesting, multiple clauses, structure preserved ---
src = ExceptionGroup(
    "L0",
    [
        ExceptionGroup("L1a", [KeyError("k1"), ValueError("v1")]),
        ExceptionGroup(
            "L1b",
            [ExceptionGroup("L2", [KeyError("k2"), TypeError("t1")]), ValueError("v2")],
        ),
        IndexError("i1"),
    ],
)
try:
    raise src
except* KeyError as eg:
    print("=== KeyError partition ===")
    shape(eg)
except* ValueError as eg:
    print("=== ValueError partition ===")
    shape(eg)
except* (TypeError, IndexError) as eg:
    print("=== TypeError/IndexError partition ===")
    shape(eg)

# --- implicit wrapping of a plain (non-group) exception ---
try:
    raise ValueError("solo")
except* ValueError as eg:
    print("wrapped:", type(eg).__name__, [type(e).__name__ for e in eg.exceptions])

# --- each clause runs at most once ---
runs = []
try:
    raise ExceptionGroup("g", [ExceptionGroup("i", [KeyError("a"), KeyError("b")])])
except* KeyError as eg:
    runs.append(len(eg.exceptions))
print("clause runs:", len(runs))


# --- PEP 654: `except*` rejects ExceptionGroup-subclass / non-exception catch
#     types with a TypeError (the "does not inherit" check wins over the EG
#     check across a tuple).  The filter runs only when an exception is present;
#     each helper raises one, and we compare just the resulting message (the
#     rich EG traceback rendering is out of scope). ---
class MyEG(ExceptionGroup):
    pass


def reject(fn):
    try:
        fn()
    except TypeError as e:
        print("TypeError:", e)
    else:
        print("no error")


def c_eg():
    try:
        raise ValueError("x")
    except* ExceptionGroup:
        pass


def c_baseeg():
    try:
        raise ValueError("x")
    except* BaseExceptionGroup:
        pass


def c_user_eg():
    try:
        raise ValueError("x")
    except* MyEG:
        pass


def c_tuple_eg():
    try:
        raise ValueError("x")
    except* (ValueError, ExceptionGroup):
        pass


def c_tuple_eg_then_int():
    try:
        raise ValueError("x")
    except* (ExceptionGroup, int):
        pass


def c_int():
    try:
        raise ValueError("x")
    except* int:
        pass


def c_group_raised_eg():
    try:
        raise ExceptionGroup("g", [ValueError("v")])
    except* ExceptionGroup:
        pass


for fn in (c_eg, c_baseeg, c_user_eg, c_tuple_eg, c_tuple_eg_then_int, c_int, c_group_raised_eg):
    reject(fn)

