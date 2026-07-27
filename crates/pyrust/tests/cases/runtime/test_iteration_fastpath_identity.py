"""Iteration specializations must retain Python iterator identity and slots."""


# A list iterator owns the list selected by iter(source), not the variable that
# happened to hold it. Rebinding that variable cannot redirect or exhaust the
# active iterator.
items = [1, 2, 3]
seen = []
for item in items:
    seen.append(item)
    items = [9]
print("rebound-source", seen)


# enumerate is its own iterator. A for-loop and next() through another reference
# must advance the same state rather than letting the loop steal its source.
iterator = enumerate([10, 20, 30])
loop_indices = []
alias_indices = []
stopped = 0
for pair in iterator:
    loop_indices.append(pair[0])
    try:
        alias_indices.append(next(iterator)[0])
    except StopIteration:
        stopped += 1
print("enumerate-alias", loop_indices, alias_indices, stopped)


# Replacing a class's __next__ slot during iteration is visible on the following
# step. A cached unbound method needs the same class version/epoch guards as
# other method caches.
class Iterator:
    def __init__(self):
        self.index = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.index += 1
        if self.index == 1:
            Iterator.__next__ = replacement
            return "original"
        raise StopIteration


def replacement(self):
    self.index += 1
    if self.index == 2:
        return "replacement"
    raise StopIteration


slot_seen = []
for item in Iterator():
    slot_seen.append(item)
print("next-slot", slot_seen)
