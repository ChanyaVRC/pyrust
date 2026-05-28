# Parity fixture for class-level int/float numeric-tower attrs (issue #1617).
# CPython reference: 3.12+
# int.real / int.imag / int.numerator / int.denominator return getset_descriptor.
# int.conjugate / float.conjugate return method_descriptor.
# bool inherits int's descriptors (same repr, says 'int' objects).

# --- hasattr checks ---
print(hasattr(int, 'real'))         # True
print(hasattr(int, 'imag'))         # True
print(hasattr(int, 'numerator'))    # True
print(hasattr(int, 'denominator'))  # True
print(hasattr(int, 'conjugate'))    # True
print(hasattr(float, 'real'))       # True
print(hasattr(float, 'imag'))       # True
print(hasattr(float, 'conjugate'))  # True
print(hasattr(bool, 'real'))        # True
print(hasattr(bool, 'conjugate'))   # True

# --- repr format ---
print(int.real)         # <attribute 'real' of 'int' objects>
print(int.imag)         # <attribute 'imag' of 'int' objects>
print(int.numerator)    # <attribute 'numerator' of 'int' objects>
print(int.denominator)  # <attribute 'denominator' of 'int' objects>
print(int.conjugate)    # <method 'conjugate' of 'int' objects>
print(float.real)       # <attribute 'real' of 'float' objects>
print(float.imag)       # <attribute 'imag' of 'float' objects>
print(float.conjugate)  # <method 'conjugate' of 'float' objects>
# bool inherits from int — the descriptor still says 'int' objects
print(bool.real)        # <attribute 'real' of 'int' objects>
print(bool.conjugate)   # <method 'conjugate' of 'int' objects>

# --- type names ---
print(type(int.real).__name__)         # getset_descriptor
print(type(int.imag).__name__)         # getset_descriptor
print(type(int.numerator).__name__)    # getset_descriptor
print(type(int.denominator).__name__)  # getset_descriptor
print(type(int.conjugate).__name__)    # method_descriptor
print(type(float.real).__name__)       # getset_descriptor
print(type(float.imag).__name__)       # getset_descriptor
print(type(float.conjugate).__name__)  # method_descriptor

# --- instance access is unaffected ---
print((5).real)           # 5
print((5).imag)           # 0
print((5).numerator)      # 5
print((5).denominator)    # 1
print((5).conjugate())    # 5
print((5.5).real)         # 5.5
print((5.5).imag)         # 0.0
print((5.5).conjugate())  # 5.5

# --- method_descriptor is callable with an explicit instance ---
print(int.conjugate(5))     # 5
print(float.conjugate(5.5)) # 5.5
