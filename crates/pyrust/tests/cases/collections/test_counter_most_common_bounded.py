from collections import Counter


def capture(label, callback):
    try:
        print(label + ":", callback())
    except Exception as exc:
        print(label + ":", type(exc).__name__, str(exc))


counts = Counter()
for key, count in [
    ("a", 5),
    ("b", 3),
    ("c", 5),
    ("d", 5),
    ("e", 2),
    ("f", 5),
    ("g", 1),
    ("h", 4),
    ("i", 4),
    ("j", 0),
    ("k", -1),
    ("l", 3),
]:
    counts[key] = count

print("all:", counts.most_common())
print("bounded two:", counts.most_common(2))
print("bounded ties:", counts.most_common(3))
print("near full:", counts.most_common(5))
print("one:", counts.most_common(1))
print("float one:", counts.most_common(1.0))
print("zero:", counts.most_common(0))
print("negative:", counts.most_common(-4))
print("false:", counts.most_common(False))
print("true:", counts.most_common(True))
print("huge:", counts.most_common(10**100))

capture("nonint float", lambda: counts.most_common(2.5))
capture("nonint str", lambda: counts.most_common("2"))


class BadCount:
    def __eq__(self, other):
        return False

    def __lt__(self, other):
        raise RuntimeError("lt boom")

    def __gt__(self, other):
        raise RuntimeError("gt boom")


bad = Counter()
for index in range(8):
    bad[index] = BadCount()

capture("bad full", lambda: bad.most_common())
capture("bad one", lambda: bad.most_common(1))
capture("bad bounded", lambda: bad.most_common(2))
print("bad zero:", bad.most_common(0))

mixed = Counter()
mixed["number"] = 1
mixed["text"] = "one"
capture("mixed full", lambda: mixed.most_common())
capture("mixed one", lambda: mixed.most_common(1))
print("mixed zero:", mixed.most_common(0))
