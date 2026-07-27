# CPython retains one frame per recursive call, and pyrust exposes that stack
# through sys._getframe(), so recursive-call optimization must preserve it.

import sys


def descend(n, seen):
    frame = sys._getframe()
    depth = 0
    while frame is not None and frame.f_code.co_name == "descend":
        depth += 1
        frame = frame.f_back
    seen.append(depth)
    if n == 0:
        return seen
    return descend(n - 1, seen)


print(descend(3, []))
