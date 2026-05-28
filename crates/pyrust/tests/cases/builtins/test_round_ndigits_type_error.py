def try_round(num, ndigits):
    try:
        round(num, ndigits)
    except TypeError as e:
        print(f"TypeError: {e}")

# Non-integer ndigits raises TypeError naming the type
try_round(7, 1.5)
try_round(7, "x")
try_round(7, [])

# Valid ndigits work normally
print(round(7, 2))
print(round(7, None))
print(round(7))
