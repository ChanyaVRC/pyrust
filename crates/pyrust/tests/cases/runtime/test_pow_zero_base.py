def try_pow(base, exp):
    try:
        result = base ** exp
        print(f"{base!r} ** {exp!r} = {result!r}")
    except ZeroDivisionError as e:
        print(f"ZeroDivisionError: {e}")


# Zero float base with negative exponent — all should raise ZeroDivisionError
try_pow(0.0, -1)
try_pow(0.0, -0.5)
try_pow(0.0, -2)
try_pow(-0.0, -1)
try_pow(-0.0, -3)

# Zero int base with negative exponent — falls to float path, same message
try_pow(0, -1)
try_pow(0, -2)

# Positive exponent — should NOT raise
try_pow(0.0, 0)
try_pow(0.0, 1)
try_pow(0.0, 2)
try_pow(0.0, 0.5)

# Non-zero base with negative exponent — should NOT raise
try_pow(1.0, -1)
try_pow(2.0, -1)
try_pow(-1.0, -1)
