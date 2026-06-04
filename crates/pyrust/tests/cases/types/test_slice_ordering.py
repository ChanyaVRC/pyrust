# slice objects order by their (start, stop, step) tuples, matching CPython
# 3.12's slice_richcompare (issue #2127).  <, <=, >, >= now work; == / != are
# unchanged.  Mixed None/int bounds raise the same TypeError the equivalent
# tuple comparison would.


def t(code):
    try:
        print(code, "=>", eval(code))
    except TypeError as e:
        print(code, "=> TypeError:", e)


# Ordering by start, then stop, then step.
t("slice(1, 2) < slice(1, 3)")        # True
t("slice(1, 3) < slice(1, 2)")        # False
t("slice(1, 5) < slice(2, 3)")        # True (start compared first)
t("slice(1, 2, 1) < slice(1, 2, 2)")  # True (step participates)
t("slice(1, 2, 3) < slice(1, 2, 4)")  # True

# <=, >, >=.
t("slice(1, 2) <= slice(1, 2)")       # True (equal slices, no None<None error)
t("slice(2, 3) > slice(1, 3)")        # True
t("slice(1, 2) >= slice(1, 2)")       # True
t("slice(1, 2) > slice(1, 2)")        # False

# == / != unchanged.
print(slice(1, 2, 3) == slice(1, 2, 3))  # True
print(slice(1, 2) != slice(1, 3))        # True
print(slice(1, 2) == slice(1, 2, None))  # True (missing step is None)

# None-field prefix equality, then int ordering.
t("slice(None, 2) < slice(None, 3)")  # True (None == None, then 2 < 3)
t("slice(None) < slice(None)")        # False (all None, equal)

# Mixed None/int bounds: unorderable, like the equivalent tuple comparison.
t("slice(None, 2) < slice(1, 2)")     # TypeError: None < 1
t("slice(1) < slice(1, 2)")           # TypeError (slice(1) is (None, 1, None))
t("slice(1, 2) < slice(1, 2, 3)")     # TypeError (None < 3)

# slice vs non-slice stays a TypeError.
t("slice(1, 2) < 5")
t("5 < slice(1, 2)")

# Non-int bounds compare by their own ordering.
t("slice('a', 'b') < slice('a', 'c')")  # True
