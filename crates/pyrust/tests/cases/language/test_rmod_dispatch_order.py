# Parity fixture for str % obj dispatch order (#1472).
#
# When the left operand is a str, str.__mod__ (printf formatting) must always
# run — rhs.__rmod__ must never be consulted.  CPython's binary_op1 only calls
# the reflected slot when the forward slot returns NotImplemented, but
# str.__mod__ never does.

class RMod:
    def __rmod__(self, fmt):
        return "RMOD"

obj = RMod()

# str % obj: must format obj, not return __rmod__ result
result_s = "val: %s" % obj
print(result_s != "RMOD")      # True
print(type(result_s).__name__)  # str
print("val:" in result_s)       # True

result_r = "val: %r" % obj
print(result_r != "RMOD")      # True
print("val:" in result_r)       # True

# __rmod__ that returns NotImplemented: str still formats normally, no TypeError
class RModNI:
    def __rmod__(self, fmt):
        return NotImplemented

obj2 = RModNI()
result2 = "x: %s" % obj2
print("x:" in result2)          # True

# Non-str lhs with __rmod__: reflected slot IS called
class MyNum:
    def __rmod__(self, other):
        return f"rmod({other})"

n = MyNum()
print(10 % n)   # rmod(10)

# Plain str formatting still works
print("%d" % 42)        # 42
print("%s" % "hello")   # hello
print("%s %s" % ("a", "b"))  # a b
