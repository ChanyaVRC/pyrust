"""
Parity fixture: print(file=obj) calls obj.write() instead of writing to stdout.
Issue #1128.
"""

class Collector:
    def __init__(self):
        self.data = []
    def write(self, s):
        self.data.append(s)


# Single item: write(item), write(end)
w = Collector()
print("hello", file=w)
print(w.data)

# Multiple items: write(a), write(sep), write(b), write(end)
w2 = Collector()
print("a", "b", file=w2)
print(w2.data)

# Custom sep
w3 = Collector()
print("x", "y", file=w3, sep=",")
print(w3.data)

# Custom end (empty string)
w4 = Collector()
print("hi", file=w4, end="")
print(w4.data)

# file=None falls back to stdout
print("stdout", file=None)

# No positional args: only write(end) is called
w5 = Collector()
print(file=w5)
print(w5.data)

# AttributeError when file has no write attribute
try:
    print("x", file=42)
except AttributeError as e:
    print("AttributeError:", e)
