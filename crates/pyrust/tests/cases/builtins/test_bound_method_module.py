# Regression test for issue #713: __module__ on built-in bound methods
# should return None, not raise AttributeError.  CPython's
# builtin_function_or_method type does not set m_module, so __module__ is
# always None for bound methods on built-in types.

# __module__ is None and is accessible via hasattr
print([].append.__module__)        # None
print({}.get.__module__)           # None
print(set().add.__module__)        # None
print(hasattr([].append, '__module__'))  # True

# __name__ and __qualname__ are also exposed
print([].append.__name__)          # append
print([].append.__qualname__)      # list.append
print({}.get.__name__)             # get
print({}.get.__qualname__)         # dict.get

# __self__ returns the receiver
lst = [1, 2, 3]
m = lst.append
print(m.__self__ is lst)           # True

# No regression: the method is still callable
lst2 = []
lst2.append(42)
print(lst2)                        # [42]
