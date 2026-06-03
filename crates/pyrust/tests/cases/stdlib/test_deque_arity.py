# Arity guards on deque methods (issue #1886 dedup).
# We assert the *exception class* only — the deque arity message text
# diverges between CPython and pyrust, so printing it would break parity.
from collections import deque


def cls(code):
    try:
        eval(code)
        return "OK"
    except Exception as e:
        return type(e).__name__


# no-argument methods reject extra positional args
print(cls("deque().pop(1)"))
print(cls("deque().popleft(1)"))
print(cls("deque().clear(1)"))
print(cls("deque().copy(1)"))
print(cls("deque().reverse(1)"))

# one-argument methods reject zero args
print(cls("deque().append()"))
print(cls("deque().appendleft()"))
print(cls("deque().extend()"))
print(cls("deque().extendleft()"))
print(cls("deque().count()"))
print(cls("deque().remove(0)") == "ValueError")  # remove(arg) on empty -> ValueError
print(cls("deque([1]).__contains__()"))

# one-argument methods reject extra args
print(cls("deque().append(1, 2)"))
print(cls("deque().count(1, 2)"))

# happy paths still work
d = deque([1, 2, 3])
d.append(4)
d.appendleft(0)
print(list(d))
print(d.pop(), d.popleft())
d.extend([9, 9])
d.extendleft([8, 8])
print(list(d))
print(d.count(9), 7 in d, d[0])
d.remove(9)
del d[0]
print(list(d))
d.reverse()
print(list(d))
print(list(d.copy()))
d.clear()
print(list(d), d.maxlen)
