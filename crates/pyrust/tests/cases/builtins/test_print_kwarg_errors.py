# Parity fixture: print() unknown keyword argument raises TypeError with CPython message format.

# Single unknown keyword
try:
    print(unknown=1)
except TypeError as e:
    print(e)
except Exception as e:
    print(type(e).__name__ + ': ' + str(e))

# Unknown keyword alongside valid kwargs
try:
    print("x", end="\n", bad=3)
except TypeError as e:
    print(e)
except Exception as e:
    print(type(e).__name__ + ': ' + str(e))

# Happy path: valid kwargs still work
print("ok", end="\n")
print("hello", "world", sep=", ")
