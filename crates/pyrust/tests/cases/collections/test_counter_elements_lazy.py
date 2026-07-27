from collections import Counter


huge = Counter()
huge["x"] = 10**12
huge_elements = huge.elements()
print("huge first:", next(huge_elements), next(huge_elements))

live = Counter(a=2, b=1)
live_elements = live.elements()
print("live first:", next(live_elements))
live["b"] = 4
print("live rest:", list(live_elements))

guarded = Counter(a=1)
guarded_elements = guarded.elements()
guarded["b"] = 1
try:
    next(guarded_elements)
except RuntimeError as exc:
    print("size guard:", str(exc))

delayed = Counter()
delayed["bad"] = 1.5
delayed["ok"] = 2
delayed_elements = delayed.elements()
print("invalid constructed")
try:
    next(delayed_elements)
except TypeError as exc:
    print("invalid driven:", type(exc).__name__)
print("invalid resumed:", list(delayed_elements))

nonpositive = Counter()
nonpositive["zero"] = 0
nonpositive["negative"] = -4
nonpositive["true"] = True
print("nonpositive:", list(nonpositive.elements()))
