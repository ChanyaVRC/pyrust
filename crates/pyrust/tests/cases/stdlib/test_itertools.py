# itertools.chain / islice — both lazy iterators in pyrust.

from itertools import chain, islice

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
