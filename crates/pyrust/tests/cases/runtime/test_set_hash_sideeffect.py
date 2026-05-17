calls = []

class H:
    def __hash__(self):
        calls.append(1)
        return 42

    def __eq__(self, other):
        return True

s = set([H()])
print(len(s))          # 1
print(len(calls) >= 1) # True — __hash__ was called
