def show(label, thunk):
    try:
        print(label, "OK", repr(thunk()))
    except Exception as exc:
        print(label, type(exc).__name__, str(exc))


# Re-declaring a slot in a subclass creates another physical member cell.
class A:
    __slots__ = ("x",)


class B(A):
    __slots__ = ("x",)


b = B()
A.x.__set__(b, "A-value")
B.x.__set__(b, "B-value")
show("A.x after both sets", lambda: A.x.__get__(b, B))
show("B.x after both sets", lambda: B.x.__get__(b, B))
B.x.__delete__(b)
show("A.x after deleting B.x", lambda: A.x.__get__(b, B))
show("B.x after deleting B.x", lambda: B.x.__get__(b, B))


# A user member descriptor may share a visible name with a BaseException
# native field without aliasing its storage.
class E(Exception):
    __slots__ = ("args",)


e = E("native")
show("E.args before member set", lambda: e.args)
show("str before member set", lambda: str(e))
E.args.__set__(e, ("member",))
show("E.args after member set", lambda: e.args)
show("str after member set", lambda: str(e))
E.args.__delete__(e)
show("E.args after member delete", lambda: e.args)
show("str after member delete", lambda: str(e))


# Compatible sibling layouts retain values by local slot name, even when the
# declarations use a different order and therefore distinct descriptors.
class Root:
    __slots__ = ()


class R1(Root):
    __slots__ = ("x", "y")


class R2(Root):
    __slots__ = ("y", "x")


r = R1()
r.x = "x-kept"
r.y = "y-kept"
r.__class__ = R2
print("forward retype", type(r).__name__, r.x, r.y)
r.__class__ = R1
print("reverse retype", type(r).__name__, r.x, r.y)
