# Regression for #2338: a multi-level `yield from` chain (top -> mid -> leaf)
# driven by a `for` loop must deliver the delegated values instead of panicking
# with `run_bytecode called on a non-generator function`.  The for-loop drives
# the top generator through the gen-drive trampoline; the top generator's
# `YieldFrom` must hand each value back to that consumer rather than returning a
# raw `Yielded` outcome up through the native `run_bytecode` entry point.


def leaf(n):
    i = 0
    while i < n:
        yield i
        i += 1


def mid(n):
    yield from leaf(n)


def top(n):
    yield from mid(n)


# Exact issue repro.
for x in top(3):
    print(x)


# Single-level delegation under for-drive (also tripped the assert).
def outer(n):
    yield from leaf(n)


for x in outer(3):
    print("single", x)


# Deep chain (5 levels) under for-drive.
def g4(n):
    yield from leaf(n)


def g3(n):
    yield from g4(n)


def g2(n):
    yield from g3(n)


def g1(n):
    yield from g2(n)


for x in g1(4):
    print("deep", x)


# Each level mixes its own bare yields with a `yield from`, for-driven.
def leaf2(n):
    for i in range(n):
        yield ("leaf", i)


def mid2(n):
    yield ("mid", "start")
    yield from leaf2(n)
    yield ("mid", "end")


def top2(n):
    yield ("top", "start")
    yield from mid2(n)
    yield ("top", "end")


for x in top2(2):
    print(x)


# `yield from` over a non-generator iterable in the middle of a chain.
def mid_list():
    yield from [10, 20, 30]


def top_list():
    yield from mid_list()


for x in top_list():
    print("list", x)


# PEP 380 return-value propagation through multiple levels (the for loop
# discards the final value but the intermediate `yield from` must observe it).
def leaf_ret(n):
    for i in range(n):
        yield i
    return "leaf-done"


def mid_ret(n):
    r = yield from leaf_ret(n)
    print("mid got", r)
    return "mid-done"


def top_ret(n):
    r = yield from mid_ret(n)
    print("top got", r)


for x in top_ret(2):
    print("ret", x)


# Empty chain under for-drive: no items, must not crash.
def empty_leaf():
    return
    yield


def empty_mid():
    yield from empty_leaf()


def empty_top():
    yield from empty_mid()


for x in empty_top():
    print("should-not-print", x)
print("empty done")
