# Parity fixture for issue #1268: str.format() {:f}/{:e}/{:g} with non-numeric
# arg must raise ValueError (not TypeError).
#
# CPython 3.12: str.__format__ checks the format code and raises ValueError for
# any float code ('f', 'e', 'g' and their uppercase / '%' variants).  pyrust
# previously let fmt_value_to_float raise TypeError instead.

# str values: all float format codes must raise ValueError
for code in ["f", "F", "e", "E", "g", "G", "%"]:
    fmt = "{:" + code + "}"
    try:
        fmt.format("hello")
    except ValueError as e:
        print(type(e).__name__ + ": " + str(e))
    except TypeError as e:
        print("WRONG TypeError: " + str(e))

# Numeric values still work correctly
print("{:f}".format(1))
print("{:f}".format(1.5))
print("{:e}".format(2.0))
print("{:g}".format(3.14))
print("{:f}".format(True))

# Int auto-converts to float
print("{:f}".format(0))
print("{:e}".format(-1))
