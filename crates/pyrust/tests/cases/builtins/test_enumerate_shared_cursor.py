# `for i, x in enumerate(seq)` runs an inline loop step, but the counter and the
# element position stay in the cells the enumerate object and its inner iterator
# own. These are the observations that pin that sharing: a loop must never move
# a cursor out of an object another name can still reach.

# --- resuming an aliased enumerate after `break` ---
e = enumerate([0, 1, 2, 3, 4])
for i, x in e:
    if i == 2:
        break
print(next(e))
for i, x in e:
    print("resumed", i, x)
print(list(e))

# --- next() interleaved with the loop consumes from the same cursor ---
e = enumerate("abcdef")
seen = []
for pair in e:
    seen.append(pair)
    try:
        seen.append(next(e))
    except StopIteration:
        seen.append("stop")
print(seen)

# --- the inner iterator stays shared: enumerate must not steal its position ---
it = iter([0, 1, 2, 3, 4, 5])
e = enumerate(it)
print(next(e), next(it))
for i, x in e:
    print("loop", i, x)
    if i == 2:
        break
print("inner left", list(it))

# --- two loops over one enumerate object continue, never restart ---
e = enumerate([0, 1, 2, 3], 10)
for i, x in e:
    for j, y in e:
        print("nested", i, x, j, y)
        break

# --- live sequence: the element walk observes mutation by index ---
xs = [0, 1, 2]
seen = []
for i, x in enumerate(xs):
    seen.append((i, x))
    if i == 0:
        xs.append(99)
print(seen, xs)

xs = [0, 1, 2, 3]
seen = []
for i, x in enumerate(xs):
    seen.append((i, x))
    if i == 1:
        del xs[0]
print(seen, xs)

xs = [0, 1, 2]
for i, x in enumerate(xs):
    xs[i] = x * 100
print(xs)

# --- every element source, driven through the loop ---
# `list(enumerate(...))` consumes the adapter directly and never reaches the
# loop step, so each source has to be walked by a `for` statement to pin the
# inline pair advance for that representation.
for i, c in enumerate("añ日", 1):
    print(i, c, len(c))
for src in ([1, 2], (3, 4), "a\U0001F600b", b"\x00\x7f\xff", bytearray(b"ab"),
            [], (), "", b"", bytearray()):
    walked = []
    for i, x in enumerate(src, 1):
        walked.append((i, x))
    print(type(src).__name__, walked)

# --- the counter promotes past the i64 boundary in the loop step too (#2125) ---
walked = []
for i, x in enumerate([10, 20, 30, 40], (1 << 63) - 2):
    walked.append(i)
print(walked)
for i, x in enumerate("ab", 1 << 70):
    print(i, x)
for i, x in enumerate((5, 6), -(1 << 64)):
    print(i, x)

# --- loop targets that must not take the fused two-register store ---
for pair in enumerate("ab", 1):
    print("single", pair, type(pair).__name__)
for i, *rest in enumerate([7, 8]):
    print("star", i, rest)
for i, (a, b) in enumerate([(1, 2), (3, 4)], 5):
    print("nested", i, a, b)
try:
    for i, x, y in enumerate([1, 2]):
        pass
except ValueError as exc:
    print("ValueError", exc)

# --- the loop step survives an exception and a generator suspension ---
e = enumerate([0, 1, 2, 3])
try:
    for i, x in e:
        if i == 1:
            raise RuntimeError("boom")
except RuntimeError as exc:
    print("caught", exc)
print(list(e))

def drive(seq):
    for i, x in enumerate(seq, 100):
        yield (i, x)

g = drive([1, 2, 3])
print(next(g))
print(list(g))

# --- sources that must stay on the generic adapter path ---
print(list(enumerate((i * i for i in range(3)), 1)))
print(list(enumerate(map(abs, [-1, -2]), 1)))
print(list(enumerate(reversed([1, 2, 3]), 1)))
print(sorted(enumerate(sorted({3, 4}))))


# --- a partly consumed inner iterator still enumerates from the start ---
it = iter([10, 20, 30, 40])
next(it)
for i, x in enumerate(it, 1):
    print("partial", i, x)


# --- a single-target loop yields a fresh tuple every iteration ---
# The two-target form writes registers directly and builds nothing; the
# single-target form must still hand out a distinct object each step, so a
# caller that keeps every pair never sees one of them change underneath it.
def all_distinct(items):
    for outer in range(len(items)):
        for inner in range(outer + 1, len(items)):
            if items[outer] is items[inner]:
                return False
    return True


for source in ([10, 20, 30], (10, 20, 30), "abc", b"abc", bytearray(b"abc")):
    kept = []
    for pair in enumerate(source):
        kept.append(pair)
    print("fresh", type(source).__name__, kept, all_distinct(kept), kept == list(enumerate(source)))

