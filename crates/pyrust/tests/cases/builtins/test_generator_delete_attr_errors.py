def gen():
    yield 1

g = gen()

# Deleting __name__ raises TypeError (same as assigning a non-string)
try:
    del g.__name__
except TypeError as e:
    print(type(e).__name__, str(e))

# Deleting __qualname__ raises TypeError
try:
    del g.__qualname__
except TypeError as e:
    print(type(e).__name__, str(e))

# Deleting gi_running raises AttributeError "not writable"
try:
    del g.gi_running
except AttributeError as e:
    print(type(e).__name__, str(e))

# Deleting gi_frame raises AttributeError "not writable"
try:
    del g.gi_frame
except AttributeError as e:
    print(type(e).__name__, str(e))

# Deleting arbitrary attribute raises AttributeError "has no attribute"
try:
    del g.foo
except AttributeError as e:
    print(type(e).__name__, str(e))
