"""Counter nonpositive cleanup is atomic, stable, and iterator-aware."""

from collections import Counter


counts = Counter()
for key, count in [
    ("drop-first", 0),
    ("keep-first", 4),
    ("drop-middle", -2),
    ("keep-last", 1),
    ("drop-last", 0),
]:
    counts[key] = count
counts += Counter()
print("stable-survivors", list(counts.items()))


class CompareBoom:
    def __gt__(self, other):
        raise RuntimeError("compare boom")


counts = Counter()
counts["zero-a"] = 0
counts["zero-b"] = -1
counts["boom"] = CompareBoom()
try:
    counts += Counter()
except RuntimeError as error:
    print("comparison-atomic", str(error), list(counts)[:2])


counts = Counter(a=0, b=2, c=-1, d=3)
iterator = iter(counts)
print("iterator-first", next(iterator))
counts += Counter()
print("iterator-survivors", list(counts))
try:
    next(iterator)
except RuntimeError as error:
    print("iterator-mutation", str(error))


class ReentrantCount:
    def __gt__(self, other):
        reentrant["side-effect"] = 1
        return True


reentrant = Counter()
reentrant["zero"] = 0
reentrant["probe"] = ReentrantCount()
try:
    reentrant += Counter()
except RuntimeError as error:
    print("reentrant-mutation", str(error), "zero" in reentrant, "side-effect" in reentrant)
