# Setting attribute on int
try:
    x = 42
    x.foo = 1
except AttributeError as e:
    print(type(e).__name__, str(e))

# Setting attribute on str
try:
    x = "hello"
    x.bar = 2
except AttributeError as e:
    print(type(e).__name__, str(e))

# Deleting attribute on int
try:
    x = 42
    del x.foo
except AttributeError as e:
    print(type(e).__name__, str(e))

# Deleting attribute on list
try:
    x = [1, 2, 3]
    del x.foo
except AttributeError as e:
    print(type(e).__name__, str(e))
