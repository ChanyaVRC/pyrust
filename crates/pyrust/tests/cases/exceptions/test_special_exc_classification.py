# Special-exception attribute classification, including user subclasses.
# Regression guard for issue #1967: exception construction was sped up by
# replacing ~12 per-instance cloning MRO walks with a single non-cloning pass;
# the classification (which exceptions get .value / .code / errno / etc.) must
# stay identical to CPython 3.12, especially for user subclasses.


# StopIteration.value
try:
    raise StopIteration(42)
except StopIteration as e:
    print("stop", e.value, e.args)
try:
    raise StopIteration()
except StopIteration as e:
    print("stop-empty", e.value, e.args)


# SystemExit.code (1 arg -> code is arg; multi -> code is args tuple)
try:
    raise SystemExit(3)
except SystemExit as e:
    print("sysexit", e.code)
try:
    raise SystemExit(1, 2)
except SystemExit as e:
    print("sysexit-multi", e.code, e.code is e.args)


# SyntaxError structured attributes
try:
    raise SyntaxError("bad", ("f.py", 1, 2, "text", 3, 4))
except SyntaxError as e:
    print("syntax", e.msg, e.filename, e.lineno, e.offset, e.text,
          e.end_lineno, e.end_offset)


# OSError errno remap to a concrete subclass
try:
    raise OSError(2, "no such file")
except Exception as e:
    print("os-remap", type(e).__name__, e.errno, e.strerror, e.args, e.filename)
try:
    raise OSError(13, "perm", "f.txt")
except Exception as e:
    print("os-3arg", type(e).__name__, e.errno, e.filename)


# Unicode errors
try:
    raise UnicodeDecodeError("utf-8", b"abc", 0, 1, "bad")
except UnicodeDecodeError as e:
    print("udecode", e.encoding, e.object, e.start, e.end, e.reason)
try:
    raise UnicodeEncodeError("utf-8", "abc", 0, 1, "bad")
except UnicodeEncodeError as e:
    print("uencode", e.encoding, e.object, e.start, e.end, e.reason)
try:
    raise UnicodeTranslateError("abc", 0, 1, "bad")
except UnicodeTranslateError as e:
    print("utranslate", e.object, e.start, e.end, e.reason)


# NameError / ImportError attributes
try:
    raise NameError("x")
except NameError as e:
    print("name", e.name)
try:
    raise ImportError("x", name="mod", path="p")
except ImportError as e:
    print("import", e.name, e.path)


# ExceptionGroup
try:
    raise ExceptionGroup("grp", [ValueError(1), TypeError(2)])
except ExceptionGroup as e:
    print("group", e.message, [type(x).__name__ for x in e.exceptions])


# Plain exception keeps its args and gets no special attribute
try:
    raise ValueError("a", "b")
except ValueError as e:
    print("value", e.args, hasattr(e, "value"))


# --- user subclasses must inherit the special handling (issue #612) ---

class MyOS(OSError):
    pass


try:
    raise MyOS(2, "nf")
except MyOS as e:
    print("user-os", e.errno, e.strerror, e.args)


class MyStop(StopIteration):
    pass


try:
    raise MyStop(99)
except MyStop as e:
    print("user-stop", e.value)


class MyDecode(UnicodeDecodeError):
    pass


try:
    raise MyDecode("utf-8", b"abc", 0, 1, "bad")
except MyDecode as e:
    print("user-udecode", e.encoding, e.reason)


# Multiple-inheritance exception subclass
class MixA(ValueError):
    pass


class MixB(KeyError):
    pass


class Mix(MixA, MixB):
    pass


try:
    raise Mix("z")
except ValueError as e:
    print("mix", e.args, isinstance(e, KeyError))


# Exception chaining survives construction
try:
    try:
        raise ValueError("inner")
    except ValueError as inner:
        raise TypeError("outer") from inner
except TypeError as e:
    print("chain", e.__cause__, type(e.__context__).__name__)
