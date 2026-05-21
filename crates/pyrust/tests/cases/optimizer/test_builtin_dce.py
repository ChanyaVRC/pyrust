# Parity fixture for the pure-builtin dead call elimination pass.
#
# Calls to pure builtins whose result is never used are deleted by the
# optimizer when all arguments are compile-time constants.  Calls with
# runtime-expression arguments are preserved even when the result is dead,
# because the call may raise an exception (e.g. TypeError on a wrong-type arg)
# which is an observable side effect that must not be silently dropped.
# The observable behaviour (output) must be identical to CPython.

x = [1, 2, 3]

# Unused pure-builtin calls with all-constant args — dropped by the optimizer,
# no output produced, and the const values are valid so no exception occurs.
abs(-5)
id(x)
chr(65)
ord('A')
hash(42)

# A call whose result IS used — must survive.
n = len(x)
print(n)       # 3

# Calls with runtime-expression arguments must NOT be eliminated even when the
# result is dead.  The argument comes from a variable, so the optimizer cannot
# statically verify its type — the call may raise TypeError at runtime.
y = 5
try:
    len(y)   # TypeError: object of type 'int' has no len()
except TypeError:
    print("runtime-arg call preserved")

# Inside a try block: pure-builtin dead call must be kept regardless of
# argument kind, because the exception would be caught by the handler.
try:
    abs("not a number")   # TypeError
except TypeError:
    print("try-block call preserved")

# Side-effecting builtins are never removed regardless of result usage.
print("done")

print("ok")
