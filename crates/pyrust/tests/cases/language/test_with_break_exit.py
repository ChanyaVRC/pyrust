"""`with` cleanup on control-flow early exit (issue #2295).

A `break`/`continue`/`return` that leaves a `with` body must still run
`__exit__(None, None, None)`, in reverse order for multiple managers, and
without disturbing the normal-exit and exception (suppression) paths.
"""


class CM:
    def __init__(self, name):
        self.name = name

    def __enter__(self):
        print("enter", self.name)
        return self.name

    def __exit__(self, et, ev, tb):
        print("exit", self.name, et)
        return False


print("== break ==")
for i in range(3):
    with CM("A"):
        print("body", i)
        if i == 1:
            break
print("after break")

print("== continue ==")
for i in range(3):
    with CM("A"):
        if i == 1:
            continue
        print("body", i)
print("after continue")

print("== return ==")


def f():
    with CM("A"):
        return 7


print(f())

print("== return binds alias ==")


def g():
    with CM("V") as v:
        return v


print(g())

print("== nested break (reverse order) ==")
for i in range(2):
    with CM("A"), CM("B"):
        if i == 0:
            break
        print("body", i)
print("after nested break")

print("== nested return ==")


def h():
    with CM("A"), CM("B"):
        return "done"


print(h())

print("== normal fall-through still works ==")
for i in range(2):
    with CM("A"):
        print("body", i)

print("== exception path still works ==")
try:
    with CM("X"), CM("Y"):
        raise KeyError("k")
except KeyError:
    print("caught")

print("== exception suppression unchanged ==")


class Sup:
    def __enter__(self):
        return self

    def __exit__(self, et, ev, tb):
        print("sup exit", et)
        return True


with Sup():
    raise ValueError("boom")
print("after suppress")

print("== loop else with break ==")
for i in range(3):
    with CM("A"):
        if i == 1:
            break
else:
    print("else ran")
print("else skipped on break")

print("== loop else without break ==")
for i in range(2):
    with CM("A"):
        print("body", i)
else:
    print("else ran")
