# Parity fixture for unary + (Pos) on BigInt and unary - (Neg) on Bool.
# Issues: #498 (Pos missing BigInt arm), #500 (Neg missing Bool arm).

# Unary + on BigInt (issue #498)
print(+(2**64))           # 18446744073709551616
print(+(-2**64))          # -18446744073709551616
print(type(+(2**64)))     # <class 'int'>

# Unary + on small Int (unchanged)
print(+0)                 # 0
print(+1)                 # 1
print(+(-1))              # -1

# Unary + on Float (unchanged)
print(+1.5)               # 1.5

# Unary - on Bool (issue #500)
print(-True)              # -1
print(-False)             # 0
print(type(-True))        # <class 'int'>
print(-True == -1)        # True

# Unary - on BigInt (pre-existing, must stay correct)
print(-(2**64))           # -18446744073709551616
print(-(-(2**64)))        # 18446744073709551616
