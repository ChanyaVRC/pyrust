# Parity fixture for issue #612: StopIteration subclasses must get a `.value`
# attribute set on construction, not just the exact StopIteration class.
#
# CPython 3.12: StopIteration.__init__ stores args[0] (or None) as .value.
# That behaviour must propagate to user-defined subclasses and deep chains.

# --- Exact class: no regression ---
s = StopIteration(99)
print(s.value)          # 99

s0 = StopIteration()
print(s0.value)         # None

# --- Direct subclass ---
class MyStop(StopIteration):
    pass

e = MyStop(42)
print(e.value)          # 42

e0 = MyStop()
print(e0.value)         # None

# --- Two-level subclass ---
class DeepStop(MyStop):
    pass

d = DeepStop(7)
print(d.value)          # 7

# --- Non-StopIteration exception must NOT get .value ---
try:
    _ = ValueError(42).value
    print("WRONG: ValueError has .value")
except AttributeError:
    print("ValueError has no .value")

class NotStop(Exception):
    pass

try:
    _ = NotStop(42).value
    print("WRONG: NotStop has .value")
except AttributeError:
    print("NotStop has no .value")

# --- Subclass caught in except block exposes .value ---
try:
    raise MyStop(100)
except StopIteration as exc:
    print(exc.value)    # 100
