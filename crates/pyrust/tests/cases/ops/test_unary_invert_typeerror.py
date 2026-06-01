# Unary `~` on a non-int operand must raise CPython 3.12's typed message
# "bad operand type for unary ~: '<type>'" (issue #1979) — the operand's type
# name in quotes, matching the unary +/- error shape, rather than the old
# fixed "... use integer" wording.


def show(label, fn):
    try:
        fn()
        print(label, "-> OK")
    except TypeError as e:
        print(label, "->", e)


# --- non-int builtins: type name must be quoted and exact ---
show("~(3+4j)", lambda: ~(3 + 4j))
show("~3.5j", lambda: ~3.5j)
show('~"x"', lambda: ~"x")
show("~[1,2]", lambda: ~[1, 2])
show("~3.0", lambda: ~3.0)
show("~None", lambda: ~None)
show("~{}", lambda: ~{})
show("~(1,2)", lambda: ~(1, 2))
show("~set()", lambda: ~set())
show('~b"x"', lambda: ~b"x")


# --- valid int / bool / bigint operands must still work (no regression) ---
print(~5)  # -6
print(~True)  # -2
print(~False)  # -1
print(~0)  # -1
print(~(-1))  # 0
print(~(10**40))  # bigint


# --- user-defined __invert__ still dispatches ---
class Inv:
    def __invert__(self):
        return "inverted"


print(~Inv())  # inverted
