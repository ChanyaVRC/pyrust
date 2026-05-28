# Tests that primitive type singletons (int, str, list, …) have object as
# their base, so __init_subclass__ and other object-level attrs are reachable
# via hasattr and attribute access.  Relates to issue #1537.

primitive_types = [int, str, list, dict, set, tuple, bytes, frozenset, bool, float, complex]

# hasattr must return True for every primitive type
for t in primitive_types:
    print(hasattr(t, '__init_subclass__'))  # True

# __init_subclass__ is callable and has the right name
print(int.__init_subclass__.__name__)    # __init_subclass__
print(str.__init_subclass__.__name__)    # __init_subclass__
print(type.__init_subclass__.__name__)   # __init_subclass__

# MRO for simple primitive: (int, object)
mro = type(5).__mro__
print(len(mro))          # 2
print(mro[0].__name__)   # int
print(mro[1].__name__)   # object

# bool's MRO: (bool, int, object)
bool_mro = bool.__mro__
print(len(bool_mro))           # 3
print(bool_mro[0].__name__)    # bool
print(bool_mro[1].__name__)    # int
print(bool_mro[2].__name__)    # object

# isinstance still works correctly after MRO fix
print(isinstance(True, int))    # True
print(isinstance(True, bool))   # True
print(isinstance(1, bool))      # False

# Subclassing a primitive still produces correct repr (not object.__repr__)
class MyList(list):
    pass

ml = MyList([1, 2, 3])
print(repr(ml))   # [1, 2, 3]
print(str(ml))    # [1, 2, 3]

class MyStr(str):
    pass

ms = MyStr("hello")
print(repr(ms))   # 'hello'
print(str(ms))    # hello

# __init_subclass__ fires as expected when subclassing
class MyInt(int):
    pass

print(MyInt(42))       # 42
print(MyInt(42) + 1)   # 43

# object attrs reachable from type too
print(hasattr(type, '__init_subclass__'))   # True
