# Issue #2335: CPython 3.12's object.__new__ / object.__init__ excess-argument
# rules.  Whether extra constructor args are accepted depends on which of
# __new__ / __init__ is overridden, and on whether the __new__ slot has ever
# been mutated at runtime (CPython's sticky slot_tp_new wrapper).


def show(label, fn):
    try:
        fn()
        print(label, "OK")
    except TypeError as e:
        print(label, "TypeError:", e)


# --- 4-quadrant matrix: which of __new__/__init__ is overridden ---

# Q1: neither overridden -> "<Cls>() takes no arguments"
class Q1:
    pass


show("Q1()", lambda: Q1())
show("Q1(1)", lambda: Q1(1))
show("Q1(x=1)", lambda: Q1(x=1))

# Q2: only __init__ overridden -> __init__ consumes args
class Q2:
    def __init__(self, x):
        self.x = x


show("Q2(1)", lambda: Q2(1))
show("Q2(1, 2)", lambda: Q2(1, 2))

# Q3: only __new__ overridden (accepts the arg)
class Q3:
    def __new__(cls, x):
        return super().__new__(cls)


show("Q3(1)", lambda: Q3(1))

# Q3b: only __new__ overridden but it does NOT accept the arg
class Q3b:
    def __new__(cls):
        return super().__new__(cls)


show("Q3b(1)", lambda: Q3b(1))

# Q4: both overridden
class Q4:
    def __new__(cls, x):
        return super().__new__(cls)

    def __init__(self, x):
        self.x = x


show("Q4(1)", lambda: Q4(1))


# --- sticky __new__ slot wrapper: del / reassign ---

# del own __new__, no custom __init__
class D1:
    def __new__(cls, *a):
        return super().__new__(cls)


del D1.__new__
show("D1(1)", lambda: D1(1))

# del own __new__, WITH custom __init__ -> still rejected by object.__new__
class D2:
    def __new__(cls, *a):
        return super().__new__(cls)

    def __init__(self, x):
        self.x = x


del D2.__new__
show("D2(1)", lambda: D2(1))

# assign object.__new__ explicitly, with custom __init__
class D3:
    def __new__(cls, *a):
        return super().__new__(cls)

    def __init__(self, x):
        self.x = x


D3.__new__ = object.__new__
show("D3(1)", lambda: D3(1))

# wrapped state is inherited: subclass of a class whose __new__ was del'd
class GP:
    def __new__(cls, *a):
        return super().__new__(cls)


del GP.__new__


class Mid(GP):
    pass


class Leaf(Mid):
    def __init__(self):
        pass


show("Leaf()", lambda: Leaf())
show("Leaf(9)", lambda: Leaf(9))

# reassigning a real (user) __new__ after del restores custom-new behaviour
class R1:
    def __new__(cls, *a):
        return super().__new__(cls)


del R1.__new__


def _r1_new(cls, *a):
    return object.__new__(cls)


R1.__new__ = _r1_new
show("R1(1)", lambda: R1(1))


# --- assigning the genuine object.__new__ does NOT wrap the slot ---
# CPython's update_one_slot keeps tp_new == object_new when the assigned value
# is the real object.__new__ and the class was not already wrapped, so the
# bare-class / __init__ rules apply (NOT the object.__new__() wording).

# A1: assign object.__new__, no custom __init__ -> "<Cls>() takes no arguments"
class A1:
    pass


A1.__new__ = object.__new__
show("A1(1)", lambda: A1(1))

# A2: assign object.__new__, WITH custom __init__ -> accepted (no error)
class A2:
    def __init__(self, x):
        self.x = x


A2.__new__ = object.__new__
show("A2(1)", lambda: A2(1))

# A3: but if a class-body __new__ existed first, the slot is already wrapped and
# re-assigning object.__new__ does NOT revert it -> object.__new__() wording.
class A3:
    def __new__(cls, *a):
        return super().__new__(cls)

    def __init__(self, x):
        self.x = x


A3.__new__ = object.__new__
show("A3(1)", lambda: A3(1))
