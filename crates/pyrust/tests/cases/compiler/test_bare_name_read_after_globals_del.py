import sys


direct = 1
del direct
try:
    direct
except NameError as exc:
    print("direct:", type(exc).__name__)
else:
    print("direct: NO ERROR")

through_globals = 2
del globals()["through_globals"]
try:
    through_globals
except NameError as exc:
    print("globals:", type(exc).__name__, str(exc))
else:
    print("globals: NO ERROR")

frame = sys._getframe()
through_frame = 3
del frame.f_locals["through_frame"]
try:
    through_frame
except NameError as exc:
    print("frame:", type(exc).__name__, str(exc))
else:
    print("frame: NO ERROR")

consumed = 4
del globals()["consumed"]
try:
    result = consumed
except NameError as exc:
    print("consumed:", type(exc).__name__)
else:
    print("consumed: NO ERROR", result)

len = "shadowed"
del globals()["len"]
try:
    len
except NameError:
    print("builtin fallback: NameError")
else:
    print("builtin fallback: resolved")

rebound = 5
del globals()["rebound"]
rebound = 6
print("rebound:", rebound)
