# Regression fixture for the `#[arity_style(...)]` typed-dialect modes
# (#2331 / #400).  Each migrated builtin must reproduce CPython's exact
# C-level argument-error wording byte-for-byte:
#
#   - `repr` / `hash` / `ord` / `chr`  → METH_O "takes exactly one
#     argument (N given)" for any positional count != 1.
#   - `isinstance` / `issubclass`      → METH_VARARGS "<name> expected 2
#     arguments, got N" (bare name, no trailing parens).
#
# Keyword arguments are still rejected with the "<name>() takes no keyword
# arguments" wording, which is unchanged by the new modes.


def show(expr):
    try:
        eval(expr)
    except TypeError as e:
        print(expr, "->", e)
    else:
        print(expr, "-> (no error)")


# takes_exactly_one: single-body builtins.
show("repr()")
show("repr(1, 2)")
show("repr(obj=1)")
show("hash()")
show("hash(1, 2)")
show("hash(x=1)")

# takes_exactly_one: overload-set builtins (shared dispatcher).
show("ord()")
show("ord('a', 'b')")
show("ord(x=1)")
show("chr()")
show("chr(1, 2)")
show("chr(x=1)")

# expected_got: METH_VARARGS bare-name wording.
show("isinstance(1)")
show("isinstance(1, int, 2)")
show("isinstance(x=1)")
show("issubclass(int)")
show("issubclass(int, object, 3)")
show("issubclass(cls=int)")

# Happy paths still work.
print(repr([1, 2]), hash(0), ord("A"), chr(66), ord(b"Z"))
print(isinstance(1, int), isinstance("x", (int, str)))
print(issubclass(bool, int), issubclass(int, str))
