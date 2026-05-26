# Empty tuple as a for-loop unpack target — issue #1266.
# CPython 3.12 accepts `for () in iterable:` as a zero-element unpack.

# Basic: loop body runs once for each empty-tuple element.
for () in [()]:
    print("x")
print("done")

# No iterations when the iterable is empty.
for () in []:
    print("should not print")
print("empty list ok")

# pass-only body is valid.
for () in [()]:
    pass
print("pass ok")

# Nested: ((),) is a one-element tuple whose element is an empty tuple.
for ((),) in [((),)]:
    print("nested")

# ValueError when element has too many values.
try:
    for () in [(1,)]:
        pass
except ValueError as e:
    print(e)

# ValueError propagates correctly across multiple iterations.
count = 0
try:
    for () in [(), (1,)]:
        count += 1
except ValueError as e:
    print(count, e)
