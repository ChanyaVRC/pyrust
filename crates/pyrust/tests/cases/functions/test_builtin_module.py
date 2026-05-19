# Parity fixture for issue #667: __module__ on builtin functions and method descriptors.
#
# CPython 3.12 distinguishes:
#   builtin_function_or_method (e.g. print, len): __module__ == 'builtins'
#   method_descriptor (e.g. str.upper, list.append): __module__ raises AttributeError

# Top-level builtins expose __module__ == 'builtins'.
print(print.__module__)   # builtins
print(len.__module__)     # builtins
print(repr.__module__)    # builtins

# hasattr reflects that top-level builtins have __module__.
print(hasattr(print, '__module__'))   # True
print(hasattr(len, '__module__'))     # True

# Method descriptors (dotted names) do NOT expose __module__.
try:
    _ = str.upper.__module__
except AttributeError as e:
    print(e)   # 'method_descriptor' object has no attribute '__module__'

try:
    _ = list.append.__module__
except AttributeError as e:
    print(e)   # 'method_descriptor' object has no attribute '__module__'

try:
    _ = str.lower.__module__
except AttributeError as e:
    print(e)   # 'method_descriptor' object has no attribute '__module__'

# hasattr reflects that method descriptors do not have __module__.
print(hasattr(str.upper, '__module__'))    # False
print(hasattr(list.append, '__module__'))  # False
