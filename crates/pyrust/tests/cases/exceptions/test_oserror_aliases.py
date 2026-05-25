# IOError and EnvironmentError are aliases for OSError in Python 3.3+.
# Both names must resolve to the identical class object (is-identity).

print(IOError is OSError)          # True
print(EnvironmentError is OSError)  # True

# Raising via alias creates an OSError instance.
try:
    raise IOError('via IOError')
except OSError as e:
    print('caught OSError from IOError:', e)

# Catching with alias catches an OSError raised directly.
try:
    raise OSError('via OSError')
except IOError as e:
    print('caught IOError from OSError:', e)

# EnvironmentError as raise target caught by OSError handler.
try:
    raise EnvironmentError('env problem')
except OSError as e:
    print('caught OSError from EnvironmentError:', e)

# EnvironmentError as except target catches OSError.
try:
    raise OSError('plain oserror')
except EnvironmentError as e:
    print('caught EnvironmentError from OSError:', e)

# Both aliases participate in the normal exception hierarchy.
print(issubclass(IOError, Exception))          # True
print(issubclass(EnvironmentError, Exception)) # True
print(issubclass(IOError, BaseException))      # True
