# Binary-op fall-through TypeError messages must match CPython 3.12 verbatim
# (issue #1875): the typed "unsupported operand type(s) for OP: 'X' and 'Y'"
# form for +, -, *, @, and the specialised "can only concatenate X (not "Y")
# to X" form when the left operand of + is str / list / tuple.


def show(label, fn):
    try:
        fn()
        print(label, "-> OK")
    except TypeError as e:
        print(label, "->", e)


class StrSub(str):
    pass


class MyInt(int):
    pass


# --- "+" : specialised concatenation message for str / list / tuple LHS ---
show('"a" + 1', lambda: "a" + 1)
show("[1] + 1", lambda: [1] + 1)
show("(1,) + 1", lambda: (1,) + 1)
show("(1,) + [2]", lambda: (1,) + [2])
show('[1] + "x"', lambda: [1] + "x")
# RHS subclass name is preserved; LHS reports the base sequence type.
show("[1] + StrSub('x')", lambda: [1] + StrSub("x"))
show('"a" + MyInt(5)', lambda: "a" + MyInt(5))

# --- "+" : generic typed message when LHS is not a sequence ---
show('1 + "a"', lambda: 1 + "a")
show('True + "a"', lambda: True + "a")
show("True + None", lambda: True + None)
show("MyInt(5) + 'a'", lambda: MyInt(5) + "a")
show("1.0 + None", lambda: 1.0 + None)

# --- "+" : bytes concat error names the original RHS operand (bool / subclass
# must not collapse to int) ---
show("b'z' + 1", lambda: b"z" + 1)
show("b'z' + True", lambda: b"z" + True)
show("b'z' + MyInt(5)", lambda: b"z" + MyInt(5))

# --- "-" : typed message ---
show('1 - "a"', lambda: 1 - "a")
show('"a" - "b"', lambda: "a" - "b")
show("None - 1", lambda: None - 1)

# --- "*" : typed message and the sequence-repeat error ---
show("{1:2} * 3", lambda: {1: 2} * 3)
show('1.0 * "a"', lambda: 1.0 * "a")
show("[] * []", lambda: [] * [])
show('"a" * 1.5', lambda: "a" * 1.5)

# --- "@" : typed message (no built-in matmul) ---
show("1 @ 2", lambda: 1 @ 2)
show('"a" @ "b"', lambda: "a" @ "b")
show("None @ None", lambda: None @ None)

# --- fused-const path must agree with the non-fused (variable) path ---
a = [1]
b = 1
show("a + b (non-fused [1] + 1)", lambda: a + b)
s = "a"
one = 1
show("s + one (non-fused)", lambda: s + one)

# --- valid ops must still work (no regression) ---
print(1 + 2)
print("a" + "b")
print([1] + [2])
print((1,) + (2,))
print(1 * "ab")
print("ab" * 2)
print([1] * 3)
print(True + 1)
