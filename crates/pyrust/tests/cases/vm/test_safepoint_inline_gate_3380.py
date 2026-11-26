"""The weakref / ``__del__`` dead-temporary safepoint sits behind an inline gate
in the VM loop (#3380): weakref callbacks and finalizers must still run before
the next statement on every control-flow edge, across frames and generators,
and when the hooks are switched on in the middle of a running frame."""

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


def flush(title):
    print(title, log)
    log.clear()


def use(o):
    return o.tag


# 1. Hooks switched on in the middle of a running frame, then more statements.
def late_activation():
    pre = [Plain("pre")]
    log.append(("start", len(pre)))
    w = weakref.ref(Plain("late"), cb("late"))
    log.append(("after-ref", w() is None))
    f = Finalized("late-del")
    log.append(("made", f.tag))
    Finalized("temp-del")
    log.append(("after-temp", None))
    return use(Plain("ret"))


print(late_activation())
flush("late activation")

# 2. A weakref temp dying on each edge of a branch and a loop back edge.
anchor = weakref.ref(Plain("anchor"))
fin = Finalized("fin-anchor")


def edges(n):
    total = 0
    for i in range(n):
        if i % 2:
            held = weakref.ref(Plain(("odd", i)), cb(("odd", i)))
            log.append(("odd-body", i))
        else:
            Finalized(("even", i))
            log.append(("even-body", i))
        total += i
    log.append(("done", total))
    return total


print(edges(4))
flush("edges")


# 3. Exception handler edges.
def boom(o):
    raise ValueError(o.tag)


def handled():
    try:
        boom(Plain("exc-arg"))
    except ValueError as e:
        log.append(("caught", e.args[0]))
    try:
        held = weakref.ref(Plain("exc-temp"), cb("exc-temp"))
        boom(Finalized("exc-del"))
    except ValueError:
        log.append(("caught2", None))
    log.append(("after-try", None))


handled()
flush("exceptions")


# 4. Generators resumed between statements of the caller.
def gen():
    for i in range(3):
        yield weakref.ref(Plain(("g", i)), cb(("g", i)))
        Finalized(("g-del", i))
        log.append(("gen-resume", i))


for r in gen():
    log.append(("got", r() is None))
flush("generator")


# 5. Recursion (trampolined self-calls) with finalizable temporaries.
def recurse(n):
    if n == 0:
        return 0
    Finalized(("rec", n))
    log.append(("rec-body", n))
    return 1 + recurse(n - 1)


print(recurse(3))
flush("recursion")


# 6. Callee frame switches in a hot loop keep the caller's temps released.
def callee(a, b):
    return a + b


def hot():
    total = 0
    for i in range(5):
        total += callee(i, i)
        if i == 2:
            held = weakref.ref(Plain("mid"), cb("mid"))
            Finalized("mid-del")
            log.append(("mid", total))
    return total


print(hot())
flush("hot loop")

# 7. Once nothing is alive any more the program still behaves normally.
del fin
log.append(("fin-gone", None))
del anchor
print(sum(i for i in range(10)))
flush("end")
