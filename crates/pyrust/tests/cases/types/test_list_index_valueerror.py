# Parity fixture for issue #652:
# list.index() and tuple.index() must raise ValueError (not RuntimeError)
# when the target value is not found.

# list.index — value not found (unbounded)
try:
    [1, 2, 3].index(99)
except ValueError as e:
    print("list unbounded:", e)
except Exception as e:
    print("wrong class:", type(e).__name__, e)

# list.index — bounded window miss
try:
    [1, 2, 3].index(2, 0, 1)
except ValueError as e:
    print("list bounded:", e)
except Exception as e:
    print("wrong class:", type(e).__name__, e)

# list.index — empty list
try:
    [].index(1)
except ValueError as e:
    print("list empty:", e)
except Exception as e:
    print("wrong class:", type(e).__name__, e)

# tuple.index — value not found; check exception class only
# (CPython 3.12 message is "tuple.index(x): x not in tuple";
#  message wording parity is tracked separately by issue #658)
try:
    (1, 2, 3).index(99)
except ValueError:
    print("tuple ValueError raised")
except Exception as e:
    print("wrong class:", type(e).__name__, e)

# tuple.index — empty tuple
try:
    ().index(1)
except ValueError:
    print("tuple empty ValueError raised")
except Exception as e:
    print("wrong class:", type(e).__name__, e)

# except ValueError catches it; RuntimeError does not
caught_as_valueerror = False
try:
    [1, 2, 3].index(99)
except ValueError:
    caught_as_valueerror = True
print("caught as ValueError:", caught_as_valueerror)

caught_as_runtimeerror = False
try:
    [1, 2, 3].index(99)
except RuntimeError:
    caught_as_runtimeerror = True
except ValueError:
    pass  # expected — don't let it propagate
print("NOT caught as RuntimeError:", not caught_as_runtimeerror)

# Success cases — must not regress
print([1, 2, 3].index(2))       # 1
print([1, 2, 3].index(1, 0))    # 0
print((1, 2, 3).index(3))       # 2
print([1, 2, 3].index(1, 0, 3)) # 0
