# Exercises float-float and str-str BinOp fast paths (issue #345).
# Runs enough iterations to warm up the adaptive inline cache and confirm
# that Specialized(Float) / Specialized(Str) paths produce correct results.

# --- float arithmetic ---
total = 0.0
for i in range(20):
    total = total + 1.5
print(total)  # 30.0

total = 10.0
for i in range(5):
    total = total - 0.5
print(total)  # 7.5

total = 1.0
for i in range(10):
    total = total * 2.0
print(total)  # 1024.0

total = 1024.0
for i in range(10):
    total = total / 2.0
print(total)  # 1.0

# --- float comparisons ---
count = 0
x = 1.5
for i in range(20):
    if x < 2.0:
        count += 1
print(count)  # 20

count = 0
for i in range(20):
    if x <= 1.5:
        count += 1
print(count)  # 20

count = 0
for i in range(20):
    if x > 1.0:
        count += 1
print(count)  # 20

# --- float edge cases (NaN, zero division) ---
import math
nan = float('nan')
print(nan == nan)   # False
print(nan < 1.0)    # False
print(nan > 1.0)    # False

# ZeroDivisionError must still raise after the division site is Specialized(Float).
# The divisors list has 8+ non-zero entries (to warm the cache) then a 0.0.
def div_by_zero_after_specialization():
    x = 2.0
    divisors = [1.0] * 20 + [0.0]  # 20 warms → Specialized, last triggers ZeroDivisionError
    caught = False
    for d in divisors:
        try:
            _ = x / d   # same BinOp instruction at fixed pc
        except ZeroDivisionError:
            caught = True
    print("ZeroDivisionError caught:", caught)  # True

div_by_zero_after_specialization()

# 0.0 ** negative similarly must raise after Pow site is Specialized(Float).
def pow_zero_negative_after_specialization():
    bases = [2.0] * 20 + [0.0]
    caught = False
    for b in bases:
        try:
            _ = b ** -1.0   # same BinOp instruction at fixed pc
        except ZeroDivisionError:
            caught = True
    print("ZeroDivisionError caught:", caught)  # True

pow_zero_negative_after_specialization()

# Deopt case: float followed by int (mixed types → Megamorphic)
def mixed_types():
    x = 1.5
    total = 0.0
    for i in range(10):
        total = total + x
    x = 1   # now int — cache should deopt gracefully
    total2 = 0
    for i in range(10):
        total2 = total2 + x
    print(total)   # 15.0
    print(total2)  # 10

mixed_types()

# --- str concatenation ---
s = ""
for i in range(10):
    s = s + "a"
print(s)  # aaaaaaaaaa
print(len(s))  # 10

# str comparison
parts = ["apple", "banana", "cherry"]
count = 0
for p in parts * 5:
    if p < "banana":
        count += 1
print(count)  # 5 (only "apple" < "banana")

# --- str deopt: mix str and int ---
def str_then_other():
    x = "hello"
    result = x + " world"  # str+str
    print(result)  # hello world
    # Now use a list add — different type, forces Megamorphic
    a = [1]
    b = [2]
    c = a + b
    print(c)  # [1, 2]

str_then_other()
