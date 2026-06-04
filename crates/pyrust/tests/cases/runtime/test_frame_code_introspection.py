# Issue #2171: sys._getframe() returns frame objects with
# f_code/f_back/f_globals/f_locals, and code objects expose
# co_flags/co_filename/co_firstlineno/co_consts/co_names in addition to
# co_name/co_argcount/co_varnames.  Asserts STRUCTURE/types — never addresses
# or absolute file paths.
import sys


def g():
    f = sys._getframe()
    print("frame type:", type(f).__name__)
    print("f_code.co_name:", f.f_code.co_name)
    print("f_back.co_name:", f.f_back.f_code.co_name)
    print("f_lineno is int:", isinstance(f.f_lineno, int))
    print("f_globals is dict:", isinstance(f.f_globals, dict))
    print("f_locals is dict:", isinstance(f.f_locals, dict))
    # _getframe(1) is the caller (module).
    print("getframe(1):", sys._getframe(1).f_code.co_name)


g()

# Top-level frame: f_back is None.
top = sys._getframe()
print("top co_name:", top.f_code.co_name)
print("top f_back:", top.f_back)

# Too-deep request raises ValueError.
try:
    sys._getframe(10_000)
except ValueError as e:
    print("deep:", type(e).__name__, str(e))


# --- code object attributes ---
def fn(a, b, *args, **kwargs):
    c = a + b
    return c


code = fn.__code__
print("co_name:", code.co_name)
print("co_argcount:", code.co_argcount)
print("co_varnames startswith ab:", code.co_varnames[:2] == ("a", "b"))
print("co_flags is int:", isinstance(code.co_flags, int))
# CO_VARARGS (0x04) and CO_VARKEYWORDS (0x08) are set for *args/**kwargs.
print("CO_VARARGS set:", bool(code.co_flags & 0x04))
print("CO_VARKEYWORDS set:", bool(code.co_flags & 0x08))
print("co_filename is str:", isinstance(code.co_filename, str))
print("co_firstlineno is int:", isinstance(code.co_firstlineno, int))
print("co_consts is tuple:", isinstance(code.co_consts, tuple))
print("co_names is tuple:", isinstance(code.co_names, tuple))


def gen():
    yield 1


# A generator function's code sets CO_GENERATOR (0x20).
print("CO_GENERATOR set:", bool(gen.__code__.co_flags & 0x20))