# Equal-valued pairs from two enumerates over equal sources are distinct objects.
left = [pair for pair in enumerate([0, 0])]
right = [pair for pair in enumerate([0, 0])]
print("cross fresh", left, left == right, left[0] is not right[0])

# A pair captured by a closure keeps the value it was created with.
captured = []
for pair in enumerate("xyz", 5):
    captured.append(lambda bound=pair: bound)
print("captured", [make() for make in captured])

# The single-target pair survives being stored into a container the loop keeps
# mutating, and is not reused as the next iteration's scratch.
box = []
for pair in enumerate([1, 2, 3, 4]):
    box.append(pair)
    box.append(pair[0])
print("boxed", box, all_distinct([entry for entry in box if isinstance(entry, tuple)]))


# ===========================================================================
# enumerate(range(...)): the same sharing obligations for a generated element
# source. The inner cursor is the range iterator's own, so nothing below may
# observe the loop having moved it out of an object another name can reach.
# ===========================================================================

# --- the inner range iterator stays shared ---
it = iter(range(6))
e = enumerate(it)
print("range alias", next(e), next(it))
for i, x in e:
    print("range loop", i, x)
    if i == 2:
        break
print("range inner left", list(it))

# --- break, then resume the same enumerate ---
e = enumerate(range(5))
for i, x in e:
    if i == 2:
        break
print("range after break", next(e))
for i, x in e:
    print("range resumed", i, x)
print("range drained", list(e))

# --- next() interleaved with the loop consumes from the same cursor ---
e = enumerate(range(6))
seen = []
for pair in e:
    seen.append(pair)
    try:
        seen.append(next(e))
    except StopIteration:
        seen.append("stop")
print("range interleaved", seen)

# --- two loops over one enumerate object continue, never restart ---
e = enumerate(range(4), 10)
for i, x in e:
    for j, y in e:
        print("range nested", i, x, j, y)
        break
print("range nested left", list(e))

# --- step, negative step, and zero-trip ranges ---
for src in (range(0), range(0, 0, -1), range(5, 5), range(1), range(0, 10, 4),
            range(10, 0, -3), range(-3, 3), range(2, -8, -5)):
    walked = []
    for i, x in enumerate(src, 1):
        walked.append((i, x))
    print("range walk", src.start, src.stop, src.step, walked)

# --- i64-boundary cursors, and the arbitrary-precision ranges beside them ---
top = 1 << 63
for src in (range(-top, -top + 3), range(top - 3, top - 1), range(-top, -top + 6, 2),
            range(top - 1, top - 7, -3), range(1 << 70, (1 << 70) + 3),
            range(-(1 << 70), -(1 << 70) + 3), range(0, 1 << 100, 1 << 99),
            range(top - 2, -top, -(1 << 62))):
    walked = []
    for i, x in enumerate(src):
        walked.append((i, x))
    print("range boundary", walked)

# --- the counter promotes past the i64 boundary over a range too (#2125) ---
walked = []
for i, x in enumerate(range(4), (1 << 63) - 2):
    walked.append(i)
print("range counter", walked)
for i, x in enumerate(range(2), 1 << 70):
    print("range bigstart", i, x)
for i, x in enumerate(range(2), -(1 << 64)):
    print("range negstart", i, x)
for i, x in enumerate(range(2), True):
    print("range boolstart", i, x)

# --- loop targets that must not take the fused two-register store ---
for pair in enumerate(range(3), 7):
    print("range single", pair, type(pair).__name__)
for i, *rest in enumerate(range(2)):
    print("range star", i, rest)
for i, (a, b) in enumerate(zip(range(2), range(10, 12)), 5):
    print("range nested target", i, a, b)
try:
    for i, x, y in enumerate(range(2)):
        pass
except ValueError as exc:
    print("range ValueError", exc)

# --- the loop step survives an exception and a generator suspension ---
e = enumerate(range(4))
try:
    for i, x in e:
        if i == 1:
            raise RuntimeError("boom")
except RuntimeError as exc:
    print("range caught", exc)
print("range after raise", list(e))


def drive_range(n):
    for i, x in enumerate(range(n), 100):
        yield (i, x)


g = drive_range(3)
print("range gen", next(g))
print("range gen rest", list(g))

# --- a partly consumed range iterator still enumerates from the start ---
it = iter(range(4))
next(it)
for i, x in enumerate(it, 1):
    print("range partial", i, x)

# --- reversed(range(...)) is a range cursor too ---
for i, x in enumerate(reversed(range(4)), 1):
    print("range reversed", i, x)
for i, x in enumerate(reversed(range(0, 9, 4))):
    print("range reversed step", i, x)

# --- a single-target loop over a range yields a fresh tuple every iteration ---
kept = []
for pair in enumerate(range(3)):
    kept.append(pair)
print("range fresh", kept, all_distinct(kept), kept == list(enumerate(range(3))))
