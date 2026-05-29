# Parity fixture for CPython 3.12 error message wording.
# Issue #1779: super() AttributeError should say "'super' object has no attribute 'x'"

# ── super() AttributeError (issue #1779) ─────────────────────────────────────

class Foo:
    pass

try:
    super(Foo, Foo()).bar
except AttributeError as e:
    print(e)  # 'super' object has no attribute 'bar'

class Base:
    x = 1

class Child(Base):
    pass

try:
    super(Child, Child()).missing
except AttributeError as e:
    print(e)  # 'super' object has no attribute 'missing'

# Happy path: attribute that does exist should not raise.
class Parent:
    def greet(self):
        return "hi"

class Sub(Parent):
    def test(self):
        return super().greet()

print(Sub().test())  # hi

# ── iter(v, w) TypeError (issue #1780 — CPython says "iter(v, w): v must be callable") ──

try:
    iter(42, None)
except TypeError as e:
    print(e)  # iter(v, w): v must be callable

try:
    iter("not callable", 0)
except TypeError as e:
    print(e)  # iter(v, w): v must be callable

try:
    iter([], None)
except TypeError as e:
    print(e)  # iter(v, w): v must be callable

# Happy path: callable first argument should succeed (exhaust immediately here).
count = [0]

def stopper():
    count[0] += 1
    return count[0]

it = iter(stopper, 3)
print(list(it))  # [1, 2]
