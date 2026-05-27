# Parity fixture for issue #1418:
# format(obj, non_empty_spec) raises TypeError (not ValueError) when the
# object has no custom __format__ (i.e. inherits object.__format__).


class Bare:
    pass


class WithFormat:
    def __format__(self, spec):
        return f"[{spec}]"


class SubBare(Bare):
    pass


# Non-empty spec on bare object → TypeError
try:
    format(Bare(), "d")
except TypeError as e:
    print("TypeError:", e)
except ValueError as e:
    print("ValueError:", e)

try:
    format(Bare(), "something")
except TypeError as e:
    print("TypeError:", e)
except ValueError as e:
    print("ValueError:", e)

# Subclass with no custom __format__ → TypeError
try:
    format(SubBare(), ".2f")
except TypeError as e:
    print("TypeError:", e)
except ValueError as e:
    print("ValueError:", e)

# Empty spec on bare object → ok (returns str(obj))
result = format(Bare(), "")
print("empty spec returns str:", isinstance(result, str))

# Class with custom __format__ is unaffected
print(format(WithFormat(), "d"))
print(format(WithFormat(), ""))

# Primitives with unknown spec → ValueError (not TypeError)
try:
    format(42, "q")
except ValueError as e:
    print("int ValueError:", e)
except TypeError as e:
    print("int TypeError:", e)

try:
    format(3.14, "q")
except ValueError as e:
    print("float ValueError:", e)
except TypeError as e:
    print("float TypeError:", e)

# Standard format specs on primitives still work
print(format(42, "d"))
print(format(3.14, ".2f"))
print(format("hi", ">5"))

# object.__format__ direct call with non-empty spec → TypeError
try:
    object.__format__(Bare(), "x")
except TypeError as e:
    print("object.__format__ TypeError:", e)
except ValueError as e:
    print("object.__format__ ValueError:", e)

# object.__format__ with empty spec → ok
result2 = object.__format__(Bare(), "")
print("object.__format__ empty ok:", isinstance(result2, str))
