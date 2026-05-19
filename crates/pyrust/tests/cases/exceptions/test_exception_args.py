# Verify that BaseException.args is a tuple, not a list (CPython 3.12 parity).

# Zero-arg: args should be an empty tuple
try:
    raise ValueError()
except ValueError as e:
    print(type(e.args).__name__)  # tuple
    print(e.args)                 # ()

# Single-arg: args should be a 1-tuple
try:
    raise ValueError("msg")
except ValueError as e:
    print(type(e.args).__name__)  # tuple
    print(e.args)                 # ('msg',)
    print(e.args[0])              # msg (indexing still works)

# Multi-arg: args should be a tuple of all arguments
try:
    raise ValueError("msg", 42)
except ValueError as e:
    print(type(e.args).__name__)  # tuple
    print(e.args)                 # ('msg', 42)
    print(len(e.args))            # 2

# type() is exactly tuple
try:
    raise RuntimeError("x")
except RuntimeError as e:
    print(type(e.args) is tuple)  # True
    print(e.args[0])              # x

# args works across different exception types
try:
    raise TypeError("bad type")
except TypeError as e:
    print(type(e.args).__name__)  # tuple
    print(e.args[0])              # bad type
