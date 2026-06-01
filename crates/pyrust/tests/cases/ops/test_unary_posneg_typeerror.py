# Unary `+`/`-` on an unsupported operand must raise CPython 3.12's typed
# message "bad operand type for unary +: '<type>'" / "... unary -: '<type>'"
# (issue #1989) — the operand's type name in quotes, matching the unary ~
# error shape, rather than the old fixed wording with no type name.


def show(label, fn):
    try:
        fn()
        print(label, "-> OK")
    except TypeError as e:
        print(label, "->", e)


# --- unsupported builtins: type name must be quoted and exact ---
show('+"x"', lambda: +"x")
show("+[1,2]", lambda: +[1, 2])
show("+None", lambda: +None)
show("+{}", lambda: +{})
show("+(1,2)", lambda: +(1, 2))
show("+set()", lambda: +set())
show('+b"y"', lambda: +b"y")
show("+frozenset()", lambda: +frozenset())
show("+bytearray()", lambda: +bytearray())
show("+range(3)", lambda: +range(3))

show('-"x"', lambda: -"x")
show("-[1,2]", lambda: -[1, 2])
show("-None", lambda: -None)
show("-{}", lambda: -{})
show("-(1,2)", lambda: -(1, 2))
show("-set()", lambda: -set())
show('-b"y"', lambda: -b"y")
show("-frozenset()", lambda: -frozenset())
show("-bytearray()", lambda: -bytearray())
show("-range(3)", lambda: -range(3))


# --- valid numeric operands must still work (no regression) ---
print(+5)  # 5
print(-5)  # -5
print(+5.0)  # 5.0
print(-True)  # -1
print(+(10**40))  # bigint preserved
print(-(10**40))  # bigint negated
print(-(3 + 4j))  # (-3-4j)
print(+(3 + 4j))  # (3+4j)


# --- user-defined __pos__ / __neg__ still dispatch ---
class P:
    def __pos__(self):
        return "pos"

    def __neg__(self):
        return "neg"


print(+P())  # pos
print(-P())  # neg


# --- PyInstance WITHOUT __pos__/__neg__ reports the class name ---
class NoDunder:
    pass


show("+NoDunder()", lambda: +NoDunder())
show("-NoDunder()", lambda: -NoDunder())
