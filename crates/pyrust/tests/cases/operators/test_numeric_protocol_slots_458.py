# Issue #458: unified 5-type NumericOps protocol slots in pyrust-core.
# Exercises the full matrix of (int, bool, float, BigInt, complex) across all
# 12 binary ops, unary (+, -, ~, abs, bool), rich comparisons (==, !=, <, <=,
# >, >=), builtins pow()/abs(), overflow promotion, precision edge cases, NaN,
# signed zero, and subclass error type names.


def show(label, fn):
    try:
        v = fn()
        print(f"{label}: {type(v).__name__}:{v!r}")
    except Exception as e:
        print(f"{label}: {type(e).__name__}: {e}")


BIG = 1 << 70
HUGE = 10**400
NAN = float("nan")
INF = float("inf")


class MyInt(int):
    pass


class MyFloat(float):
    pass


class MyComplex(complex):
    pass


# 1. Binary arithmetic (+, -, *, /, //, %, **) across all 5 numeric kinds.
samples = [
    ("int", 5),
    ("bool", True),
    ("float", 2.5),
    ("bigint", BIG),
    ("complex", 1 + 2j),
]

for la, a in samples:
    for lb, b in samples:
        show(f"{la}+{lb}", lambda a=a, b=b: a + b)
        show(f"{la}-{lb}", lambda a=a, b=b: a - b)
        show(f"{la}*{lb}", lambda a=a, b=b: a * b)
        show(f"{la}/{lb}", lambda a=a, b=b: a / b)
        show(f"{la}//{lb}", lambda a=a, b=b: a // b)
        show(f"{la}%{lb}", lambda a=a, b=b: a % b)

# 2. Power (** and 2-arg pow()) across all 5 kinds + special cases.
show("int**int_pos", lambda: 2**64)
show("int**int_neg", lambda: 2**-3)
show("bool**bool", lambda: True**False)
show("float**int", lambda: 2.5**3)
show("neg_float**half", lambda: (-1.0) ** 0.5)
show("neg_float**nan", lambda: (-2.0) ** NAN)
show("zero_float**neg", lambda: 0.0**-1)
show("zero_complex**0", lambda: 0j**0)
show("zero_complex**pos", lambda: 0j**2)
show("zero_complex**neg", lambda: 0j**-1)
show("zero_complex**imag", lambda: 0j**1j)
show("complex**int", lambda: (1 + 1j) ** 2)
show("complex**neg_int", lambda: (1 + 1j) ** -2)
show("complex**complex", lambda: (1 + 2j) ** (0.5 - 0.5j))
show("pow_complex_int", lambda: pow(1j, 2))
show("pow_int_complex", lambda: pow(2, 1j))
show("pow_0j_0", lambda: pow(0j, 0))
show("pow_0j_neg", lambda: pow(0j, -1))
show("pow_bool_bool", lambda: pow(True, True))

# 3. Overflow promotion & BigInt-to-float OverflowError.
show("i64_max+1", lambda: 9223372036854775807 + 1)
show("i64_min-1", lambda: -9223372036854775808 - 1)
show("i64_max*2", lambda: 9223372036854775807 * 2)
show("i64_min//-1", lambda: -9223372036854775808 // -1)
show("huge+float", lambda: HUGE + 1.0)
show("huge+complex", lambda: HUGE + 1j)
show("complex/huge", lambda: 1j / HUGE)
show("complex_div_0", lambda: (1 + 2j) / 0j)
show("complex_div_large", lambda: 1e200j / 1e200j)
show("complex_div_small", lambda: complex(1e-308, 1e-308) / complex(1e-308, 1e-308))
show("complex_div_inf", lambda: (1 + 2j) / complex(INF, 0))
show("complex_div_nan", lambda: (1 + 2j) / complex(NAN, 0))

# 4. Bitwise (&, |, ^) and shifts (<<, >>) across all 5 kinds.
show("bool&bool", lambda: True & False)
show("bool|bool", lambda: True | False)
show("bool^bool", lambda: True ^ True)
show("bool&int", lambda: True & 3)
show("int|bool", lambda: 2 | True)
show("bool^int", lambda: True ^ 3)
show("bigint&int", lambda: (BIG | 7) & 5)
show("bigint|int", lambda: BIG | 3)
show("bigint^int", lambda: (BIG | 3) ^ 1)
show("int<<80", lambda: 1 << 80)
show("bigint>>70", lambda: BIG >> 70)
show("bool<<bool", lambda: True << True)
show("neg_shift", lambda: 1 << -1)
show("float&int", lambda: 1.5 & 1)
show("bool|float", lambda: True | 1.5)
show("complex&int", lambda: 1j & 1)
show("complex|int", lambda: 1j | 1)
show("complex^int", lambda: 1j ^ 1)
show("complex<<int", lambda: 1j << 1)
show("complex>>int", lambda: 1j >> 1)

# 5. Unary (+, -, ~, abs, bool) across all 5 kinds + identity.
for name, val in [
    ("int", -5),
    ("i64_min", -9223372036854775808),
    ("bool_t", True),
    ("bool_f", False),
    ("float", -2.5),
    ("neg_zero", -0.0),
    ("nan", NAN),
    ("bigint", -BIG),
    ("complex", 3 - 4j),
    ("complex_zero", 0j),
]:
    show(f"+{name}", lambda v=val: +v)
    show(f"-{name}", lambda v=val: -v)
    show(f"~{name}", lambda v=val: ~v)
    show(f"abs({name})", lambda v=val: abs(v))
    show(f"bool({name})", lambda v=val: bool(v))

print("pos_nan_is_self:", (+NAN) is NAN)

# 6. Rich comparisons (==, !=, <, <=, >, >=) across all 5 kinds.
i53_plus_1 = (1 << 53) + 1
f53 = float(i53_plus_1)
print("int_float_eq_exact:", i53_plus_1 == f53, i53_plus_1 != f53, i53_plus_1 > f53, f53 < i53_plus_1)
print("int_complex_eq_exact:", i53_plus_1 == complex(f53, 0), i53_plus_1 != complex(f53, 0))
pow2_100 = 1 << 100
ten_30 = 10**30
print("bigint_complex_pow2:", pow2_100 == complex(float(pow2_100), 0))
print("bigint_complex_10_30:", ten_30 == complex(float(ten_30), 0))
print("bool_complex_eq:", True == (1 + 0j), False == 0j, True != 1j)
print("signed_zero_eq:", -0.0 == 0.0, -0.0 == 0, -0.0 == False, -0.0 == 0j)
print("nan_cmp:", NAN == NAN, NAN != NAN, NAN < 0, NAN <= 0, NAN > 0, NAN >= 0)
print("nan_complex_eq:", NAN == complex(NAN, 0), complex(NAN, 0) == complex(NAN, 0))
show("nan<str", lambda: NAN < "a")
show("nan<complex", lambda: NAN < 1j)
for op_name, fn in [
    ("<", lambda: 1j < 2),
    ("<=", lambda: 2 <= 1j),
    (">", lambda: 1j > 1.5),
    (">=", lambda: BIG >= 1j),
    ("bool<complex", lambda: True < 1j),
]:
    show(f"complex_cmp_{op_name}", fn)

# 7. Subclass diagnostic type-name preservation.
show("MyComplex//int", lambda: MyComplex(1, 2) // 2)
show("MyComplex%int", lambda: MyComplex(1, 2) % 2)
show("MyComplex<int", lambda: MyComplex(1, 2) < 2)
show("MyFloat&int", lambda: MyFloat(1.5) & 1)
show("~MyFloat", lambda: ~MyFloat(1.5))
show("MyComplex+MyInt", lambda: MyComplex(1, 2) + MyInt(3))
show("abs(MyComplex)", lambda: abs(MyComplex(3, 4)))
