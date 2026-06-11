# Regression fixture for the #400 batch-1 `#[arity_style(takes_exactly_one)]`
# migrations (abs / ascii / id / bin / oct / hex / callable).  Each of these
# is a METH_O builtin in CPython, so the wrong-arity wording must read
# `NAME() takes exactly one argument (N given)` for any positional count != 1,
# and keyword arguments are rejected with `NAME() takes no keyword arguments`.
#
# Before this batch these typed-dialect builtins emitted the argument-clinic
# wording (`missing required argument` / `takes 1 positional argument but N
# were given`), which diverged from CPython.  The fixture pins the fix.


def show(expr):
    try:
        eval(expr)
    except TypeError as e:
        print(expr, "->", e)
    else:
        print(expr, "-> (no error)")


for name in ("abs", "ascii", "id", "bin", "oct", "hex", "callable"):
    show(f"{name}()")
    show(f"{name}(1, 2)")
    show(f"{name}(1, 2, 3)")
    show(f"{name}(x=1)")

# Happy paths still work after the wording change.
print(abs(-5), abs(-2.5), abs(True))
print(bin(255), oct(8), hex(255), hex(True))
print(ascii("café"))
print(callable(len), callable(5))
print(isinstance(id(object()), int))
