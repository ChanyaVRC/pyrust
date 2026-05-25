class MySet(set):
    pass

s = MySet([1, 2])

# Each operator returns a plain set (not MySet), matching CPython 3.12.
print(s | {2, 3})       # {1, 2, 3}
print(s & {1, 3})       # {1}
print(s - {2})          # {1}
print(s ^ {2, 3})       # {1, 3}

# Both operands are subclass instances.
print(MySet([1]) | MySet([2]))   # {1, 2}
print(MySet([1, 2]) & MySet([2, 3]))  # {2}
print(MySet([1, 2]) - MySet([2]))     # {1}
print(MySet([1, 2]) ^ MySet([2, 3]))  # {1, 3}

# Subclass on the right only.
print({1, 2} | MySet([3]))   # {1, 2, 3}
print({1, 2} & MySet([2, 3]))  # {2}
print({1, 2} - MySet([2]))     # {1}
print({1, 2} ^ MySet([2, 3]))  # {1, 3}

# Plain set operations are unaffected.
print({1, 2} | {2, 3})   # {1, 2, 3}
print({1, 2} & {1, 3})   # {1}
print({1, 2} - {2})      # {1}
print({1, 2} ^ {2, 3})   # {1, 3}

# Empty subclass.
print(MySet() | {1, 2})   # {1, 2}
print(MySet([1, 2]) | MySet())  # {1, 2}
