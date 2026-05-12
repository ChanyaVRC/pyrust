# complex number type

# Imaginary literal
j = 1j
assert j == 1j
assert j.real == 0.0
assert j.imag == 1.0

# Mixed with int/float
z = 3 + 4j
assert z == complex(3, 4)
assert z.real == 3.0
assert z.imag == 4.0

# Floats with j
z2 = 2.5j
assert z2.real == 0.0
assert z2.imag == 2.5

# Constructors
assert complex() == 0j
assert complex(5) == 5 + 0j
assert complex(1, 2) == 1 + 2j
assert complex(2 + 3j) == 2 + 3j

# Arithmetic
a = 1 + 2j
b = 3 + 4j
assert a + b == 4 + 6j
assert a - b == -2 - 2j
assert a * b == complex(-5, 10)
# Division
assert (4 + 0j) / (2 + 0j) == 2 + 0j
# Real-complex mixed
assert 2 + a == 3 + 2j
assert a + 2 == 3 + 2j
assert 2 * a == 2 + 4j

# abs is the magnitude
assert abs(3 + 4j) == 5.0
assert abs(0j) == 0.0

# Negation
assert -(1 + 2j) == -1 - 2j

# conjugate
assert (3 + 4j).conjugate() == 3 - 4j
assert (1 - 2j).conjugate() == 1 + 2j

# Equality between complex and int/float
assert (5 + 0j) == 5
assert 5 == (5 + 0j)
assert (5 + 1j) != 5
assert (2.5 + 0j) == 2.5

# isinstance
assert isinstance(1j, complex)
assert not isinstance(1, complex)
assert not isinstance(1.0, complex)

# repr
assert repr(1j) == "1j"
assert repr(2 + 3j) == "(2+3j)"
assert repr(2 - 3j) == "(2-3j)"
assert repr(complex(0, 0)) == "0j"

# Truthiness
assert 1j
assert (0 + 1j)
assert not (0 + 0j)

# Division by zero
try:
    _ = (1 + 1j) / (0 + 0j)
    print("FAIL: expected ZeroDivisionError")
except ZeroDivisionError:
    pass

# Large magnitudes — repr should match CPython's scientific notation
assert repr(complex(1e20, 0)) == "(1e+20+0j)"
assert repr(complex(1e100, 1e100)) == "(1e+100+1e+100j)"
assert repr(complex(1e16, 0)) == "(1e+16+0j)"
# Integer-valued below 1e16 stays in integer form
assert repr(complex(1e15, 0)) == "(1000000000000000+0j)"
# Negative magnitudes
assert repr(complex(-1e20, 0)) == "(-1e+20+0j)"

print("complex OK")
