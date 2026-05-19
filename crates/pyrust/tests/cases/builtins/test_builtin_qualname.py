# Parity fixture for issue #662:
# __qualname__, __name__, and __module__ on builtin functions.

# --- Top-level builtins (builtin_function_or_method in CPython) ---
print(print.__qualname__)     # 'print'
print(print.__name__)         # 'print'
print(print.__module__)       # 'builtins'

print(len.__qualname__)       # 'len'
print(len.__name__)           # 'len'
print(len.__module__)         # 'builtins'

# --- hasattr checks ---
print(hasattr(print, '__qualname__'))  # True
print(hasattr(print, '__name__'))      # True
print(hasattr(print, '__module__'))    # True
print(hasattr(len, '__qualname__'))    # True

# --- Method-style builtins (method_descriptor in CPython) ---
# __qualname__ and __name__ work; __module__ raises AttributeError.
print(str.upper.__qualname__)   # 'str.upper'
print(str.upper.__name__)       # 'upper'
try:
    print(str.upper.__module__)
    print("no-error")
except AttributeError:
    print("AttributeError")

print(list.append.__qualname__)  # 'list.append'
print(list.append.__name__)      # 'append'
try:
    print(list.append.__module__)
    print("no-error")
except AttributeError:
    print("AttributeError")

# hasattr returns False for __module__ on method-style builtins
print(hasattr(str.upper, '__qualname__'))  # True
print(hasattr(str.upper, '__name__'))      # True
print(hasattr(str.upper, '__module__'))    # False
