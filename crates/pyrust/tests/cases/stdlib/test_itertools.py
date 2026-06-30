# itertools.chain / islice — both lazy iterators in pyrust.

from itertools import chain, islice, repeat, count, cycle


# Helper for the laziness probe below — counts how many `__next__` calls
# reach the underlying source.
class CountingSource:
    def __init__(self):
        self.pulled = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.pulled += 1
        return self.pulled

# --- chain: concatenate ---
print("chain-3-lists", list(chain([1, 2], [3, 4], [5])))
print("chain-empty", list(chain([])))
print("chain-no-args", list(chain()))
print("chain-mixed", list(chain('ab', [1, 2], (3.5,))))

# Stepwise consumption to prove laziness — chain yields source 1's items
# before touching source 2.
it = chain([1, 2], [3, 4])
print("chain-next-1", next(it))
print("chain-next-2", next(it))
print("chain-rest", list(it))

# --- islice: 2-arg form (just stop) ---
print("islice-stop", list(islice([0, 1, 2, 3, 4, 5, 6, 7, 8, 9], 5)))

# --- islice: 3-arg form (start, stop) ---
print("islice-start-stop", list(islice([0, 1, 2, 3, 4, 5, 6, 7, 8, 9], 2, 7)))

# --- islice: 4-arg form (start, stop, step) ---
print("islice-step", list(islice([0, 1, 2, 3, 4, 5, 6, 7, 8, 9], 0, 10, 2)))

# --- islice: None in any slot is "default" ---
print("islice-none-start", list(islice(range(5), None, 4)))
print("islice-none-stop", list(islice(range(5), 1, None)))

# Empty slice
print("islice-empty", list(islice([1, 2, 3], 0, 0)))

# --- islice laziness probe (the load-bearing test) ---
# CountingSource records every `__next__` call.  `islice(src, 3)`
# must only pull what it yields plus any in-step skips — not
# materialise the whole source up front.

src1 = CountingSource()
print("islice-lazy-vals", list(islice(src1, 3)))
print("islice-lazy-pulled", src1.pulled)        # 3

src2 = CountingSource()
print("islice-step-vals", list(islice(src2, 0, 6, 2)))
print("islice-step-pulled", src2.pulled)        # 6 (pulled 1,2,3,4,5,6; yielded 1,3,5)

# `start > 0` also pulls eagerly past the prefix (matches CPython).
src3 = CountingSource()
print("islice-start-vals", list(islice(src3, 5, 8)))
print("islice-start-pulled", src3.pulled)       # 8 (skipped 5, then yielded 6,7,8)

# --- chain over INFINITE sources must stay lazy (regression) ---
# chain() must not drain its arguments at construction time; an infinite
# itertools source (repeat / count / cycle) as a tail used to hang forever.
print("chain-next-inf", next(chain([3], repeat(3))))
print("chain-repeat", list(islice(chain([3], repeat(3)), 3)))
print("chain-count", list(islice(chain([0], count(1)), 5)))
print("chain-cycle", list(islice(chain("ab", cycle("xy")), 6)))


# for/break over a chain whose tail is infinite must terminate on break.
def grouped(digits):
    groups = []
    for length in chain([3], repeat(3)):
        length = min(len(digits), length)
        groups.append(digits[-length:])
        digits = digits[:-length]
        if not digits:
            break
    return groups


print("chain-for-break", grouped("1234567"))
