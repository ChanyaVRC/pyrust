"""Parity: CmpJumpIfFalseConst/TrueConst str fast path (avoids Value::clone)."""

# Eq / Ne hot path — tight loop so the fast path matters
def count_matches(words, target):
    n = 0
    for w in words:
        if w == target:
            n += 1
    return n

words = ["hello", "world", "hello", "foo", "hello"]
print(count_matches(words, "hello"))   # 3
print(count_matches(words, "world"))   # 1
print(count_matches(words, "bar"))     # 0

# Ordering comparisons
def first_gt(words, pivot):
    for w in words:
        if w > pivot:
            return w
    return None

tokens = ["apple", "banana", "cherry", "date"]
print(first_gt(tokens, "b"))          # banana
print(first_gt(tokens, "z"))          # None

# Ne fast path
def count_non_empty(items):
    n = 0
    for s in items:
        if s != "":
            n += 1
    return n

print(count_non_empty(["a", "", "b", "", "c"]))  # 3
