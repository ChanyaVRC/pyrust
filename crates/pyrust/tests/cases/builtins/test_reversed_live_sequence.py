"""reversed() fixes the initial length but reads each index lazily."""


values = [1, 2, 3]
iterator = reversed(values)
values.append(4)
values[2] = 9
print(next(iterator))
values[1] = 8
print(next(iterator))
values.insert(0, 8)
print(next(iterator))
print(list(iterator))


data = bytearray(b"abc")
iterator = reversed(data)
data.append(ord("d"))
data[2] = ord("z")
print(next(iterator), next(iterator))


events = []


class Sequence:
    def __init__(self):
        self.values = [10, 20, 30]

    def __len__(self):
        events.append("len")
        return len(self.values)

    def __getitem__(self, index):
        events.append(("get", index))
        return self.values[index]


sequence = Sequence()
iterator = reversed(sequence)
print(events)
sequence.values[2] = 99
print(next(iterator), events)
print(list(iterator), events)
