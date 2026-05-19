# KeyboardInterrupt is a direct child of BaseException, not Exception.
# CPython 3.12 reference: https://docs.python.org/3/library/exceptions.html#KeyboardInterrupt

# Name and hierarchy
print(KeyboardInterrupt.__name__)
print(issubclass(KeyboardInterrupt, BaseException))
print(issubclass(KeyboardInterrupt, Exception))

# Can be raised and caught by its own name
try:
    raise KeyboardInterrupt
except KeyboardInterrupt:
    print("caught by KeyboardInterrupt")

# Can be caught by BaseException (its direct parent)
try:
    raise KeyboardInterrupt
except BaseException:
    print("caught by BaseException")

# Must NOT be caught by Exception
try:
    try:
        raise KeyboardInterrupt
    except Exception:
        print("WRONG: caught by Exception")
except KeyboardInterrupt:
    print("not caught by Exception")

# Can be raised with a message argument
try:
    raise KeyboardInterrupt("interrupted")
except KeyboardInterrupt as e:
    print(str(e))
