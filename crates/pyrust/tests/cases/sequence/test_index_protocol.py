# list.index and tuple.index resolve start/stop through __index__.
#
# CPython 3.12 semantics:
# - An object that defines __index__ is accepted as start or stop.
# - Objects without __index__ raise TypeError.
# - __index__ returning a non-int raises TypeError.
# - Plain int/bool start/stop still work (regression guard).

class MyIndex:
    def __init__(self, n):
        self.n = n

    def __index__(self):
        return self.n


lst = [10, 20, 30, 40]
t = (10, 20, 30, 40)

# __index__ on start argument
print(lst.index(30, MyIndex(1)))       # 2: start=1, 30 is at idx 2
print(t.index(30, MyIndex(1)))         # 2

# __index__ on stop argument
print(lst.index(10, 0, MyIndex(3)))    # 0: stop=3, 10 is at idx 0
print(t.index(10, 0, MyIndex(3)))      # 0

# __index__ on both start and stop
print(lst.index(30, MyIndex(1), MyIndex(4)))  # 2

# Plain int start/stop still work
print(lst.index(30, 1))       # 2
print(lst.index(30, 1, 4))    # 2
print(t.index(30, 1, 4))      # 2

# TypeError when no __index__ method
try:
    lst.index(30, "oops")
except TypeError as e:
    print(e)

try:
    lst.index(30, 1, "oops")
except TypeError as e:
    print(e)

try:
    t.index(30, "oops")
except TypeError as e:
    print(e)

# TypeError when __index__ returns non-int
class BadIndex:
    def __index__(self):
        return "not an int"

try:
    lst.index(30, BadIndex())
except TypeError as e:
    print(e)

try:
    t.index(30, BadIndex())
except TypeError as e:
    print(e)
