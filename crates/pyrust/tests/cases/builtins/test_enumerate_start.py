print(list(enumerate(["a", "b"], 10)))
print(list(enumerate(["a", "b"], start=10)))
print(list(enumerate(["a", "b"])))
print(list(enumerate([], start=5)))
print(list(enumerate(["x"], -3)))
# Type errors
try:
    list(enumerate(["a"], "bad"))
    print("type-error", "FAIL")
except TypeError:
    print("type-error", "TypeError")
# Duplicate args
try:
    list(enumerate(["a"], 1, start=2))
    print("dup-arg", "FAIL")
except TypeError:
    print("dup-arg", "TypeError")


# Issue #2897: enumerate's start argument uses the index protocol and keeps
# omitted start distinct from an explicitly supplied None.


def report(label, fn):
    try:
        print(label, fn())
    except Exception as exc:
        print(label, type(exc).__name__, str(exc))


class IndexStart:
    def __init__(self):
        self.calls = 0

    def __index__(self):
        self.calls += 1
        return 7


class RaisingIndex:
    def __init__(self):
        self.calls = 0

    def __index__(self):
        self.calls += 1
        raise ValueError("index boom")


report("none", lambda: list(enumerate(["a"], None)))

index_start = IndexStart()
report("index", lambda: list(enumerate(["a", "b"], index_start)))
print("index-calls", index_start.calls)

raising_start = RaisingIndex()
report("raising", lambda: enumerate(["a"], raising_start))
print("raising-calls", raising_start.calls)

print("bool", list(enumerate(["a", "b"], True)))
big_start = 1 << 100
print("big", list(enumerate(["a", "b"], big_start)))
