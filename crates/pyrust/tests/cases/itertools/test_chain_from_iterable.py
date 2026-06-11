# itertools.chain.from_iterable — native generator-state iterator (#2362).
# Covers arg edge cases, laziness of the outer source, exception propagation,
# exhaustion semantics, type identity, and nesting.

from itertools import chain, count, islice

# --- Arg edge cases ---
print(list(chain.from_iterable([])))                       # zero inners -> []
print(list(chain.from_iterable([[], [1], [], [2, 3], []])))  # empty inners interleaved
print(list(chain.from_iterable(["ab", "", "c"])))          # strings as inners
print(sorted(chain.from_iterable([{1: 1, 2: 2}.keys(), {3: 3}.values()])))  # dict views

# Wrong argument count.
try:
    chain.from_iterable([1], [2])
except TypeError as e:
    print(e)

# Non-iterable single argument -> TypeError at construction (iter() of it).
try:
    chain.from_iterable(5)
except TypeError as e:
    print("ctor:", e)

# --- iter(obj) is obj; type / metadata ---
it = chain.from_iterable([[1]])
print(iter(it) is it)               # True
print(type(it).__name__)            # "chain"

# --- Laziness: outer consumed one inner at a time, with side effects ---
def gen(n, log):
    log.append(("start", n))
    for i in range(n):
        yield i
    log.append(("end", n))


log = []


def outer():
    for n in (2, 3):
        log.append(("outer", n))
        yield gen(n, log)


it = chain.from_iterable(outer())
print(next(it), log)                # 0; only the first inner started
print(next(it), log)                # 1; still only first inner
print(next(it), log)                # 0; first inner exhausted, second started
print(list(it), log)               # [1, 2]; fully drained

# Infinite outer, partial consumption — laziness must hold.
def inf():
    n = 0
    while True:
        yield [n, n + 10]
        n += 1


print(list(islice(chain.from_iterable(inf()), 5)))  # [0, 10, 1, 11, 2]

# --- Exhaustion stays exhausted ---
it2 = chain.from_iterable([[1]])
print(next(it2))
try:
    next(it2)
except StopIteration:
    print("StopIteration")
try:
    next(it2)
except StopIteration:
    print("still StopIteration")

# --- Non-iterable inner raises TypeError only when reached ---
it3 = chain.from_iterable([[1], 5, [2]])
print(next(it3))
try:
    next(it3)
except TypeError as e:
    print("inner TypeError:", e)

# --- Inner generator raising mid-iteration propagates with type/message ---
def boom():
    yield 1
    raise ValueError("kaboom")


it4 = chain.from_iterable([boom(), [9]])
print(next(it4))
try:
    next(it4)
except ValueError as e:
    print("ValueError:", e)

# --- Consumers other than for/next: list / sum ---
print(sum(chain.from_iterable([[1, 2], [3, 4]])))           # 10
print(list(chain.from_iterable(g for g in ([1], [2], [3]))))  # [1, 2, 3]

# --- Nesting chain.from_iterable inside itself / chain / enumerate ---
print(list(chain.from_iterable(chain.from_iterable([[[1, 2]], [[3]]]))))  # [1, 2, 3]
print(list(chain(chain.from_iterable([[1], [2]]), [3])))                  # [1, 2, 3]
print(list(enumerate(chain.from_iterable([[10], [20]]))))                 # [(0, 10), (1, 20)]
