"""member_descriptor receiver checks use owner identity, not owner name."""


def show_error(label, fn):
    try:
        fn()
    except TypeError as exc:
        print(label, exc)


class Owner:
    __slots__ = ("value",)


descriptor = Owner.value


class Child(Owner):
    pass


# A genuine subclass is accepted by every descriptor operation.
child = Child()
descriptor.__set__(child, 10)
print("subclass get", descriptor.__get__(child, Child))
descriptor.__delete__(child)
print("subclass deleted", hasattr(child, "value"))

# The descriptor retains the canonical owner and follows its live display name.
print("objclass", descriptor.__objclass__ is Owner)
Owner.__name__ = "RenamedOwner"
print("renamed repr", repr(descriptor))
print("renamed objclass", descriptor.__objclass__ is Owner)

# Same visible name, unrelated identity: direct descriptor calls all reject it.
Unrelated = type("RenamedOwner", (), {})
unrelated = Unrelated()
show_error("spoof get", lambda: descriptor.__get__(unrelated, Unrelated))
show_error("spoof set", lambda: descriptor.__set__(unrelated, 1))
show_error("spoof delete", lambda: descriptor.__delete__(unrelated))

# Copying the data descriptor onto that unrelated same-name class must not
# bypass the guard through ordinary attribute syntax either.
Unrelated.value = descriptor
show_error("automatic get", lambda: unrelated.value)
show_error("automatic set", lambda: setattr(unrelated, "value", 2))
show_error("automatic delete", lambda: delattr(unrelated, "value"))


class Ephemeral:
    __slots__ = ("slot",)


# Class-dict and class-attribute reads expose the same descriptor object.
retained = vars(Ephemeral)["slot"]
print(
    "descriptor identity",
    retained is Ephemeral.slot,
    retained is vars(Ephemeral)["slot"],
)

# A member_descriptor strongly retains its owner even after the last Python
# binding to that type is deleted. Internally this must remain cycle-free.
del Ephemeral
print("retained owner", retained.__objclass__.__name__)
print("retained repr", repr(retained))
