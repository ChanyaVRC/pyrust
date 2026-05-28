p = property(lambda self: None)

# Keyword arg errors
try:
    p.getter(x=1)
except TypeError as e:
    print(str(e))  # property.getter() takes no keyword arguments

try:
    p.setter(x=1)
except TypeError as e:
    print(str(e))  # property.setter() takes no keyword arguments

try:
    p.deleter(x=1)
except TypeError as e:
    print(str(e))  # property.deleter() takes no keyword arguments

# Arity errors
try:
    p.getter()
except TypeError as e:
    print(str(e))  # property.getter() takes exactly one argument (0 given)

try:
    p.getter(lambda: 1, lambda: 2)
except TypeError as e:
    print(str(e))  # property.getter() takes exactly one argument (2 given)
