import contextlib
import os

# Available as an attribute and importable.
print(hasattr(contextlib, "chdir"))  # True
from contextlib import chdir

print(chdir.__name__)  # chdir

# Basic usage: cwd changes inside the block, restored after.
original = os.getcwd()
with contextlib.chdir("/tmp"):
    print(os.getcwd() == "/tmp")  # True

print(os.getcwd() == original)  # True (restored)

# Exception still restores directory and propagates.
try:
    with contextlib.chdir("/tmp"):
        print(os.getcwd() == "/tmp")  # True
        raise ValueError("oops")
except ValueError:
    print("caught")  # caught

print(os.getcwd() == original)  # True (restored even after exception)

# Nested usage restores in LIFO order.
with contextlib.chdir("/tmp"):
    inner = os.getcwd()
    with contextlib.chdir("/"):
        print(os.getcwd() == "/")  # True
    print(os.getcwd() == inner)  # True

print(os.getcwd() == original)  # True

print("contextlib.chdir ok")
