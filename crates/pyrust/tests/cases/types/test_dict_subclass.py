# Parity fixture for dict subclass __getitem__ and __missing__ dispatch (issue #1134).
# Case 1: user-defined __getitem__ on a dict subclass is called on subscript.
class LoggingDict(dict):
    def __getitem__(self, key):
        print("getitem called for " + repr(key))
        return super().__getitem__(key)

d = LoggingDict({"a": 1})
print(d["a"])

# Case 2: __missing__ is called when a key is absent.
class DefaultDict(dict):
    def __missing__(self, key):
        return "default(" + repr(key) + ")"

dd = DefaultDict()
dd["x"] = 42
print(dd["x"])
print(dd["y"])

# Case 3: __missing__ that mutates the dict (defaultdict-like pattern).
class Counter(dict):
    def __missing__(self, key):
        self[key] = 0
        return 0

c = Counter()
c["a"] += 1
print(c["a"])
print(c["b"])

# Case 4: subclass with neither override — native fast path still works.
class SubDict(dict):
    pass

sd = SubDict({"x": 10, "y": 20})
print(sd["x"])
try:
    sd["z"]
except KeyError as e:
    print("KeyError:", e)

# Case 5: format_map with __missing__.
class Defaulter(dict):
    def __missing__(self, key):
        return "<" + key + ">"

print("{x} and {y}".format_map(Defaulter(x=1)))

# Case 6: plain dict is unchanged (no regression).
plain = {"k": 99}
print(plain["k"])
try:
    plain["nope"]
except KeyError as e:
    print("KeyError:", e)
