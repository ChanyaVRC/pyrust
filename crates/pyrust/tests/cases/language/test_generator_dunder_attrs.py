"""
Parity test for generator __iter__ and __next__ attribute access (issue #1413).

Verifies that hasattr/getattr see __iter__, __next__, send, close, throw on
generator objects, and that the retrieved methods work correctly.
"""


def gen():
    yield 1
    yield 2


g = gen()

# hasattr must see the iteration protocol methods
print(hasattr(g, '__iter__'))   # True
print(hasattr(g, '__next__'))   # True
print(hasattr(g, 'send'))       # True
print(hasattr(g, 'close'))      # True
print(hasattr(g, 'throw'))      # True

# getattr must NOT return the default sentinel
print(getattr(g, '__iter__', None) is not None)   # True
print(getattr(g, '__next__', None) is not None)   # True

# __iter__() returns the generator itself (identity)
g2 = gen()
print(g2.__iter__() is g2)     # True

# __next__() advances the generator one step at a time
g3 = gen()
print(g3.__next__())            # 1
print(g3.__next__())            # 2

# Exhausted generator raises StopIteration via __next__
try:
    g3.__next__()
    print("no error")
except StopIteration:
    print("StopIteration")      # StopIteration

# Bound-method captured via getattr is callable and works
g4 = gen()
nxt = g4.__next__
print(nxt())                    # 1
print(nxt())                    # 2

# send() works via attribute access
g5 = gen()
s = g5.send
print(s(None))                  # 1

# close() via attribute access exhausts the generator
g6 = gen()
g6.close()
try:
    next(g6)
    print("no StopIteration")
except StopIteration:
    print("closed OK")          # closed OK

# type(gen).__iter__ and type(gen).__next__ are accessible
print(hasattr(type(g), '__iter__'))  # True
print(hasattr(type(g), '__next__'))  # True

# Unbound descriptor call: type(gen).__iter__(g) returns g
g7 = gen()
print(type(g7).__iter__(g7) is g7)   # True

# Unbound descriptor call: type(gen).__next__(g) advances g
g8 = gen()
print(type(g8).__next__(g8))         # 1

# Non-generator iterator (NativeIterFrame) also exposes __iter__/__next__
lst_iter = iter([10, 20, 30])
print(hasattr(lst_iter, '__iter__'))       # True
print(hasattr(lst_iter, '__next__'))       # True
print(lst_iter.__iter__() is lst_iter)    # True
print(lst_iter.__next__())                 # 10

# Wrong-type descriptor call raises TypeError
try:
    type(g).__iter__(42)
except TypeError:
    print("TypeError on wrong type")       # TypeError on wrong type
