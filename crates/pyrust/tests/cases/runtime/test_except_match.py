# MatchExcept: exception type matching in except clauses
try:
    raise ValueError("oops")
except ValueError as e:
    print("caught ValueError:", e)   # caught ValueError: oops
except TypeError:
    print("wrong branch")

try:
    raise TypeError("bad type")
except ValueError:
    print("wrong branch")
except TypeError as e:
    print("caught TypeError:", e)    # caught TypeError: bad type

# Catch base class
try:
    raise ValueError("base")
except Exception as e:
    print("caught Exception:", e)    # caught Exception: base

# Uncaught re-raise propagates
try:
    try:
        raise RuntimeError("inner")
    except ValueError:
        print("wrong")
except RuntimeError as e:
    print("re-raised:", e)           # re-raised: inner
