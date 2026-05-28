class MyComplex(complex):
    pass

x = MyComplex(1+2j)
print(repr(x))   # (1+2j)
print(str(x))    # (1+2j)

y = MyComplex(0+1j)
print(repr(y))   # 1j

z = MyComplex(1+0j)
print(repr(z))   # (1+0j)

# Verify type name
print(type(x).__name__)   # MyComplex
