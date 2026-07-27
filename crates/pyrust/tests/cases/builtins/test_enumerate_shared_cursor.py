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

# --- immutable element sources under the fused unpack ---
for i, c in enumerate("añ日", 1):
    print(i, c, len(c))
print(list(enumerate("a\U0001F600b")))
print(list(enumerate(b"\x00\x7f\xff", 1)))
print(list(enumerate(bytearray(b"ab"), 1)))
print(list(enumerate((), 7)))
print(list(enumerate("")))

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
