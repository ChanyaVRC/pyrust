# Parity fixture for the pure-builtin dead call elimination pass.
#
# Calls to pure builtins whose result is never used are deleted by the
# optimizer.  The observable behaviour (output) must be identical to CPython:
# no output difference, no spurious side effects.

x = [1, 2, 3]

# Unused pure-builtin calls — dropped by the optimizer, no output produced.
len(x)
abs(-5)
id(x)
chr(65)
ord('A')
hash(42)

# A call whose result IS used — must survive.
n = len(x)
print(n)       # 3

# Side-effecting builtins are never removed regardless of result usage.
print("done")

print("ok")
