# Regression coverage for #2257: the suspended-generator iterator buffer
# (`GeneratorFrame::iters`) is stored compactly (inline-1).  The dispatch loop
# and the suspended frame share the same `ItersBuf` type, so suspend/resume
# moves the buffer by value (`mem::take`) with no per-resume conversion; a frame
# with 2+ simultaneously-active for-loops simply spills its extra iterators to
# the heap.  Exercise every path where the iters buffer crosses the
# suspend/resume boundary so it stays correct: mid-for-loop suspension, nested
# for-loops (the spill case), yield-from, send/throw/close, StopIteration.value,
# and re-entrancy.


# --- suspended mid-for-loop, then resumed (1 active iter on the stack) ---
def for_gen(src):
    for x in src:
        yield x * 2
        yield x + 1


g = for_gen([10, 20, 30])
print([next(g) for _ in range(6)])


# --- nested for-loops: 2 iters on the stack at the yield (spill past inline-1) ---
def nested(outer, inner):
    for a in outer:
        for b in inner:
            yield (a, b)


print(list(nested([1, 2], ["x", "y"])))


# --- interleave many partially-advanced for-generators (the RSS workload) ---
gens = [for_gen([i, i + 1]) for i in range(5)]
print([next(g) for g in gens])  # each suspended mid-for-loop
print([next(g) for g in gens])  # resume each: same iter must survive


# --- yield-from chain (sub-iterator drive + StopIteration.value, PEP 380) ---
def inner_gen():
    for i in range(3):
        yield i
    return "done"


def outer_gen():
    val = yield from inner_gen()
    yield val


print(list(outer_gen()))


# --- send() into a generator suspended mid-for-loop ---
def echo(src):
    for x in src:
        got = yield x
        yield got


e = echo([1, 2])
print(e.send(None))  # -> 1
print(e.send("a"))  # -> "a"
print(e.send(None))  # -> 2
print(e.send("b"))  # -> "b"


# --- throw() into a generator suspended mid-for-loop ---
def guarded(src):
    for x in src:
        try:
            yield x
        except ValueError:
            yield "caught"


gg = guarded([1, 2, 3])
print(next(gg))  # 1
print(gg.throw(ValueError))  # caught
print(next(gg))  # 2


# --- close() while suspended mid-for-loop releases the iter buffer ---
def closeable(src):
    for x in src:
        try:
            yield x
        finally:
            print("cleanup", x)


c = closeable([7, 8])
print(next(c))  # 7
c.close()  # triggers finally -> cleanup 7


# --- re-entrancy: a generator that tries to advance itself ('already executing') ---
def recursive():
    for _ in range(3):
        yield next(self_ref)


self_ref = recursive()
try:
    next(self_ref)
except ValueError as exc:
    print("reentry:", exc)
