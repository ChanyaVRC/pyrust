# Issue #1874: `list + <non-list>` must raise TypeError and must NOT mutate the
# left operand.  The bug only surfaced when the right operand was a compile-time
# constant that the optimizer fused into a BinOpConst/BinOpImm opcode, which was
# then mis-dispatched to the in-place `list.extend` fast path (treating `+` as
# `+=`).  Exercise every sibling RHS type, both fused-const and variable RHS,
# both literal-LHS and named-LHS, and confirm the LHS is never mutated.


def check_add(label, make_lhs, make_rhs):
    lhs = make_lhs()
    try:
        result = lhs + make_rhs()
        print(label, "ok:", result)
    except TypeError as e:
        print(label, "TypeError:", e)
    # The failed `+` must never have mutated the left operand.
    print(label, "lhs after:", lhs)


# --- const RHS (fused BinOpConst) — every sibling type must raise ---
check_add("list+str-const", lambda: [1, 2], lambda: "xy")
check_add("list+bytes-const", lambda: [1, 2], lambda: b"xy")
check_add("list+tuple-const", lambda: [1, 2], lambda: (3, 4))
check_add("list+frozenset-const", lambda: [1, 2], lambda: frozenset((3, 4)))
check_add("list+int-const", lambda: [1, 2], lambda: 5)
check_add("list+dict-const", lambda: [1, 2], lambda: {1: 2})

# --- literal LHS with const RHS (ensure_dst reuses the lhs temp) ---
print("literal [1]+1:")
try:
    print([1] + 1)
except TypeError as e:
    print("TypeError:", e)

# --- variable RHS (plain BinOp, not fused) — same TypeError ---
s = "xy"
check_add("list+str-var", lambda: [1, 2], lambda: s)

# --- valid list + list concatenates into a NEW list, no mutation ---
a = [1, 2]
b = a + [3, 4]            # const RHS
print("list+list-const:", b, "a:", a)
rhs = [3, 4]
c = a + rhs               # variable RHS
print("list+list-var:", c, "a:", a)

# --- augmented assign (`+=`) is unaffected: still extends in place ---
d = [1, 2]
d += "xy"                 # str is iterable
print("+= str:", d)
e = [1, 2]
e += (3, 4)
print("+= tuple:", e)
f = [1, 2]
f += [9]
print("+= list:", f)

# --- list * / *= still work; list * non-int raises ---
g = [1, 2]
print("list*2:", g * 2, "g:", g)
g *= 3
print("*= 3:", g)

# --- non-list sequence LHS: str / tuple concat error messages ---
for label, fn in (
    ("str+int", lambda: "a" + 1),
    ("tuple+list", lambda: (1,) + [2]),
    ("str+list", lambda: "a" + [1]),
):
    try:
        fn()
    except TypeError as ex:
        print(label, "TypeError:", ex)

# --- valid str/tuple concat unaffected ---
print("str+str:", "a" + "b")
print("tuple+tuple:", (1,) + (2,))
