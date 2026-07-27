# Parity fixture for dead-result builtin calls.
#
# Calls remain runtime operations even when their results are unused: the
# global binding may be replaced and the call itself may raise. The observable
# behaviour must remain identical to CPython.

x = [1, 2, 3]

# Valid unused builtin calls produce no output and do not raise.
abs(-5)
id(x)
chr(65)
ord('A')
hash(42)

# A call whose result IS used — must survive.
n = len(x)
print(n)       # 3

# A runtime-expression argument may raise even when the result is dead.
y = 5
try:
    len(y)   # TypeError: object of type 'int' has no len()
except TypeError:
    print("runtime-arg call preserved")

# Exceptions from dead-result calls remain catchable.
try:
    abs("not a number")   # TypeError
except TypeError:
    print("try-block call preserved")

# Side-effecting builtins are never removed regardless of result usage.
print("done")

print("ok")
