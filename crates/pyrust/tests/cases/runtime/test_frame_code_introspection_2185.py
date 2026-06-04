# Issue #2185: complete the frame / code-object introspection started in #2183.
#
# Asserts only DETERMINISTIC values (line numbers, names, counts, types) — never
# addresses, ids, or operand-stack sizes (which are implementation-specific).
import sys


# --- f_lineno: the current line for the innermost frame (depth 0) ---------
def here():
    return sys._getframe().f_lineno


# The body of `here` is the line `return sys._getframe()...` — line 11 here.
print("f_lineno:", here())


# --- co_firstlineno: the `def` line, not the first body statement ---------
# (single-line signature; CPython reports the `def` line).
def firstline(a, b):
    x = a + b
    return x


print("co_firstlineno:", firstline.__code__.co_firstlineno)


# --- co_consts: implicit None reserved at slot 0 --------------------------
def no_literals(a, b):
    return a + b


print("co_consts (no literals):", no_literals.__code__.co_consts)


def with_literals():
    a = 10
    b = "hi"
    return a, b


print("co_consts (literals):", with_literals.__code__.co_consts)


# --- co_varnames / co_nlocals: params (CPython order) + body locals -------
def s(a, b, c=1, *args, d, e=2, **kw):
    x = a + b
    y = x
    return y


co = s.__code__
print("co_varnames:", co.co_varnames)
print("co_nlocals:", co.co_nlocals)
print("co_nlocals == len(co_varnames):", co.co_nlocals == len(co.co_varnames))
print("co_argcount:", co.co_argcount)
print("co_posonlyargcount:", co.co_posonlyargcount)
print("co_kwonlyargcount:", co.co_kwonlyargcount)


# --- positional-only / keyword-only counts --------------------------------
def posonly(a, b, /, c, d, *, e, f, **kw):
    pass


pco = posonly.__code__
print("posonly counts:", pco.co_argcount, pco.co_posonlyargcount, pco.co_kwonlyargcount)
print("posonly varnames:", pco.co_varnames)


# --- co_qualname: compile-time qualified name (ignores __qualname__ set) ---
class Outer:
    def method(self):
        pass


def make_inner():
    def inner():
        pass

    return inner


print("method co_qualname:", Outer.method.__code__.co_qualname)
print("inner co_qualname:", make_inner().__code__.co_qualname)
print("lambda co_qualname:", (lambda: 0).__code__.co_qualname)


# co_qualname is fixed at compile time even if __qualname__ is reassigned.
def renamed():
    pass


renamed.__qualname__ = "OVERRIDE"
print("renamed __qualname__:", renamed.__qualname__)
print("renamed co_qualname:", renamed.__code__.co_qualname)


# --- co_cellvars / co_freevars --------------------------------------------
def closure_outer():
    z = 1
    a = 2

    def closure_inner():
        return z + a

    return closure_inner


outer_code = closure_outer.__code__
inner_code = closure_outer().__code__
print("outer co_cellvars:", outer_code.co_cellvars)
print("outer co_varnames:", outer_code.co_varnames)
print("inner co_freevars:", inner_code.co_freevars)


# --- the remaining co_* and frame attrs: type/shape correctness -----------
print("co_stacksize is int:", isinstance(co.co_stacksize, int))
print("co_code is bytes:", isinstance(co.co_code, bytes))
print("co_freevars is tuple:", isinstance(co.co_freevars, tuple))

f = sys._getframe()
print("f_lasti is int:", isinstance(f.f_lasti, int))
print("f_trace:", f.f_trace)


# --- generator gi_frame ---------------------------------------------------
def gen():
    x = 1
    yield x
    yield x + 1


g = gen()
print("gi_frame before start type:", type(g.gi_frame).__name__)
print("gi_frame before start f_lineno:", g.gi_frame.f_lineno)
next(g)
print("gi_frame suspended type:", type(g.gi_frame).__name__)
print("gi_frame suspended f_lineno:", g.gi_frame.f_lineno)
print("gi_frame co_name:", g.gi_frame.f_code.co_name)
print("gi_frame co_firstlineno:", g.gi_frame.f_code.co_firstlineno)
next(g)
print("gi_frame second f_lineno:", g.gi_frame.f_lineno)
# Exhaust the generator: gi_frame becomes None.
try:
    next(g)
except StopIteration:
    pass
print("gi_frame exhausted:", g.gi_frame)
