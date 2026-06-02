# Parity fixture for issue #2024:
# round(x) with no ndigits returns an int, so non-finite floats cannot be
# converted: ±inf raises OverflowError, NaN raises ValueError (matching
# int(float) / math.floor / math.ceil). The ndigits-given path returns a
# float and must propagate inf/nan unchanged.

def show(label, fn):
    try:
        print(label, repr(fn()))
    except OverflowError as e:
        print(label, "OverflowError:", e)
    except ValueError as e:
        print(label, "ValueError:", e)

# No ndigits → int conversion: non-finite raises.
show("round(inf)", lambda: round(float('inf')))
show("round(-inf)", lambda: round(float('-inf')))
show("round(nan)", lambda: round(float('nan')))

# None ndigits behaves like no ndigits.
show("round(inf, None)", lambda: round(float('inf'), None))
show("round(nan, None)", lambda: round(float('nan'), None))

# ndigits given → float result: inf/nan pass through unchanged.
show("round(inf, 2)", lambda: round(float('inf'), 2))
show("round(-inf, 2)", lambda: round(float('-inf'), 2))
show("round(nan, 2)", lambda: round(float('nan'), 2))

# Finite rounding is unaffected (banker's rounding preserved).
show("round(2.5)", lambda: round(2.5))
show("round(3.5)", lambda: round(3.5))
show("round(-0.5)", lambda: round(-0.5))
show("round(2.675, 2)", lambda: round(2.675, 2))
