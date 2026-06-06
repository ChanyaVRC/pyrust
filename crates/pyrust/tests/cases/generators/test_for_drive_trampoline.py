# Generator trampoline (#2253): a `for`/comprehension consumer driving a plain,
# handler-free generator runs it within the consumer's dispatch loop (run to a
# `yield`, switch back) instead of re-entering the VM natively per element.  The
# behaviour must be byte-identical to the native resume path, including across
# yields, returns, nested generators, closures, exceptions, and early exit.


# --- 1. Basic for-over-generator ---
def squares(n):
    i = 0
    while i < n:
        yield i * i
        i += 1


acc = []
for v in squares(6):
    acc.append(v)
print(acc)

# Comprehensions (also compile to ForIter loops).
print([v for v in squares(5)])
print({v for v in squares(5)} == {0, 1, 4, 9, 16})
print({v: v + 1 for v in squares(4)})


# --- 2. Generator that returns a value (StopIteration.value via yield from) ---
def with_return():
    yield 10
    yield 20
    return 99


def delegate():
    r = yield from with_return()
    print("returned", r)


print(list(delegate()))


# --- 3. Nested generators: for-over-gen whose body fors-over another gen ---
def inner(n):
    for i in range(n):
        yield i


def outer(n):
    for x in inner(n):
        yield x * 10


print(list(outer(5)))


def deep(n, d):
    if d == 0:
        for i in range(n):
            yield i
    else:
        for x in deep(n, d - 1):
            yield x + 1


print(list(deep(4, 6)))


# --- 4. Closures / nonlocal / global captured by a driven generator ---
def make(base):
    def g():
        for i in range(3):
            yield base + i

    return g()


print(list(make(100)))


def with_nonlocal():
    total = 0

    def g():
        nonlocal total
        for i in range(4):
            total += i
            yield total

    seq = [x for x in g()]
    return seq, total


print(with_nonlocal())


COUNTER = 0


def uses_global():
    global COUNTER
    for _ in range(3):
        COUNTER += 1
        yield COUNTER


print(list(uses_global()), COUNTER)


# --- 5. Empty generator and immediate-return generator ---
def empty():
    return
    yield  # unreachable


print(list(empty()))
print([x for x in empty()])


# --- 6. Early exit: consumer breaks; generator left suspended, still usable ---
def naturals():
    i = 0
    while True:
        yield i
        i += 1


first = []
g = naturals()
for v in g:
    if v >= 4:
        break
    first.append(v)
print(first)
# The same generator can keep going from where it stopped.
print(next(g))
print(next(g))


# --- 7. Exception raised inside a driven generator, caught by the consumer ---
def raises_midway():
    yield 1
    yield 2
    raise ValueError("mid")
    yield 3  # unreachable


caught = None
out = []
try:
    for v in raises_midway():
        out.append(v)
except ValueError as e:
    caught = str(e)
print(out, caught)


# A try/except *around* the loop, generator itself handler-free.
def total_or_zero(it):
    s = 0
    try:
        for v in it:
            s += v
    except ValueError:
        return -1
    return s


print(total_or_zero(squares(4)))
print(total_or_zero(raises_midway()))


# --- 8. PEP 479: bare StopIteration escaping a generator → RuntimeError ---
def leaks_stop():
    yield 1
    raise StopIteration


try:
    print([x for x in leaks_stop()])
except RuntimeError:
    print("pep479 RuntimeError")


# --- 9. A driven generator that itself makes ordinary function calls ---
def helper(x):
    return x * 2 + 1


def calls_out(n):
    for i in range(n):
        yield helper(i)


print(list(calls_out(5)))


# --- 10. sum()/list()/tuple()/max() over a generator (mixed consumers) ---
print(sum(squares(10)))
print(tuple(squares(4)))
print(max(squares(5)))


# --- 11. Generator with try/except inside (native fallback) still correct ---
def safe(n):
    for i in range(n):
        try:
            if i == 2:
                raise ValueError
            yield i
        except ValueError:
            yield -1


print([x for x in safe(5)])


# --- 12. zip / enumerate over driven generators ---
print(list(enumerate(squares(4))))
print(list(zip(squares(3), "abc")))
