try:
    tuple.__new__()
except TypeError as e:
    print(e)

try:
    frozenset.__new__()
except TypeError as e:
    print(e)

print(tuple.__new__(tuple))
print(tuple.__new__(tuple, [1, 2]))
print(frozenset.__new__(frozenset))
print(frozenset.__new__(frozenset, [1, 2]))
