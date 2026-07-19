# bool is a subclass of int, so it must be orderable against float and
# arbitrarily large int (BigInt) everywhere a numeric comparison happens.
# Regression for a gap in the sort/min/max internal comparator
# (compare_values_with_op): it handled Bool<->Bool/Int but not Bool<->Float
# or Bool<->BigInt, so `sorted([1.5, True, 0.0])` raised TypeError while the
# plain `True < 1.5` operator worked.

# sorted() with bool + float
print(sorted([1.5, 1, True, 0]))
print(sorted([1.5, True, 0.0]))
print(sorted([True, 0.5, False, 2.0]))

# sorted() with bool + BigInt
big = 10 ** 30
print(sorted([big, True, 0, False, -big]))
print(sorted([True, big, False]))

# min / max mixing bool with float and BigInt
print(min(True, 0.5), max(False, 1.5, True))
print(min(big, True, 0.0), max(big, True, 0.0))

# list / tuple ordering (recurses through the same comparator)
print([True, 2.0] < [True, 3.0])
print([False, big] < [True, 0])
print((True, 1.5) < (True, 2.5))

# reverse=True path
print(sorted([1.5, True, 0.0, 2.0], reverse=True))

# float/bool and bigint/bool both orientations via a shuffled list
print(sorted([2.0, False, big, True, -1.5, -big]))
