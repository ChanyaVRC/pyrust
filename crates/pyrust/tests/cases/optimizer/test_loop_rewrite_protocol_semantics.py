# Loop-shape rewrites must preserve protocol choice and evaluation count.


bool_events = []


class BoolProbe:
    def __bool__(self):
        bool_events.append(len(bool_events) + 1)
        return True


probe = BoolProbe()
rounds = 0
while probe:
    rounds += 1
    if rounds == 3:
        break

print("while-bool", rounds, bool_events)


tail_events = []


class TailProbe:
    def __bool__(self):
        tail_events.append("bool")
        return False


tail_probe = TailProbe()
for _ in (1, 2, 3):
    if tail_probe:
        continue

print("tail-continue", tail_events)


compare_events = []


class RichProbe:
    def __lt__(self, other):
        compare_events.append(("lt", other))
        return False

    def __ge__(self, other):
        compare_events.append(("ge", other))
        return True


rich = RichProbe()
rounds = 0
while True:
    if rich < 1:
        break
    rounds += 1
    if rounds == 2:
        break

print("break-rich-compare", rounds, compare_events)
