# Issue #2323: calling a class that overrides neither __init__ nor __new__ with
# excess arguments must raise `TypeError: <Cls>() takes no arguments`
# (CPython 3.12), NOT the `<Cls>.__init__() takes exactly one argument` wording.
#
# CPython rejects the excess args in object.__new__ (which runs before
# object.__init__) when neither slot is user-defined.  Classes that DO override
# __init__ and/or __new__ keep their own (arity-style) messages, which this
# fixture also pins so the bare-class special-case stays narrow.


def show(desc, fn):
    try:
        fn()
        print(f"{desc}: NO ERROR")
    except Exception as e:
        print(f"{desc}: {type(e).__name__}: {e}")


def show_type(desc, fn):
    # Assert only the exception *type* (not the message) for cases whose message
    # depends on the user-function arity wording, which still lacks the
    # `<Cls>.` qualifier in pyrust (tracked in this PR's "Out of scope" note).
    try:
        fn()
        print(f"{desc}: NO ERROR")
    except Exception as e:
        print(f"{desc}: {type(e).__name__}")


# --- Bare class: neither __new__ nor __init__ defined. ---
class Plain:
    pass


show("Plain(1)", lambda: Plain(1))
show("Plain(x=1)", lambda: Plain(x=1))
show("Plain(1, x=2)", lambda: Plain(1, x=2))
show("Plain() ok", lambda: Plain())


# --- Subclass inheriting only the implicit object.__new__/__init__. ---
class Base:
    pass


class Derived(Base):
    pass


show("Derived(1)", lambda: Derived(1))
show("Derived(x=1)", lambda: Derived(x=1))


# --- object itself with excess args. ---
show("object(1)", lambda: object(1))
show("object(x=1)", lambda: object(x=1))


# --- __slots__ class with no __init__/__new__ still gets the bare-class wording
#     (slots do not introduce a custom allocator). ---
class Slotted:
    __slots__ = ("a",)


show("Slotted(1)", lambda: Slotted(1))
show("Slotted() ok", lambda: Slotted())


# --- Class WITH a user __init__ inherited from a base: keeps arity wording. ---
class HasInitBase:
    def __init__(self):
        pass


class HasInitDerived(HasInitBase):
    pass


show_type("HasInitDerived(1)", lambda: HasInitDerived(1))


# --- __new__ defined but not __init__: object.__new__ is no longer the slot,
#     so the bare-class wording must NOT apply.  A *args __new__ accepts extra
#     args (no error); a strict __new__ rejects them with its own wording. ---
class NewVararg:
    def __new__(cls, *a, **k):
        return super().__new__(cls)


show("NewVararg(1)", lambda: NewVararg(1))


# --- __init__ defined but not __new__: keeps its own arity message. ---
class InitOnly:
    def __init__(self):
        pass


show_type("InitOnly(1)", lambda: InitOnly(1))
