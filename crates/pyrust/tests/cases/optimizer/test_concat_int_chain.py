# Parity fixture for the int fast path in Insn::Concat (#2381).
#
# pass_concat_merge fuses *every* chain of BinOp(Add) with 3+ operands into a
# single Concat instruction, not just string concatenation. The non-string
# Concat fallback gained an i64 fast path; these cases pin its correctness and
# its boundaries (i64 overflow -> BigInt, mixed int/float, user __add__, errors)
# byte-for-byte against CPython 3.12.

# Plain int chains (the hot case: `return a+b+c` in a method body).
a, b, c, d, e = 1, 2, 3, 4, 5
print(a + b + c)              # 6
print(a + b + c + d)          # 10
print(a + b + c + d + e)      # 15

# Negative and mixed-sign int chains.
print(a + (-b) + c + (-d))    # -2
print((-1) + (-2) + (-3))     # -6

# i64::MAX overflow mid-chain -> BigInt promotion, exact value.
big = 9223372036854775807
print(big + 1 + 1)            # 9223372036854775809
print(1 + 2 + big)           # 9223372036854775810

# i64::MIN underflow.
nmin = -9223372036854775808
print(nmin + (-1) + (-1))    # -9223372036854775810

# Running sum overflows only after several ints accumulate.
print(9223372036854775800 + 100 + 100)   # 9223372036854776000

# BigInt as the leading operand: first as_int() fails -> slow fallback path.
huge = 10 ** 30
print(huge + 1 + 2 + 3)       # 1000000000000000000000000000006

# Mixed int + float: fast path bails at the float operand, eval_binary coerces.
print(1 + 2 + 3.5)            # 6.5
print(1.5 + 2 + 3)           # 6.5
print(1 + 2.0 + 3 + 4)       # 10.0

# Chain that ends in a TypeError must raise with CPython's exact wording.
try:
    print(1 + 2 + "x")
except TypeError as ex:
    print("TypeError:", ex)   # unsupported operand type(s) for +: 'int' and 'str'

# str leading, int trailing: different CPython message.
try:
    print("x" + 1 + 2)
except TypeError as ex:
    print("TypeError:", ex)   # can only concatenate str (not "int") to str

# List concatenation chain (non-int, non-str: pure eval_binary fallback).
print([1] + [2] + [3])        # [1, 2, 3]

# User __add__ participates correctly when reached via the fallback resume.
class Acc:
    def __init__(self, v):
        self.v = v
    def __add__(self, other):
        return Acc(self.v + (other.v if isinstance(other, Acc) else other))
    def __radd__(self, other):
        return Acc((other.v if isinstance(other, Acc) else other) + self.v)
    def __repr__(self):
        return f"Acc({self.v})"

# int + int + Acc: fast path accumulates 1+2, then resumes into __radd__.
print(1 + 2 + Acc(10))        # Acc(13)
# Acc + int + int: leading object -> slow fallback the whole way.
print(Acc(0) + 1 + 2 + 3)     # Acc(6)

# Chained add inside a hot method body (the original motivating regression).
class C:
    def m(self, a, b, c):
        return a + b + c

o = C()
print(o.m(10, 20, 30))        # 60
