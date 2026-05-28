# Verify that assert constructs AssertionError with the correct args.
# CPython 3.12 parity test for issue #1237.

# No message: args must be empty tuple, str() must be ''
try:
    assert False
except AssertionError as e:
    print(repr(e.args))
    print(repr(str(e)))

# Integer message: raw value, not str(value)
try:
    assert False, 42
except AssertionError as e:
    print(repr(e.args))
    print(repr(str(e)))

# Tuple message: raw tuple, not repr(tuple)
try:
    assert False, ("a", "b")
except AssertionError as e:
    print(repr(e.args))

# None message: passed as raw None, not omitted
try:
    assert False, None
except AssertionError as e:
    print(repr(e.args))
    print(repr(str(e)))

# String message: must not regress
try:
    assert False, "msg"
except AssertionError as e:
    print(repr(e.args))

# True condition: must not raise
assert True
assert True, "not raised"
print("ok")
