# Float / mixed-numeric fast path for the aug-assign (BinOpInPlace) and fused
# const/imm (BinOpConst / BinOpImm) handlers.  These don't carry the BinOp
# adaptive cache, so before this path `s += <float>` etc. went through
# eval_binary_aug.  All results must match CPython 3.12.

# --- Augmented assign on floats (BinOpInPlace: reg op reg) ---
s = 0.0
v = 1.5
for _ in range(5):
    s += v
    s -= 0.25
    s *= 1.1
    s /= 1.3
print("aug float reg:", repr(s))

# Aug with a float constant (BinOpConst: reg op const).
t = 100.0
for _ in range(5):
    t += 2.5
    t -= 0.5
    t *= 0.9
    t //= 1.0
    t %= 97.0
print("aug float const:", repr(t))

# Aug with an int immediate on a float lhs (BinOpImm: reg op int-imm → mixed).
u = 10.0
for _ in range(5):
    u += 3
    u -= 1
    u *= 2
    u //= 4
    u %= 7
print("aug mixed imm:", repr(u))

# int lhs augmented by a float → becomes float (mixed, both operand orders).
w = 7
w += 2.5
w *= 1.5
w -= 0.5
print("int += float:", repr(w))

# --- Const-folded plain binary ops (is_aug = False) on floats/mixed ---
# These reuse the same fast path; values must be identical to eval_binary.
xs = [1.0, 2.5, -3.5, 0.0, 10.25]
for x in xs:
    print("plain:", repr(x + 2.0), repr(x - 2.0), repr(x * 2.0), repr(x / 2.0))
    print("plain mixed:", repr(x + 3), repr(x * 3), repr(x - 3), repr(x // 3), repr(x % 3))

# Comparisons involving a float const must stay correct (fall through, exact).
for x in [2.0, 2.5, 3.0]:
    print("cmp:", x < 2.5, x == 2.5, x > 2.5, x <= 2.5, x >= 2.5, x != 2.5)

# Pow must fall through (mixed / float): float ** int, and neg base ** 0.5.
print("pow:", repr(2.0**3), repr(9.0**0.5))
b = [-8.0]
print("pow negbase:", repr(b[0] ** 2))  # even int exponent → real


# Div/mod/floordiv by float 0.0 must still raise (caught; traceback not compared).
def show_err(fn):
    try:
        print(fn())
    except Exception as e:
        print(type(e).__name__ + ": " + str(e))


z = [5.0, 0.0]
show_err(lambda: z[0] / z[1])
show_err(lambda: z[0] // z[1])
show_err(lambda: z[0] % z[1])
