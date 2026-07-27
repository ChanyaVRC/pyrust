# A deque iterator's structural-mutation counter is native implementation
# state. It must not leak as a Python list whose resize can invalidate the
# iterator guard.

from collections import deque


leak_probe = deque([1, 2])
leak_iterator = iter(leak_probe)
print("leak first:", next(leak_iterator))
try:
    leaked_state = leak_probe._state
except AttributeError:
    print("state read: hidden")
else:
    leaked_state.clear()
    leaked_state.append(0)
    print("state read: exposed")

try:
    leak_probe._state = [0]
except AttributeError:
    print("state write: rejected")
else:
    print("state write: accepted")


mutated = deque([1, 2])
iterator = iter(mutated)
print("mutation first:", next(iterator))
mutated.append(3)
try:
    next(iterator)
except RuntimeError as exc:
    print("mutation guard:", str(exc))


reinitialised = deque([1, 2])
iterator = iter(reinitialised)
print("reinit first:", next(iterator))
reinitialised.__init__([8, 9])
try:
    next(iterator)
except RuntimeError as exc:
    print("reinit guard:", str(exc))
