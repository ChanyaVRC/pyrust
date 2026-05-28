import sys

# Outside handler: both return None / (None, None, None)
print(sys.exc_info())
print(sys.exception())

# Inside handler
try:
    raise ValueError("test")
except ValueError as e:
    info = sys.exc_info()
    print(info[0] is ValueError)
    print(info[1] is e)
    # traceback is None (pyrust does not yet implement traceback objects)
    print(info[2] is None or type(info[2]).__name__ in ('traceback', 'NoneType'))

    exc = sys.exception()
    print(exc is e)

# After handler exits: cleared back to None
print(sys.exc_info())
print(sys.exception())

# Inside a nested handler: inner exception is visible
try:
    raise TypeError("outer")
except TypeError:
    print(type(sys.exception()).__name__)
    try:
        raise ValueError("inner")
    except ValueError:
        print(type(sys.exception()).__name__)
