# Parity fixture for issue #389: globals() builtin.
#
# globals() returns a dict snapshot of the current module namespace.
# Mutations to the returned dict do NOT propagate back to the module
# namespace (known snapshot limitation, documented in PR #483).
#
# We check specific keys by name rather than printing the whole dict so
# that insertion-order differences between pyrust and CPython don't cause
# spurious failures.

x = 42
name_str = "hello"


def foo():
    # From inside a function, globals() still returns the module namespace,
    # not the function's local vars.
    g = globals()
    return g['x']


# Basic key lookup.
g = globals()
print(g['x'])           # 42
print(g['name_str'])    # hello

# Type is dict.
print(type(globals()).__name__)   # dict
print(isinstance(globals(), dict))  # True

# From inside a function.
print(foo())   # 42

# globals() does not expose local vars of the calling function.
def check_no_local_leak():
    local_var = 999
    g = globals()
    return 'local_var' not in g

print(check_no_local_leak())  # True

# Module-level names assigned after the first globals() call are visible
# in a fresh globals() call.
later_name = 77
print('later_name' in globals())  # True

# Function names appear in globals().
def a_function():
    pass

print('a_function' in globals())  # True

# globals() inside a class body returns the module namespace (not the
# class namespace — that is what locals() returns inside a class body).
class MyClass:
    g = globals()
    print(type(g).__name__)     # dict
    print('x' in g)            # True

# Iteration: keys are strings.
all_keys = list(globals().keys())
print(all(isinstance(k, str) for k in all_keys))  # True

# TypeError on any positional argument.
try:
    globals(1)
    print("no-error: FAIL")
except TypeError:
    print("globals(1) -> TypeError")

try:
    globals("a", "b")
    print("no-error: FAIL")
except TypeError:
    print("globals(a,b) -> TypeError")
