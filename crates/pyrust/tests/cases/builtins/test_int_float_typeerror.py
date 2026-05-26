# Parity fixture: int() and float() raise TypeError for unsupported argument types.
# CPython 3.12 reference: TypeError with a message that names the bad type.

def check(expr, exc_type, msg_fragment):
    try:
        expr()
        print("FAIL: no exception for " + msg_fragment)
    except BaseException as e:
        t = type(e).__name__
        m = str(e)
        if t != exc_type:
            print("FAIL type: expected " + exc_type + " got " + t + " for " + msg_fragment)
        elif msg_fragment not in m:
            print("FAIL msg: expected fragment '" + msg_fragment + "' in '" + m + "'")
        else:
            print("OK: " + t + ": " + m)

# int() with a list
check(lambda: int([]), "TypeError", "int() argument must be a string, a bytes-like object or a real number, not 'list'")

# int() with a dict
check(lambda: int({}), "TypeError", "int() argument must be a string, a bytes-like object or a real number, not 'dict'")

# int() with complex
check(lambda: int(1+2j), "TypeError", "int() argument must be a string, a bytes-like object or a real number, not 'complex'")

# float() with a list
check(lambda: float([]), "TypeError", "float() argument must be a string or a real number, not 'list'")

# float() with a dict
check(lambda: float({}), "TypeError", "float() argument must be a string or a real number, not 'dict'")

# float() with complex
check(lambda: float(1+2j), "TypeError", "float() argument must be a string or a real number, not 'complex'")

# int() happy paths still work
print("int happy:", int("42"), int(3.9), int(True))

# float() happy paths still work
print("float happy:", float("3.14"), float(2), float(False))
