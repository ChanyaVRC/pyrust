"""Weakref callbacks fire as soon as the last strong reference in a dead
temporary register goes away (memoized dead-temp scrub), including when a
live ``__del__`` object coexists and across jumps/loops/calls."""

import weakref

log = []


class Plain:
    def __init__(self, tag):
        self.tag = tag


class Finalized:
    def __init__(self, tag):
        self.tag = tag

    def __del__(self):
        log.append(("del", self.tag))


def cb(tag):
    def inner(ref):
        log.append(("cb", tag))

    return inner


def make(tag, cls=Plain):
    o = cls(tag)
    return weakref.ref(o, cb(tag)), o.tag


def flush(title):
    print(title, log)
    log.clear()


# 1. Temporary holds the only reference; callback fires before next statement.
w = weakref.ref(Plain("a"), cb("a"))
log.append(("after-stmt", None))
flush("temp")

# 2. A temp consumed by a call then dropped.
def use(o):
    return o.tag


log.append(("next", None))
print(use(Plain("c")))
flush("call")

# 3. Callback inside a loop with a jump back edge (jump targets).
keep = weakref.ref(Plain("keep"))
holder = Plain("held")
hw = weakref.ref(holder, cb("held"))
for i in range(3):
    t = weakref.ref(Plain(i), cb(i))
    log.append(("iter", i))
flush("loop")

# 4. Branches: a temp that dies on one path only.
def branchy(flag):
    r = weakref.ref(Plain("br"), cb("br"))
    if flag:
        x = Plain("t1")
        rx = weakref.ref(x, cb("t1"))
        log.append(("in-branch", None))
    else:
        log.append(("else", None))
    return r


for flag in (True, False):
    branchy(flag)
    flush(f"branch {flag}")

# 5. Live weakref and live __del__ object together.
f = Finalized("fin")
fw = weakref.ref(f)
w2 = weakref.ref(Plain("d"), cb("d"))
log.append(("mid", None))
Finalized("tmp-del")
log.append(("after-del-temp", None))
log.append(("end", None))
flush("coexist")

# 6. Same with calls and a __del__ temp inside a function frame.
def both():
    Finalized("inner")
    log.append(("body", None))
    return use(Plain("h"))


print(both())
flush("both")

# 7. Strong refs still keep callbacks quiet.
live = Plain("live")
lw = weakref.ref(live, cb("live"))
log.append(("held", lw() is live))
del live
log.append(("released", lw() is None))
flush("held-then-released")

# 8. Dereferencing a dead weakref right after its temp died.
m = weakref.ref(Plain("m"), cb("m"))
log.append(("m-dead", m() is None))
flush("method")

del f
flush("final")
