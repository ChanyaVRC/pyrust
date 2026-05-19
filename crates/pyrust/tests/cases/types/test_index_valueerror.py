# Regression test for issue #652:
# list.index() and tuple.index() must raise ValueError, not RuntimeError,
# when the target value is not found.

# list.index — value not found
try:
    [1, 2, 3].index(99)
except ValueError as e:
    print(e)           # 99 is not in list
except Exception as e:
    print("wrong class:", type(e).__name__, e)

# list.index — bounded search, value not in window
try:
    [1, 2, 3].index(2, 0, 1)
except ValueError as e:
    print(e)           # 2 is not in list
except Exception as e:
    print("wrong class:", type(e).__name__, e)

# tuple.index — value not found; check exception class only
# (message wording differs between implementations)
try:
    (1, 2, 3).index(99)
except ValueError:
    print("tuple ValueError raised")
except Exception as e:
    print("wrong class:", type(e).__name__, e)

# Success cases must still work
print([1, 2, 3].index(2))     # 1
print([1, 2, 3].index(1, 0))  # 0
print((1, 2, 3).index(3))     # 2
