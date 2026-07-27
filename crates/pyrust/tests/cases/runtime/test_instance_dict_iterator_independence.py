class Box:
    pass


def outcome(call):
    try:
        return ("value", call())
    except Exception as exc:
        return (type(exc).__name__, str(exc))


obj = Box()
obj.a = 1
obj.b = 2
obj.c = 3
proxy = vars(obj)

# A dict proxy is reusable iterable state, not an iterator itself.
print(type(proxy).__name__)
print(type(proxy) is dict)
proxy_dir = dir(proxy)
for method_name in (
    "keys",
    "items",
    "update",
    "__iter__",
    "__getitem__",
    "fromkeys",
):
    print(method_name, method_name in proxy_dir, hasattr(proxy, method_name))
print("instance_dict" in proxy_dir)
print(type(iter(proxy)).__name__)
print(iter(proxy) is proxy)
cursor = iter(proxy)
print(iter(cursor) is cursor)
print(next(proxy.__iter__()))
print(outcome(lambda: next(proxy)))
print(outcome(lambda: hash(proxy)))

# Cursors created from one proxy advance independently.
left = iter(proxy)
right = iter(proxy)
print(next(left), next(left))
print(next(right))
print(list(left))
print(list(right))

# Exhausting one traversal must not consume future traversals.
print(list(proxy))
print(list(proxy))
print([key for key in proxy])

# A size change is reported independently and remains latched on every live
# cursor, as it is for CPython dict iterators.
changed = Box()
changed.a = 1
changed.b = 2
changed_proxy = vars(changed)
first = iter(changed_proxy)
second = iter(changed_proxy)
print(next(first))
changed.c = 3
print(outcome(lambda: next(first)))
print(outcome(lambda: next(first)))
print(outcome(lambda: next(second)))

# Value replacement is not structural and remains legal.
values = Box()
values.a = 1
values.b = 2
value_cursor = iter(vars(values))
print(next(value_cursor))
values.b = 20
print(list(value_cursor))

# Same-size key replacement follows the live insertion order.
same_size = Box()
same_size.a = 1
same_size.b = 2
same_size.c = 3
same_cursor = iter(vars(same_size))
print(next(same_cursor))
del same_size.b
same_size.d = 4
print(list(same_cursor))

# Instance dictionaries use CPython's split-table ordering behavior: deleting
# an already-yielded key shifts the live cursor, while reinserting the terminal
# key after all entries were yielded does not revive an exhausted iterator.
shifted = Box()
shifted.a = 1
shifted.b = 2
shifted.c = 3
shifted_cursor = iter(vars(shifted))
print(next(shifted_cursor))
del shifted.a
shifted.d = 4
print(list(shifted_cursor))

terminal = Box()
terminal.a = 1
terminal.b = 2
terminal.c = 3
terminal_cursor = iter(vars(terminal))
print(next(terminal_cursor), next(terminal_cursor), next(terminal_cursor))
del terminal.c
terminal.c = 30
print(outcome(lambda: next(terminal_cursor)))

# Slot storage is outside the visible __dict__. Adding or removing it during a
# dict traversal must not shift or invalidate the logical key cursor.
class SlotIteratorBox:
    __slots__ = ("slot", "__dict__")


slot_removed = SlotIteratorBox()
slot_removed.slot = 10
slot_removed.a = 1
slot_removed.b = 2
slot_removed_cursor = iter(vars(slot_removed))
print(next(slot_removed_cursor))
del slot_removed.slot
print(list(slot_removed_cursor))

slot_added = SlotIteratorBox()
slot_added.a = 1
slot_added.b = 2
slot_added_cursor = iter(vars(slot_added))
print(next(slot_added_cursor))
slot_added.slot = 10
print(list(slot_added_cursor))

# Replacing __dict__ detaches the old real dict in CPython. A proxy obtained
# before replacement must likewise retain its original backing: its iterator
# and later writes cannot jump to the newly assigned dict.
detached = Box()
detached.a = 1
detached.b = 2
stale_proxy = vars(detached)
stale_cursor = iter(stale_proxy)
print(next(stale_cursor))
replacement = {"x": 10}
detached.__dict__ = replacement
print(
    stale_proxy,
    vars(detached),
    stale_proxy is replacement,
    vars(detached) is replacement,
)
print(list(stale_cursor))
stale_proxy["c"] = 3
print(stale_proxy, replacement, hasattr(detached, "c"))
print(stale_proxy["a"], list(stale_proxy.items()), len(stale_proxy))
print(
    stale_proxy.get("a"),
    stale_proxy.get("x"),
    stale_proxy == {"a": 1, "b": 2, "c": 3},
)
print(stale_proxy.setdefault("x", 99), stale_proxy, replacement)
print(stale_proxy.pop("b"), stale_proxy, replacement)
replacement["y"] = 20
print(list(stale_proxy), list(vars(detached)))

# A detached proxy for a slotted instance must clear only its old visible
# mapping. Slot storage survives and the replacement dict is left untouched.
class SlotBox:
    __slots__ = ("slot", "__dict__")


slotted = SlotBox()
slotted.slot = 7
slotted.visible = 1
stale_slotted_proxy = vars(slotted)
slot_replacement = {"x": 2}
slotted.__dict__ = slot_replacement
stale_slotted_proxy.clear()
print(
    stale_slotted_proxy,
    slot_replacement,
    slotted.slot,
    "slot" in slot_replacement,
)
