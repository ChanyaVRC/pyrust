"""A source while-loop must keep its resolved call and indexing protocols."""


def shadowed_len_case():
    def len(value):
        return 0

    items = [1, 2]
    index = 0
    seen = []
    while index < len(items):
        seen.append(items[index])
        index += 1
    return seen


events = []


class ProtocolSequence:
    def __len__(self):
        events.append("len")
        return 2

    def __getitem__(self, index):
        events.append(("get", index))
        if index >= 2:
            raise IndexError
        return index + 10


sequence = ProtocolSequence()
index = 0
seen = []
while index < len(sequence):
    seen.append(sequence[index])
    index += 1

print(shadowed_len_case())
print(events)
print(seen)
assert shadowed_len_case() == []
assert events == ["len", ("get", 0), "len", ("get", 1), "len"]
assert seen == [10, 11]
