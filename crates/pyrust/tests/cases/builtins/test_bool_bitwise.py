# Bitwise &, |, ^ on two bool operands preserve the bool type (CPython 3.12).
# A single int operand makes the result int (int wins over bool).
# Shifts (<<, >>) are always int, even for bool operands.

# bool OP bool -> bool (value)
print(True & False)
print(True & True)
print(False & False)
print(True | False)
print(False | False)
print(True | True)
print(True ^ True)
print(True ^ False)
print(False ^ False)

# bool OP bool -> bool (type)
print(type(True & False) is bool)
print(type(True | False) is bool)
print(type(True ^ True) is bool)

# bool OP int / int OP bool -> int (int wins)
print(True & 1)
print(True & 0)
print(1 & True)
print(0 & True)
print(1 | True)
print(0 | True)
print(True ^ 1)
print(2 ^ True)

# mixed result type is int
print(type(True & 1) is int)
print(type(1 | True) is int)
print(type(True ^ 2) is int)

# shifts never yield bool
print(type(True << True) is int)
print(type(True >> True) is int)
print(True << True)

# augmented assignment preserves bool for bool OP bool
b = True
b &= False
print(b, type(b) is bool)
c = False
c |= True
print(c, type(c) is bool)
d = True
d ^= True
print(d, type(d) is bool)

# augmented assignment with an int operand yields int
e = True
e &= 1
print(e, type(e) is int)
