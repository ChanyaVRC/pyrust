# Issue #2383: pass_concat_merge only fuses BinOp(Add) chains into Concat when
# the leading operand is statically known to be a string.  This fixture checks
# that the gated optimization keeps results identical to CPython for every
# operand-type combination — string chains, int chains, float chains, and the
# mixed-type error path.


# String literal chain (statically string → fused into Concat).
def str_literals():
    return "a" + "b" + "c" + "d"


# String chain through locals (also fused).
def str_locals():
    a = "foo"
    b = "bar"
    c = "baz"
    return a + b + c


# Int chain (NOT fused — must stay a plain BinOp chain, same result).
def int_chain(x, y, z):
    return x + y + z


# Float chain.
def float_chain():
    return 1.5 + 2.25 + 0.25 + 4.0


# Mixed str + int raises TypeError (parity backstop: Concat must not silently
# accept a non-string operand).
def mixed_str_int():
    a = "x"
    b = 1
    c = "z"
    return a + b + c


# Mixed int + str (the leading operand is an int, so the chain isn't fused; the
# BinOp still raises the right TypeError).
def mixed_int_str():
    a = 1
    b = "x"
    return a + b + "y"


print(str_literals())
print(str_locals())
print(int_chain(1, 2, 3))
print(int_chain(-5, 5, 100))
print(float_chain())

# Longer chains exercise the count window.
print("1" + "2" + "3" + "4" + "5" + "6")
print(10 + 20 + 30 + 40 + 50)

try:
    mixed_str_int()
except TypeError as e:
    print("TypeError:", e)

try:
    mixed_int_str()
except TypeError as e:
    print("TypeError:", e)
