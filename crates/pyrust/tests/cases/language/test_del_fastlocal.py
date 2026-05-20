# Parity fixture for del semantics on module-scope fastlocals (issue #846).
#
# Three sub-cases:
#  1. `del x` on an unbound name raises NameError.
#  2. `del y` after assignment removes the name from globals().
#  3. `del __doc__` after assigning a pre-seeded dunder removes it from globals().

# 1. del on unbound raises NameError
try:
    del x
except NameError as e:
    print("NameError:", "x" in str(e))

# 2. del clears the fastlocal and removes from globals()
y = 42
del y
print("y" in globals())  # False

# 3. del on a pre-seeded / user-assigned dunder clears it from globals()
__doc__ = "hello"
del __doc__
print("__doc__" in globals())  # False

# 4. del then re-assign works correctly
z = 1
del z
z = 2
print(z)  # 2

# 5. Deleting a name and catching NameError on re-read
w = 10
del w
try:
    print(w)
except NameError:
    print("NameError on read after del")
