# Verify that exceptions raised with no message have e.args == () rather than
# e.args == ('',).  CPython 3.12 never includes an empty string in args.

# Empty-message MemoryError from an internal named-exception path (str repeat).
try:
    "x" * (2**60)
except MemoryError as e:
    print(repr(e.args))

# ZeroDivisionError carries its message through (regression guard).
try:
    1 / 0
except ZeroDivisionError as e:
    print(repr(e.args))

# TypeError with a message is also preserved (regression guard).
try:
    round(1.5, "x")
except TypeError as e:
    print(repr(e.args))

# Raising MemoryError directly with no arguments.
try:
    raise MemoryError()
except MemoryError as e:
    print(repr(e.args))

# Raising MemoryError with an explicit message preserves that message.
try:
    raise MemoryError("out of memory")
except MemoryError as e:
    print(repr(e.args))

# StopIteration with no message.
try:
    raise StopIteration()
except StopIteration as e:
    print(repr(e.args))

# StopIteration with a message is preserved.
try:
    raise StopIteration("done")
except StopIteration as e:
    print(repr(e.args))
